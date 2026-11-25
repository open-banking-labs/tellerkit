//! 交易引擎（P1 第一片）：状态机 + 幂等 + 离线边界。
//!
//! 这一层刻意不碰 IO：主机是 trait，落盘与重放在后续切片里接。
//! 原因很实际——交易状态机是柜面最容易出错的地方，它必须能脱离网络、
//! 数据库和设备，用纯内存用例把每条分支跑一遍。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use ts_rs::TS;

/// 一笔交易对柜员而言只有三种可见状态。
///
/// 刻意不做 `Unknown`：状态不明必须有一个**明确处置**（挂起重放，或标记待冲正），
/// 客户端不该有"我不知道"这种既不重试也不查账的终态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case")]
#[ts(export)]
pub enum TxStatus {
    /// 主机已受理，拿到流水号。
    Committed,
    /// 网络不可用且该交易允许离线：已挂起，等待重放。
    Pending,
    /// 明确失败：业务拒绝。
    Rejected,
    /// **状态不明**：超时/不可达，且该交易不允许离线。
    ///
    /// 这不是"失败"——主机可能已经入账了。把它报成 Rejected 会让柜员重做一笔，
    /// 而主机可能已经扣过款。正确处置是标记待冲正、查账确认（`core::reversal`）。
    MarkedForReversal,
}

/// 交易请求。幂等键由**客户端**生成——断网时拿不到服务端 ID。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TxRequest {
    pub tx_code: String,
    /// 客户端生成的幂等键（UUID）。同一笔业务只能有一个。
    pub idempotency_key: String,
    /// 金额（分）。非资金类交易为 0。
    pub amount_cents: i64,
    /// 交易要素，键值均为字符串，避免前端出现类型歧义。
    pub elements: BTreeMap<String, String>,
    /// 是否允许离线挂起。资金类默认 false，只有服务端配置能覆盖。
    pub offline_capable: bool,
}

impl TxRequest {
    pub fn new(tx_code: &str, key: &str) -> Self {
        Self {
            tx_code: tx_code.to_string(),
            idempotency_key: key.to_string(),
            amount_cents: 0,
            elements: BTreeMap::new(),
            offline_capable: false,
        }
    }

    pub fn amount(mut self, cents: i64) -> Self {
        self.amount_cents = cents;
        self
    }

    pub fn offline(mut self, capable: bool) -> Self {
        self.offline_capable = capable;
        self
    }

    pub fn element(mut self, k: &str, v: &str) -> Self {
        self.elements.insert(k.to_string(), v.to_string());
        self
    }
}

/// 提交结果。`replayable` 是重放队列的唯一判据，不靠调用方猜。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TxResult {
    pub status: TxStatus,
    pub reference: Option<String>,
    pub message: String,
    pub replayable: bool,
}

impl TxResult {
    fn committed(reference: String) -> Self {
        Self {
            status: TxStatus::Committed,
            reference: Some(reference),
            message: "交易成功".into(),
            replayable: false,
        }
    }

    fn pending(message: &str) -> Self {
        Self {
            status: TxStatus::Pending,
            reference: None,
            message: message.into(),
            replayable: true,
        }
    }

    fn rejected(message: &str) -> Self {
        Self {
            status: TxStatus::Rejected,
            reference: None,
            message: message.into(),
            replayable: false,
        }
    }
}

/// 主机返回的错误。区分"没送到"与"送到了但业务拒绝"——两者的处置完全不同。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// 超时 / 网络不可达：不知道主机是否处理了。
    #[error("主机超时")]
    Timeout,
    /// 明确不可达（连接被拒）。
    #[error("主机不可达")]
    Unavailable,
    /// 主机已受理并明确拒绝（如余额不足）。
    #[error("主机拒绝：{0}")]
    Rejected(String),
}

/// 持久化端口。core 只定义"需要记什么"，不关心存哪里。
///
/// 刻意不把 SQLite 放进 core：这一层必须能脱离磁盘与驱动做纯逻辑测试。
/// 实现放在 `packages/journal-sqlite`，由主壳装配。
///
/// 方法返回 `Result` 而不是静默失败：记账失败必须能被看见，
/// 但也不允许它改变交易结论——结论由主机与状态机决定。
pub trait TxJournal: Send + Sync {
    /// 追加一条交易结论（审计日志，只增）。
    fn record(&self, request: &TxRequest, result: &TxResult) -> Result<(), String>;
    /// 启动时读回幂等索引：(幂等键, 载荷签名, 首次结果)。
    ///
    /// 默认空实现给纯内存模式；接了账本的实现必须真正读回，
    /// 否则重启后同一个键会被当成新交易再次打到主机。
    fn load_idempotent(&self) -> Result<Vec<(String, String, TxResult)>, String> {
        Ok(Vec::new())
    }
    /// 把一条可缓存的幂等结论落盘。
    fn save_idempotent(
        &self,
        _key: &str,
        _signature: &str,
        _result: &TxResult,
    ) -> Result<(), String> {
        Ok(())
    }
    /// 写前意图：调用主机**之前**落盘。
    ///
    /// 默认空实现给纯内存预览用；接了持久化账本的实现必须真正落盘，
    /// 否则崩溃后无法区分"主机已受理但本地没记账"与"从未提交"。
    fn record_intent(&self, _request: &TxRequest) -> Result<(), String> {
        Ok(())
    }
    /// 结论落盘与意图收口放在同一个事务里。
    ///
    /// 默认退化为逐条 record + dequeue，仅用于没有事务能力的实现。
    fn settle(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        self.record(request, result)?;
        if result.status != TxStatus::Pending {
            self.dequeue(&request.idempotency_key)?;
        }
        Ok(())
    }
    /// 启动时读出上次崩溃遗留的在途意图，由上层转入异常队列。
    fn in_flight_intents(&self) -> Result<Vec<TxRequest>, String> {
        Ok(Vec::new())
    }
    /// 挂起请求入队，供重启后重放。
    fn enqueue(&self, request: &TxRequest) -> Result<(), String>;
    /// 重放完成后出队。
    fn dequeue(&self, idempotency_key: &str) -> Result<(), String>;
    /// 启动时读取未完成的挂起请求。
    fn pending(&self) -> Result<Vec<TxRequest>, String>;
}

/// 主机接口。真实实现走网络，测试实现可以精确控制失败方式。
pub trait TxHost {
    fn submit(&self, request: &TxRequest) -> Result<String, HostError>;

    /// 主机是否按 `idempotency_key` 去重。
    ///
    /// 默认 `false`：**未知即不安全**。离线挂起意味着"以后一定会重发"，
    /// 而重发只有在校验键去重时才不会重复入账。主机没声明，就不许挂起。
    fn supports_idempotency(&self) -> bool {
        false
    }
}

/// 演示用主机：把"在线/离线/业务拒绝"做成可切换的开关。
///
/// 真实实现走网络；这个版本让界面能在没有后端的情况下演示完整状态机。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxHostMode {
    Online,
    Offline,
    Reject(String),
}

pub struct DemoTxHost {
    mode: Mutex<TxHostMode>,
    seq: AtomicU64,
}

impl Default for DemoTxHost {
    fn default() -> Self {
        Self {
            mode: Mutex::new(TxHostMode::Online),
            seq: AtomicU64::new(0),
        }
    }
}

impl DemoTxHost {
    pub fn new(mode: TxHostMode) -> Self {
        Self {
            mode: Mutex::new(mode),
            seq: AtomicU64::new(0),
        }
    }

    pub fn set_mode(&self, mode: TxHostMode) {
        *self.mode.lock().unwrap_or_else(|e| e.into_inner()) = mode;
    }

    pub fn mode(&self) -> TxHostMode {
        self.mode.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

impl TxHost for DemoTxHost {
    fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
        match self.mode() {
            TxHostMode::Online => {
                let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
                Ok(format!("TX{n:08}"))
            }
            TxHostMode::Offline => Err(HostError::Unavailable),
            TxHostMode::Reject(reason) => Err(HostError::Rejected(reason)),
        }
    }
}

/// 一笔已从待重放队列取出、等待主机回执的重放。
#[derive(Debug)]
pub struct ReplayTicket {
    request: TxRequest,
    signature: String,
}

impl ReplayTicket {
    pub fn request(&self) -> &TxRequest {
        &self.request
    }

    pub fn key(&self) -> &str {
        &self.request.idempotency_key
    }
}

/// 交易引擎。幂等索引常驻内存；待重放队列可选持久化。
#[derive(Default)]
pub struct TxEngine {
    /// 幂等键 -> (载荷签名, 首次结果)。
    ///
    /// 存签名是为了区分"同一笔业务重复提交"与"同一个键被挪作他用"——
    /// 后者必须显式拒绝，否则柜员会把另一笔业务误认为已成功。
    idempotent: HashMap<String, (String, TxResult)>,
    /// 待重放请求。
    pending: Vec<TxRequest>,
    /// 持久化端口；None 表示纯内存模式（测试与浏览器预览）。
    journal: Option<Box<dyn TxJournal>>,
    /// 最近一次记账失败。不改变交易结论，但必须能被上层看见。
    journal_error: Option<String>,
    /// 上次崩溃时未完成主机调用的交易，启动时读出等待上层转异常。
    in_flight: Vec<TxRequest>,
    /// "正在飞"的幂等键 -> 载荷签名。
    ///
    /// 与 `in_flight` 不是一回事：`in_flight` 是**上次崩溃**遗留的意图（落盘读回），
    /// 这里是**本进程正在打主机**的那几笔。作用也不同：幂等索引防"重放已完成的交易"，
    /// 这里防"同一笔被并发提交两次"——索引要等结论落定才有记录，覆盖不到
    /// "第一笔还没回来"的窗口。
    submitting: HashMap<String, String>,
}

impl std::fmt::Debug for TxEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TxEngine")
            .field("idempotent_keys", &self.idempotent.len())
            .field("pending", &self.pending.len())
            .field("journaled", &self.journal.is_some())
            .field("submitting", &self.submitting.len())
            .finish()
    }
}

/// 提交第一阶段的结果。
#[derive(Debug)]
pub enum SubmitStart {
    /// 放行：调用方去打主机，结论必须交回 [`TxEngine::finish_submit`]。
    Proceed { signature: String },
    /// 不必打主机：已有结论（重复提交/幂等键冲突/在途）或参数、落盘不成立。
    ShortCircuited(TxResult),
}

impl TxEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// 挂上持久化端口，并把上次未完成的挂起请求读回来。
    ///
    /// 读队列失败不阻断启动：柜面宁可"能开但提示异常"，也不要"打不开"。
    pub fn with_journal(mut self, journal: Box<dyn TxJournal>) -> Self {
        match journal.pending() {
            Ok(list) => self.pending = list,
            Err(e) => self.journal_error = Some(format!("读取待重放队列失败：{e}")),
        }
        match journal.in_flight_intents() {
            Ok(list) => self.in_flight = list,
            Err(e) => self.journal_error = Some(format!("读取在途交易意图失败：{e}")),
        }
        match journal.load_idempotent() {
            Ok(list) => {
                for (key, signature, result) in list {
                    self.idempotent.insert(key, (signature, result));
                }
            }
            Err(e) => self.journal_error = Some(format!("读取幂等索引失败：{e}")),
        }
        self.journal = Some(journal);
        self
    }

    /// 取走最近一次记账错误（取走后清空，避免重复上报）。
    pub fn take_journal_error(&mut self) -> Option<String> {
        self.journal_error.take()
    }

    /// 取走上次崩溃遗留的在途交易，由上层转入异常队列处置。
    pub fn take_in_flight(&mut self) -> Vec<TxRequest> {
        std::mem::take(&mut self.in_flight)
    }

    /// 幂等索引的内存与落盘写入放在一处，避免漏掉重放路径。
    fn cache_result(&mut self, key: String, signature: String, result: &TxResult) {
        self.idempotent
            .insert(key.clone(), (signature.clone(), result.clone()));
        if let Some(journal) = self.journal.as_deref() {
            if let Err(e) = journal.save_idempotent(&key, &signature, result) {
                self.journal_error = Some(format!("幂等索引落盘失败：{e}"));
            }
        }
    }

    /// 阶段一：校验 → 幂等查表 → 在途去重 → 写前意图落盘。
    ///
    /// 顺序刻意如此：先校验、再查幂等、最后才碰主机。把幂等检查放在最前，
    /// 是因为"重复提交"不该因为主机暂时不可用而变成失败。
    ///
    /// 返回 `Proceed` 后**必须**调用 [`Self::finish_submit`]：否则该幂等键会一直
    /// 占着在途位，之后同一笔会被误判成"正在处理中"。
    pub fn begin_submit(&mut self, request: &TxRequest) -> SubmitStart {
        if let Some(msg) = validate(request) {
            return SubmitStart::ShortCircuited(TxResult::rejected(&msg));
        }

        let signature = request_signature(request);
        if let Some((known, previous)) = self.idempotent.get(&request.idempotency_key) {
            if *known != signature {
                return SubmitStart::ShortCircuited(TxResult::rejected(
                    "幂等键已用于另一笔交易（交易码或要素不同），请为该笔业务使用新的幂等键",
                ));
            }
            let mut replay = previous.clone();
            replay.message = format!("重复提交：返回首次结果（{}）", previous.message);
            return SubmitStart::ShortCircuited(replay);
        }

        // 在途去重：结论还没回来就又一次提交同一笔。放过去就是两笔。
        // 幂等索引要等结论落定才有记录，覆盖不到这个窗口——这就是本表存在的理由。
        if let Some(in_flight_signature) = self.submitting.get(&request.idempotency_key) {
            if *in_flight_signature != signature {
                return SubmitStart::ShortCircuited(TxResult::rejected(
                    "幂等键已用于另一笔交易（交易码或要素不同），请为该笔业务使用新的幂等键",
                ));
            }
            return SubmitStart::ShortCircuited(TxResult::rejected(
                "同一笔交易正在处理中，请勿重复提交；若上一笔超时，请先查账确认再决定是否重试",
            ));
        }
        self.submitting
            .insert(request.idempotency_key.clone(), signature.clone());

        // 写前意图必须先落盘：崩溃时至少能知道主机可能受理了哪一笔。
        // 落盘失败就绝不碰主机——否则事故后没有任何证据可查。
        if let Some(journal) = self.journal.as_deref() {
            if let Err(e) = journal.record_intent(request) {
                self.submitting.remove(&request.idempotency_key);
                return SubmitStart::ShortCircuited(TxResult::rejected(&format!(
                    "写前意图落盘失败，交易未提交：{e}"
                )));
            }
        }
        SubmitStart::Proceed { signature }
    }

    /// 阶段二：把主机结论落成账，并释放在途标记。
    ///
    /// 瞬时失败（超时/不可达）**不写幂等索引**：此刻我们并不知道主机是否已处理。
    /// 若把"拒绝"永久缓存，柜员就再也无法安全重试；而换个新键重试又可能重复入账。
    /// 正确做法是保留原键，让重试打到主机，由主机的幂等键去重收口。
    pub fn finish_submit(
        &mut self,
        request: &TxRequest,
        signature: &str,
        outcome: Result<String, HostError>,
    ) -> TxResult {
        let (result, cacheable) = match outcome {
            Ok(reference) => (TxResult::committed(reference), true),
            Err(HostError::Rejected(reason)) => {
                (TxResult::rejected(&format!("主机拒绝：{reason}")), true)
            }
            Err(HostError::Timeout) | Err(HostError::Unavailable) if request.offline_capable => {
                self.pending.push(request.clone());
                (
                    TxResult::pending("网络不可用：交易已挂起，将在恢复后重放"),
                    true,
                )
            }
            Err(HostError::Timeout) | Err(HostError::Unavailable) => (
                // 状态不明，不是拒绝。不缓存幂等索引（cacheable=false），
                // 由上层标记待冲正并查账。
                TxResult {
                    status: TxStatus::MarkedForReversal,
                    reference: None,
                    message: "网络不可用且该交易不支持离线：状态不明，已标记待冲正，请查账确认"
                        .to_string(),
                    replayable: false,
                },
                false,
            ),
        };

        if cacheable {
            self.cache_result(
                request.idempotency_key.clone(),
                signature.to_string(),
                &result,
            );
        }

        // 记账失败不改变结论：交易已经发生，日志没写成是运维问题，不是业务问题。
        if let Some(journal) = self.journal.as_deref() {
            if let Err(e) = journal.settle(request, &result) {
                self.journal_error = Some(e);
            }
        }
        self.submitting.remove(&request.idempotency_key);
        result
    }

    /// 提交一笔交易（调用方持锁期间一次做完）。
    ///
    /// 拆成两阶段是为了让上层能把**主机调用**放在两阶段之间（P0-10 第 2b 步）；
    /// 此刻两段仍连续执行，行为与拆分前完全一致。
    pub fn submit(&mut self, host: &dyn TxHost, request: TxRequest) -> TxResult {
        let signature = match self.begin_submit(&request) {
            SubmitStart::Proceed { signature } => signature,
            SubmitStart::ShortCircuited(result) => return result,
        };
        let outcome = host.submit(&request);
        self.finish_submit(&request, &signature, outcome)
    }

    /// 待重放的请求（按提交顺序）。
    pub fn pending_requests(&self) -> &[TxRequest] {
        &self.pending
    }

    /// 重放**单笔**挂起交易，供调度器按退避节奏逐笔调用。
    ///
    /// 返回 None 表示队列里没有这笔（可能已被别处重放）。
    /// 重放阶段一：把该笔从待重放队列**取出**，交给调用方去锁外发送。
    ///
    /// 取出而不是只读取，是为了当"在途占位"：重放期间同一笔不会再被另一轮重放
    /// 取到（否则两轮都会打主机）。进程崩了也不怕——未成功的仍在账本队列里，
    /// 下次启动会读回。
    ///
    /// 返回 `None` 表示队列里没有这笔（可能已被手动重放掉）。
    pub fn take_for_replay(&mut self, idempotency_key: &str) -> Option<ReplayTicket> {
        let index = self
            .pending
            .iter()
            .position(|r| r.idempotency_key == idempotency_key)?;
        let request = self.pending.remove(index);
        Some(ReplayTicket {
            signature: request_signature(&request),
            request,
        })
    }

    /// 重放阶段三：把主机回执落账。不可达时放回队列，绝不丢单。
    pub fn finish_replay(
        &mut self,
        ticket: ReplayTicket,
        outcome: Result<String, HostError>,
    ) -> (String, TxResult) {
        let key = ticket.request.idempotency_key.clone();
        let result = match outcome {
            Ok(reference) => TxResult::committed(reference),
            Err(HostError::Rejected(reason)) => TxResult::rejected(&format!("主机拒绝：{reason}")),
            Err(HostError::Timeout) | Err(HostError::Unavailable) => {
                // 不可达：放回队列，绝不丢单
                self.pending.push(ticket.request.clone());
                TxResult::pending("重放失败：仍不可达，保留在队列中")
            }
        };

        self.cache_result(key.clone(), ticket.signature, &result);

        if let Some(journal) = self.journal.as_deref() {
            if let Err(e) = journal.settle(&ticket.request, &result) {
                self.journal_error = Some(e);
            }
        }
        (key, result)
    }

    pub fn replay_one(
        &mut self,
        host: &dyn TxHost,
        idempotency_key: &str,
    ) -> Option<(String, TxResult)> {
        let ticket = self.take_for_replay(idempotency_key)?;
        let outcome = host.submit(ticket.request());
        Some(self.finish_replay(ticket, outcome))
    }

    /// 重放全部挂起交易。成功的移出队列，失败的保留。
    ///
    /// 返回每笔的结果，便于上层逐笔向柜员展示——批次里失败一笔不代表全批失败。
    pub fn replay_pending(&mut self, host: &dyn TxHost) -> Vec<(String, TxResult)> {
        let drained = std::mem::take(&mut self.pending);
        let mut outcomes = Vec::new();
        for request in drained {
            let key = request.idempotency_key.clone();
            let signature = request_signature(&request);
            // 直接走主机，不查幂等：重放的前提就是"上次没成功"
            let result = match host.submit(&request) {
                Ok(reference) => TxResult::committed(reference),
                Err(HostError::Rejected(reason)) => {
                    TxResult::rejected(&format!("主机拒绝：{reason}"))
                }
                Err(HostError::Timeout) | Err(HostError::Unavailable) => {
                    self.pending.push(request.clone());
                    TxResult::pending("重放失败：仍不可达，保留在队列中")
                }
            };
            self.cache_result(key.clone(), signature, &result);
            if let Some(journal) = self.journal.as_deref() {
                if let Err(e) = journal.settle(&request, &result) {
                    self.journal_error = Some(e);
                }
            }
            outcomes.push((key, result));
        }
        outcomes
    }
}

/// 载荷签名：幂等键相同但内容不同，说明键被复用，必须拒绝。
fn request_signature(request: &TxRequest) -> String {
    let mut parts: Vec<String> = request
        .elements
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    parts.sort();
    format!(
        "{}|{}|{}|{}",
        request.tx_code,
        request.amount_cents,
        request.offline_capable,
        parts.join(",")
    )
}

/// 校验只拦"明显不该发给主机"的请求，不做业务规则（业务规则在主机侧）。
fn validate(request: &TxRequest) -> Option<String> {
    if request.tx_code.trim().is_empty() {
        return Some("交易码为空".into());
    }
    if request.idempotency_key.trim().is_empty() {
        return Some("缺少幂等键：无法保证重放不重复入账".into());
    }
    if request.amount_cents < 0 {
        return Some("金额不能为负".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// 可编程主机：按预置脚本依次返回结果，并记录被调用次数。
    struct ScriptedHost {
        script: RefCell<Vec<Result<String, HostError>>>,
        calls: RefCell<u32>,
    }

    impl ScriptedHost {
        fn new(script: Vec<Result<String, HostError>>) -> Self {
            Self {
                script: RefCell::new(script),
                calls: RefCell::new(0),
            }
        }
        fn always(result: Result<String, HostError>) -> Self {
            Self::new(vec![result])
        }
        fn calls(&self) -> u32 {
            *self.calls.borrow()
        }
    }

    impl TxHost for ScriptedHost {
        fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
            *self.calls.borrow_mut() += 1;
            let mut script = self.script.borrow_mut();
            if script.len() > 1 {
                script.remove(0)
            } else {
                script
                    .first()
                    .cloned()
                    .unwrap_or(Err(HostError::Unavailable))
            }
        }
    }

    /// 一旦被调用就 panic 的宿主：用于证明重复提交**根本没有**碰主机。
    /// 比"调用次数 == 1"更严格——计数被错误重置或漏加时，它仍然抓得住。
    struct PanicHost;

    impl TxHost for PanicHost {
        fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
            panic!("重复提交不得再调用主机");
        }
    }

    fn req(key: &str) -> TxRequest {
        TxRequest::new("account.deposit", key).amount(10_000)
    }

    // S-01
    #[test]
    fn submit_success_is_committed_with_reference() {
        let host = ScriptedHost::always(Ok("TX20261124001".into()));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-1"));
        assert_eq!(result.status, TxStatus::Committed);
        assert_eq!(result.reference.as_deref(), Some("TX20261124001"));
        assert!(!result.replayable);
        assert_eq!(host.calls(), 1);
    }

    // S-02 的核心断言：主机只被调用一次
    #[test]
    fn duplicate_key_returns_first_result_without_calling_host_again() {
        let host = ScriptedHost::always(Ok("TX-1".into()));
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-dup"));
        let second = engine.submit(&host, req("k-dup"));

        assert_eq!(first.reference, second.reference);
        assert_eq!(second.status, TxStatus::Committed);
        assert_eq!(host.calls(), 1, "重复提交不得再次调用主机");
        assert!(second.message.contains("重复提交"));
    }

    // ---- P0-10 第 3 步：同一笔"在途"去重（并发窗口）----
    //
    // 幂等索引记的是"结论"，覆盖不到"第一笔还没回来"的窗口；`submitting`
    // 记的是"正在飞"，两者互补。

    #[test]
    fn second_begin_while_first_in_flight_is_refused() {
        let mut engine = TxEngine::new();
        let request = req("k-fly");

        assert!(
            matches!(engine.begin_submit(&request), SubmitStart::Proceed { .. }),
            "首笔必须放行"
        );
        match engine.begin_submit(&request) {
            SubmitStart::Proceed { .. } => panic!("同一笔不应被放行两次"),
            SubmitStart::ShortCircuited(result) => {
                assert_eq!(result.status, TxStatus::Rejected);
                assert!(
                    result.message.contains("正在处理中"),
                    "文案要让人看懂是「别点第二下」：{}",
                    result.message
                );
            }
        }
    }

    #[test]
    fn finish_releases_in_flight_so_the_same_key_can_retry() {
        let mut engine = TxEngine::new();
        let request = req("k-retry");
        let signature = match engine.begin_submit(&request) {
            SubmitStart::Proceed { signature } => signature,
            SubmitStart::ShortCircuited(result) => panic!("首笔应放行：{}", result.message),
        };

        // 状态不明（超时且不支持离线）：不写幂等索引，必须允许按原键重试。
        let first = engine.finish_submit(&request, &signature, Err(HostError::Timeout));
        assert_eq!(first.status, TxStatus::MarkedForReversal);
        assert!(
            matches!(engine.begin_submit(&request), SubmitStart::Proceed { .. }),
            "在途位必须释放，否则原键永远无法重试"
        );
    }

    #[test]
    fn different_keys_may_be_in_flight_together() {
        let mut engine = TxEngine::new();
        assert!(matches!(
            engine.begin_submit(&req("k-a")),
            SubmitStart::Proceed { .. }
        ));
        assert!(
            matches!(
                engine.begin_submit(&req("k-b")),
                SubmitStart::Proceed { .. }
            ),
            "不同业务不该互相挡路"
        );
    }

    #[test]
    fn same_key_with_different_payload_in_flight_is_refused() {
        let mut engine = TxEngine::new();
        let first = req("k-x");
        let mut second = req("k-x");
        second.amount_cents = 20_000;

        assert!(matches!(
            engine.begin_submit(&first),
            SubmitStart::Proceed { .. }
        ));
        match engine.begin_submit(&second) {
            SubmitStart::Proceed { .. } => panic!("同一键换要素必须拒绝"),
            SubmitStart::ShortCircuited(result) => assert!(
                result.message.contains("另一笔交易"),
                "错误文案要指向幂等键被挪用：{}",
                result.message
            ),
        }
    }

    // S-03
    #[test]
    fn offline_without_capability_is_marked_for_reversal_not_rejected() {
        let host = ScriptedHost::always(Err(HostError::Unavailable));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-off"));

        // 超时/不可达 = 状态不明，绝不能报"拒绝"——那会让柜员重做一笔
        assert_eq!(result.status, TxStatus::MarkedForReversal);
        assert!(!result.replayable);
        assert!(result.message.contains("状态不明"));
        assert!(result.message.contains("待冲正"));
        assert!(
            engine.pending_requests().is_empty(),
            "不允许离线的交易不得入队"
        );
    }

    // S-04
    #[test]
    fn offline_with_capability_is_pending_and_queued() {
        let host = ScriptedHost::always(Err(HostError::Timeout));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-pend").offline(true));

        assert_eq!(result.status, TxStatus::Pending);
        assert!(result.replayable);
        assert_eq!(engine.pending_requests().len(), 1);
        assert_eq!(engine.pending_requests()[0].idempotency_key, "k-pend");
    }

    // S-05
    #[test]
    fn business_rejection_is_not_replayable() {
        let host = ScriptedHost::always(Err(HostError::Rejected("余额不足".into())));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-biz"));

        assert_eq!(result.status, TxStatus::Rejected);
        assert!(!result.replayable);
        assert!(result.message.contains("余额不足"));
        assert!(engine.pending_requests().is_empty());
    }

    #[test]
    fn replay_succeeds_and_clears_queue() {
        let host = ScriptedHost::new(vec![Err(HostError::Unavailable), Ok("TX-REPLAY-1".into())]);
        let mut engine = TxEngine::new();
        engine.submit(&host, req("k-r").offline(true));
        assert_eq!(engine.pending_requests().len(), 1);

        let outcomes = engine.replay_pending(&host);
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].1.status, TxStatus::Committed);
        assert!(engine.pending_requests().is_empty());
        assert_eq!(host.calls(), 2);
    }

    #[test]
    fn replay_keeps_request_when_host_still_unreachable() {
        let host = ScriptedHost::always(Err(HostError::Timeout));
        let mut engine = TxEngine::new();
        engine.submit(&host, req("k-r2").offline(true));

        let outcomes = engine.replay_pending(&host);
        assert_eq!(outcomes[0].1.status, TxStatus::Pending);
        assert_eq!(engine.pending_requests().len(), 1, "不可达时不得丢单");
    }

    #[test]
    fn validation_rejects_missing_idempotency_key() {
        let host = ScriptedHost::always(Ok("TX".into()));
        let mut engine = TxEngine::new();
        let mut request = req("  ");
        request.idempotency_key = "  ".into();

        let result = engine.submit(&host, request);
        assert_eq!(result.status, TxStatus::Rejected);
        assert!(result.message.contains("幂等键"));
        assert_eq!(host.calls(), 0, "校验失败不得打主机");
    }

    #[test]
    fn negative_amount_is_rejected_before_host() {
        let host = ScriptedHost::always(Ok("TX".into()));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-neg").amount(-1));
        assert_eq!(result.status, TxStatus::Rejected);
        assert_eq!(host.calls(), 0);
    }

    // ------------------------------------------------------------------
    // S-02 对抗用例：同一幂等键在"成功 / 业务拒绝 / 断网挂起"三种首次结果下
    // 重复提交。每条都断言主机调用次数，而不是只看返回值。
    // ------------------------------------------------------------------

    // S-02 对抗：首次成功后重复提交。用 PanicHost 证明主机一次都没再被碰，
    // 同时检查重复读取不会把"重复提交"前缀叠加回缓存结果。
    #[test]
    fn s02_duplicate_after_success_returns_first_result_without_touching_host() {
        let host = ScriptedHost::always(Ok("TX-FIRST".into()));
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-dup-ok"));
        assert_eq!(first.reference.as_deref(), Some("TX-FIRST"));

        let second = engine.submit(&PanicHost, req("k-dup-ok"));
        let third = engine.submit(&PanicHost, req("k-dup-ok"));

        assert_eq!(second.status, TxStatus::Committed);
        assert_eq!(
            second.reference, first.reference,
            "重复提交必须返回首次的 reference"
        );
        assert!(!second.replayable, "已成功的交易不可重放");
        assert!(second.message.contains("重复提交"));
        assert_eq!(
            second.message, third.message,
            "重复读取不得污染缓存：前缀不能每次叠加"
        );
        assert_eq!(host.calls(), 1, "主机只被调用一次");
        assert!(
            engine.pending_requests().is_empty(),
            "成功交易不得进入待处理队列"
        );
    }

    // S-02 对抗：首次是业务拒绝。幂等索引里存的是"拒绝"，再次提交不得变成
    // 一次新的主机调用（本脚本里主机第二次明明会成功）——否则同一笔业务
    // 会先后产生两个互相矛盾的结论。
    #[test]
    fn s02_duplicate_after_business_rejection_returns_first_rejection() {
        let host = ScriptedHost::new(vec![
            Err(HostError::Rejected("余额不足".into())),
            Ok("TX-WOULD-SUCCEED".into()),
        ]);
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-dup-biz"));
        let second = engine.submit(&host, req("k-dup-biz"));

        assert_eq!(first.status, TxStatus::Rejected);
        assert_eq!(second.status, TxStatus::Rejected);
        assert_eq!(second.reference, None);
        assert!(!second.replayable);
        assert!(second.message.contains("重复提交"));
        assert!(
            second.message.contains("余额不足"),
            "重复提交应带回首次拒绝原因"
        );
        assert_eq!(host.calls(), 1, "业务拒绝的幂等键不得再次打主机");
        assert!(
            engine.pending_requests().is_empty(),
            "业务拒绝不得进入待处理队列"
        );
    }

    // S-02 对抗：首次是断网挂起。重复提交不得再次入队——否则重放时同一笔
    // 会向主机提交两次，恰恰制造"重复入账"。
    #[test]
    fn s02_duplicate_while_pending_does_not_enqueue_twice() {
        let host = ScriptedHost::always(Err(HostError::Timeout));
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-dup-pend").offline(true));
        let second = engine.submit(&host, req("k-dup-pend").offline(true));
        let third = engine.submit(&host, req("k-dup-pend").offline(true));

        for result in [&first, &second, &third] {
            assert_eq!(result.status, TxStatus::Pending);
            assert!(result.replayable, "挂起结果可重放");
            assert_eq!(result.reference, None);
        }
        assert_eq!(host.calls(), 1, "挂起后的重复提交不得再打主机");
        assert_eq!(
            engine.pending_requests().len(),
            1,
            "重复提交不得重复入队（否则重放会重复入账）"
        );
        assert_eq!(engine.pending_requests()[0].idempotency_key, "k-dup-pend");
    }

    // S-02 对抗：先挂起、再重放成功，之后用同一键重复提交，必须返回"已入账"。
    // 防止重放成功时没更新幂等索引，让柜员一直看到过期的 Pending。
    #[test]
    fn s02_duplicate_after_successful_replay_returns_committed() {
        let host = ScriptedHost::new(vec![Err(HostError::Unavailable), Ok("TX-REPLAY-A".into())]);
        let mut engine = TxEngine::new();

        let pending = engine.submit(&host, req("k-dup-replay").offline(true));
        assert_eq!(pending.status, TxStatus::Pending);

        let outcomes = engine.replay_pending(&host);
        assert_eq!(outcomes[0].1.status, TxStatus::Committed);
        assert_eq!(host.calls(), 2, "提交一次 + 重放一次");

        let duplicate = engine.submit(&host, req("k-dup-replay").offline(true));
        assert_eq!(duplicate.status, TxStatus::Committed);
        assert_eq!(duplicate.reference.as_deref(), Some("TX-REPLAY-A"));
        assert!(!duplicate.replayable);
        assert_eq!(host.calls(), 2, "重放成功后的重复提交不得再打主机");
        assert!(engine.pending_requests().is_empty(), "重放成功后队列应为空");
    }

    // S-02 对抗：挂起 → 重放失败（仍在队列）→ 再次重复提交 → 再重放成功。
    // 重放失败也会改写幂等索引，重复提交必须返回 Pending 且不得二次入队；
    // 而重放会把同一个 key 再发给主机——客户端去重挡不住这条路径，
    // 主机必须按 key 幂等（风险见 T-02 回报）。
    #[test]
    fn s02_duplicate_after_failed_replay_keeps_single_queue_entry() {
        let host = ScriptedHost::new(vec![
            Err(HostError::Timeout),
            Err(HostError::Unavailable),
            Ok("TX-REPLAY-R".into()),
        ]);
        let mut engine = TxEngine::new();

        let pending = engine.submit(&host, req("k-dup-retry").offline(true));
        assert_eq!(pending.status, TxStatus::Pending);

        let first_replay = engine.replay_pending(&host);
        assert_eq!(first_replay[0].1.status, TxStatus::Pending);
        assert_eq!(host.calls(), 2, "提交一次 + 重放失败一次");
        assert_eq!(
            engine.pending_requests().len(),
            1,
            "重放失败必须保留在队列中"
        );

        let duplicate = engine.submit(&host, req("k-dup-retry").offline(true));
        assert_eq!(duplicate.status, TxStatus::Pending);
        assert!(duplicate.replayable);
        assert!(duplicate.message.contains("重复提交"));
        assert_eq!(host.calls(), 2, "重复提交不得再打主机");
        assert_eq!(engine.pending_requests().len(), 1, "重复提交不得二次入队");

        let second_replay = engine.replay_pending(&host);
        assert_eq!(second_replay[0].1.status, TxStatus::Committed);
        assert_eq!(second_replay[0].1.reference.as_deref(), Some("TX-REPLAY-R"));
        assert_eq!(
            host.calls(),
            3,
            "重放把同一个 key 再次发给主机：依赖主机侧按 key 去重"
        );
        assert!(engine.pending_requests().is_empty());

        let after = engine.submit(&host, req("k-dup-retry").offline(true));
        assert_eq!(after.status, TxStatus::Committed);
        assert_eq!(host.calls(), 3, "成功后重复提交仍不碰主机");
    }

    // S-02 对抗 / 边界：首次是"断网且不支持离线"的拒绝，网络恢复后用同一键重试。
    // 现状把这次拒绝缓存进幂等索引，所以同一键永远不会再打主机——尽管首次文案
    // 让柜员"等网络恢复后重试"。本用例只锁定安全性质（主机只调一次、不入队），
    // 该文案与缓存行为的张力见 T-02 回报，是否改成"用新键重试"由人决定。
    #[test]
    fn s06_transient_rejection_is_not_cached_so_retry_reaches_host() {
        let host = ScriptedHost::new(vec![Err(HostError::Unavailable), Ok("TX-RECOVERED".into())]);
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-dup-off"));
        assert_eq!(first.status, TxStatus::MarkedForReversal);
        assert!(first.message.contains("待冲正"));

        // 网络恢复后，同一幂等键必须能真正重试——不能被"首次拒绝"永久缓存挡住。
        // 若这里返回缓存的 Rejected，柜员就只能换新键重试，那才可能重复扣账。
        let second = engine.submit(&host, req("k-dup-off"));
        assert_eq!(second.status, TxStatus::Committed);
        assert_eq!(second.reference.as_deref(), Some("TX-RECOVERED"));
        assert_eq!(host.calls(), 2, "瞬时失败不得缓存，重试必须打到主机");
    }

    // S-02 边界：校验失败不得占用幂等键。首次因金额为负被本地拒绝后，
    // 用同一键提交合法请求必须真的打到主机并成功；否则一次手误就会把
    // 整个幂等键永久污染成"拒绝"。
    #[test]
    fn s02_validation_failure_does_not_burn_the_idempotency_key() {
        let host = ScriptedHost::always(Ok("TX-OK".into()));
        let mut engine = TxEngine::new();

        let invalid = engine.submit(&host, req("k-burn").amount(-1));
        assert_eq!(invalid.status, TxStatus::Rejected);
        assert_eq!(host.calls(), 0, "校验失败不得打主机");

        let valid = engine.submit(&host, req("k-burn"));
        assert_eq!(valid.status, TxStatus::Committed);
        assert_eq!(valid.reference.as_deref(), Some("TX-OK"));
        assert_eq!(host.calls(), 1, "校验失败不应把幂等键写死为拒绝");
    }

    // S-02 对抗（现状锁定，非规格背书）：同一幂等键携带不同交易要素重复提交。
    // 引擎只按 key 去重，不比对 tx_code / amount / elements，返回第一笔的 reference。
    // spec 未定义此情形；若产品要求"键复用但要素不同必须显式冲突"，应先改实现，
    // 再反转本用例——不允许为了变绿直接改断言。
    #[test]
    fn s07_reusing_key_with_different_payload_is_rejected() {
        let host = ScriptedHost::new(vec![Ok("TX-A".into()), Ok("TX-B".into())]);
        let mut engine = TxEngine::new();

        let first = engine.submit(&host, req("k-reuse").amount(10_000).element("to", "A"));
        assert_eq!(first.reference.as_deref(), Some("TX-A"));

        // 同一个键换了金额和交易码：这不是"重复提交"，是键被挪作他用。
        // 静默返回第一笔的成功会让柜员把另一笔业务误认为已入账。
        let changed = TxRequest::new("account.withdraw", "k-reuse")
            .amount(999_999)
            .element("to", "B");
        let second = engine.submit(&host, changed);

        assert_eq!(second.status, TxStatus::Rejected);
        assert!(second.message.contains("幂等键已用于另一笔交易"));
        assert!(second.reference.is_none(), "不得把第一笔的流水号给出去");
        assert_eq!(host.calls(), 1, "载荷冲突在本地就能判，不必打主机");
        assert!(engine.pending_requests().is_empty());
    }

    // S-02 边界：去重必须按 key 隔离。不同幂等键不得互相吞掉，
    // 否则一笔新交易会拿到另一笔的 reference。
    #[test]
    fn s02_distinct_idempotency_keys_are_not_deduped_together() {
        let host = ScriptedHost::always(Err(HostError::Timeout));
        let mut engine = TxEngine::new();

        let a = engine.submit(&host, req("k-a").offline(true));
        let b = engine.submit(&host, req("k-b").offline(true));

        assert_eq!(a.status, TxStatus::Pending);
        assert_eq!(b.status, TxStatus::Pending);
        assert_eq!(host.calls(), 2, "不同幂等键各打一次主机");
        let keys: Vec<&str> = engine
            .pending_requests()
            .iter()
            .map(|request| request.idempotency_key.as_str())
            .collect();
        assert_eq!(keys, vec!["k-a", "k-b"], "两笔都应入队，顺序为提交顺序");
    }

    /// 写前意图落盘失败时，必须拒绝提交且绝不碰主机。
    struct FailingIntentJournal;

    impl TxJournal for FailingIntentJournal {
        fn record(&self, _request: &TxRequest, _result: &TxResult) -> Result<(), String> {
            Ok(())
        }
        fn record_intent(&self, _request: &TxRequest) -> Result<(), String> {
            Err("磁盘已满".to_string())
        }
        fn enqueue(&self, _request: &TxRequest) -> Result<(), String> {
            Ok(())
        }
        fn dequeue(&self, _key: &str) -> Result<(), String> {
            Ok(())
        }
        fn pending(&self) -> Result<Vec<TxRequest>, String> {
            Ok(Vec::new())
        }
    }

    // S-08：写前意图是提交的前置条件，不是事后补记。
    #[test]
    fn s08_failed_intent_write_refuses_submit_without_calling_host() {
        let host = ScriptedHost::always(Ok("TX-1".to_string()));
        let mut engine = TxEngine::new().with_journal(Box::new(FailingIntentJournal));

        let result = engine.submit(&host, req("k-intent"));

        assert_eq!(result.status, TxStatus::Rejected);
        assert!(result.message.contains("写前意图落盘失败"));
        assert_eq!(host.calls(), 0, "落盘失败时不得向主机发出任何请求");
    }
    /// 能活过引擎重建的幂等索引，用来验证重启后不会二次打到主机。
    struct SharedIdempotentJournal(
        std::sync::Arc<std::sync::Mutex<Vec<(String, String, TxResult)>>>,
    );

    impl TxJournal for SharedIdempotentJournal {
        fn record(&self, _request: &TxRequest, _result: &TxResult) -> Result<(), String> {
            Ok(())
        }
        fn load_idempotent(&self) -> Result<Vec<(String, String, TxResult)>, String> {
            Ok(self.0.lock().map_err(|e| e.to_string())?.clone())
        }
        fn save_idempotent(
            &self,
            key: &str,
            signature: &str,
            result: &TxResult,
        ) -> Result<(), String> {
            let mut rows = self.0.lock().map_err(|e| e.to_string())?;
            rows.retain(|(k, _, _)| k != key);
            rows.push((key.to_string(), signature.to_string(), result.clone()));
            Ok(())
        }
        fn enqueue(&self, _request: &TxRequest) -> Result<(), String> {
            Ok(())
        }
        fn dequeue(&self, _key: &str) -> Result<(), String> {
            Ok(())
        }
        fn pending(&self) -> Result<Vec<TxRequest>, String> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn idempotent_index_survives_engine_restart() {
        use std::sync::{Arc, Mutex};

        let store = Arc::new(Mutex::new(Vec::new()));
        let host = ScriptedHost::always(Ok("TX-1".to_string()));
        let mut first =
            TxEngine::new().with_journal(Box::new(SharedIdempotentJournal(Arc::clone(&store))));
        let result = first.submit(&host, req("k-restart"));
        assert_eq!(result.status, TxStatus::Committed);
        assert_eq!(host.calls(), 1);

        let mut restarted =
            TxEngine::new().with_journal(Box::new(SharedIdempotentJournal(Arc::clone(&store))));
        let repeated = restarted.submit(&host, req("k-restart"));
        assert_eq!(repeated.status, TxStatus::Committed);
        assert_eq!(host.calls(), 1, "重启后同键不得再打到主机");
    }
}

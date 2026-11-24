//! 交易引擎（P1 第一片）：状态机 + 幂等 + 离线边界。
//!
//! 这一层刻意不碰 IO：主机是 trait，落盘与重放在后续切片里接。
//! 原因很实际——交易状态机是柜面最容易出错的地方，它必须能脱离网络、
//! 数据库和设备，用纯内存用例把每条分支跑一遍。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// 一笔交易对柜员而言只有三种可见状态。
///
/// 刻意不做 `Unknown`：状态不明是最坏的情况，客户端不该有"我不知道"这种终态，
/// 要么挂起可重放（Pending），要么拒绝（Rejected）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TxStatus {
    /// 主机已受理，拿到流水号。
    Committed,
    /// 网络不可用且该交易允许离线：已挂起，等待重放。
    Pending,
    /// 明确失败：业务拒绝，或不允许离线的交易遇到断网。
    Rejected,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// 超时 / 网络不可达：不知道主机是否处理了。
    Timeout,
    /// 明确不可达（连接被拒）。
    Unavailable,
    /// 主机已受理并明确拒绝（如余额不足）。
    Rejected(String),
}

/// 主机接口。真实实现走网络，测试实现可以精确控制失败方式。
pub trait TxHost {
    fn submit(&self, request: &TxRequest) -> Result<String, HostError>;
}

/// 交易引擎。内存态：幂等索引 + 待重放队列。
#[derive(Debug, Default)]
pub struct TxEngine {
    /// 幂等键 -> 首次结果。命中即返回，绝不再调主机。
    idempotent: HashMap<String, TxResult>,
    /// 待重放请求（P1.4 会落盘）。
    pending: Vec<TxRequest>,
}

impl TxEngine {
    pub fn new() -> Self {
        Self::default()
    }

    /// 提交一笔交易。
    ///
    /// 顺序刻意如此：先校验、再查幂等、最后才碰主机。
    /// 把幂等检查放在最前，是因为"重复提交"不该因为主机暂时不可用而变成失败。
    pub fn submit(&mut self, host: &dyn TxHost, request: TxRequest) -> TxResult {
        if let Some(msg) = validate(&request) {
            return TxResult::rejected(&msg);
        }

        if let Some(previous) = self.idempotent.get(&request.idempotency_key) {
            let mut replay = previous.clone();
            replay.message = format!("重复提交：返回首次结果（{}）", previous.message);
            return replay;
        }

        let result = match host.submit(&request) {
            Ok(reference) => TxResult::committed(reference),
            Err(HostError::Rejected(reason)) => TxResult::rejected(&format!("主机拒绝：{reason}")),
            Err(HostError::Timeout) | Err(HostError::Unavailable) => {
                if request.offline_capable {
                    self.pending.push(request.clone());
                    TxResult::pending("网络不可用：交易已挂起，将在恢复后重放")
                } else {
                    TxResult::rejected("网络不可用，且该交易不支持离线：请等网络恢复后重试")
                }
            }
        };

        self.idempotent
            .insert(request.idempotency_key.clone(), result.clone());
        result
    }

    /// 待重放的请求（按提交顺序）。
    pub fn pending_requests(&self) -> &[TxRequest] {
        &self.pending
    }

    /// 重放全部挂起交易。成功的移出队列，失败的保留。
    ///
    /// 返回每笔的结果，便于上层逐笔向柜员展示——批次里失败一笔不代表全批失败。
    pub fn replay_pending(&mut self, host: &dyn TxHost) -> Vec<(String, TxResult)> {
        let drained = std::mem::take(&mut self.pending);
        let mut outcomes = Vec::new();
        for request in drained {
            let key = request.idempotency_key.clone();
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
            self.idempotent.insert(key.clone(), result.clone());
            outcomes.push((key, result));
        }
        outcomes
    }
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

    // S-03
    #[test]
    fn offline_without_capability_is_rejected_and_not_queued() {
        let host = ScriptedHost::always(Err(HostError::Unavailable));
        let mut engine = TxEngine::new();
        let result = engine.submit(&host, req("k-off"));

        assert_eq!(result.status, TxStatus::Rejected);
        assert!(!result.replayable);
        assert!(result.message.contains("不支持离线"));
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
}

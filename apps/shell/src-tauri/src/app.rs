//! L3 应用层：用例编排。
//!
//! 规则（docs/architecture.md §2）：
//! - 只依赖领域（`tellerkit-core`）与[端口](crate::ports)，**不知道** Tauri、不知道 DTO；
//! - 这里写"先做什么、再做什么、失败怎么降级"；
//! - 命令层（`commands.rs`）只负责加锁 → 调这里 → 映射成 IPC DTO。
//!
//! 为什么不用"一个用例一个 struct"：没有第二个实现，结构体只是把函数换了个形状
//! （见 §6 的三问结论）。

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};

use tellerkit_core as sync;
use tellerkit_core::banking::{MenuSource, MenuVersionProbe};
use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::mock::DemoRole;
use tellerkit_core::pkg;
use tellerkit_core::pubkey_to_hex;
use tellerkit_core::reversal::{
    Authorization, Authorizer, ExceptionKind, ReversalError, ReversalRequest,
};
use tellerkit_core::session::{LockReason, SessionGuard, SessionPolicy};
use tellerkit_core::tx::{HostError, TxHost, TxJournal, TxRequest, TxResult, TxStatus};
use tellerkit_core::MenuTree;
use tellerkit_core::MenuView;
use tellerkit_core::{filter_visible, LoadSource};
use tellerkit_journal_sqlite::{AuditEvent, SqliteJournal};

use crate::bootstrap::StartupFatal;
use crate::lock_app_state;
use crate::ports::{Clock, RemoteBanking, RemoteError};
use crate::{AppState, SharedState};

/// 包调用的来源标记：宿主注入到 IPC 的 map 参数里，Rust 校验前移除。
/// 前端在包参数上覆盖写入，包自己给的同名字段不能生效。
pub const PKG_ORIGIN_KEY: &str = "__pkg_origin";

/// `package_module` 的返回视图。源码与能力一起下发，命令层负责编码为字符串。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PackageModuleView {
    source: String,
    origin: String,
    capabilities: pkg::PackageCapabilities,
}

impl PackageModuleView {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn origin(&self) -> &str {
        &self.origin
    }

    pub fn capabilities(&self) -> &pkg::PackageCapabilities {
        &self.capabilities
    }
}

/// 从 IPC map 中提取的来源。只识别 `pkg:<id>`，格式不合法一律拒绝。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCall {
    id: String,
}

impl PackageCall {
    fn take(fields: &mut BTreeMap<String, String>) -> Result<Option<Self>, String> {
        match fields.remove(PKG_ORIGIN_KEY) {
            None => Ok(None),
            Some(origin) => {
                let id = origin.strip_prefix("pkg:").unwrap_or("");
                if id.is_empty() {
                    Err(format!("业务包来源格式非法：{origin}"))
                } else {
                    Ok(Some(Self { id: id.to_string() }))
                }
            }
        }
    }

    fn origin(&self) -> String {
        format!("pkg:{}", self.id)
    }
}

// 只在测试里模拟"审计账本写不进去"，生产代码没有这个开关。
#[cfg(test)]
thread_local! {
    static FAIL_AUDIT_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 启动结果。命令层据此拼 `BootInfo`。
#[derive(Debug, Clone)]
pub struct BootSummary {
    shell_version: String,
    pubkey_hex: String,
    appdata_dir: String,
    offline: bool,
    local_versions: Vec<u64>,
    menu_version: u64,
    source: String,
    menu_nodes: usize,
    pack_bytes: usize,
    load_ms: f64,
    notes: Vec<String>,
}

impl BootSummary {
    pub fn shell_version(&self) -> &str {
        &self.shell_version
    }

    pub fn pubkey_hex(&self) -> &str {
        &self.pubkey_hex
    }

    pub fn appdata_dir(&self) -> &str {
        &self.appdata_dir
    }

    pub fn offline(&self) -> bool {
        self.offline
    }

    pub fn local_versions(&self) -> &[u64] {
        &self.local_versions
    }

    pub fn menu_version(&self) -> u64 {
        self.menu_version
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn menu_nodes(&self) -> usize {
        self.menu_nodes
    }

    pub fn pack_bytes(&self) -> usize {
        self.pack_bytes
    }

    pub fn load_ms(&self) -> f64 {
        self.load_ms
    }

    pub fn notes(&self) -> &[String] {
        &self.notes
    }
}

#[derive(Debug, Clone)]
pub struct LoginOutcome {
    snapshot: tellerkit_core::PermSnapshot,
    menu: Vec<MenuView>,
    filter_ms: f64,
}

impl LoginOutcome {
    pub fn snapshot(&self) -> &tellerkit_core::PermSnapshot {
        &self.snapshot
    }

    pub fn menu(&self) -> &[MenuView] {
        &self.menu
    }

    pub fn filter_ms(&self) -> f64 {
        self.filter_ms
    }
}

#[derive(Debug, Clone)]
pub struct ReloadOutcome {
    menu_version: u64,
    source: String,
    verify_failed: bool,
    note: String,
    menu: Option<Vec<MenuView>>,
}

impl ReloadOutcome {
    pub fn menu_version(&self) -> u64 {
        self.menu_version
    }

    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn verify_failed(&self) -> bool {
        self.verify_failed
    }

    pub fn note(&self) -> &str {
        &self.note
    }

    pub fn menu(&self) -> &Option<Vec<MenuView>> {
        &self.menu
    }
}

/// 会话视图（给界面看的三元组）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    locked: bool,
    reason: Option<String>,
    idle_secs: u64,
}

impl SessionView {
    pub fn locked(&self) -> bool {
        self.locked
    }

    pub fn reason(&self) -> &Option<String> {
        &self.reason
    }

    pub fn idle_secs(&self) -> u64 {
        self.idle_secs
    }
}

/// 会话状态的独立锁。
///
/// `session_tick` 每秒一次，交易却可能持状态锁打主机数秒；共用一把锁时
/// 安全检查会跟着停摆。所以会话单独托管，`session_*` 只碰这把锁。
/// **锁序固定**：`AppState` → `HostCalls` → `SessionSlot`，禁止反向。
pub struct SessionSlot {
    guard: Option<SessionGuard>,
    teller_no: Option<String>,
    clock: Arc<dyn Clock>,
    audit: Option<Arc<SqliteJournal>>,
    /// 启动拒绝的镜像：会话命令不拿 AppState 锁，也要在槽里拦一次。
    startup_fatal: StartupFatal,
}

impl SessionSlot {
    pub fn new(clock: Arc<dyn Clock>, audit: Option<Arc<SqliteJournal>>) -> Self {
        Self {
            guard: None,
            teller_no: None,
            clock,
            audit,
            startup_fatal: StartupFatal::default(),
        }
    }

    pub fn set_startup_fatal(&mut self, fatal: StartupFatal) {
        self.startup_fatal = fatal;
    }

    pub fn check_startup(&self) -> Result<(), String> {
        self.startup_fatal.check()
    }

    #[cfg(test)]
    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    /// 登录成功：开新会话，空闲计时从此刻起算。
    pub fn begin(&mut self, teller_no: &str) {
        self.guard = Some(SessionGuard::new(
            SessionPolicy::default(),
            self.clock.now_secs(),
        ));
        self.teller_no = Some(teller_no.to_string());
    }

    /// 登出：先清身份，避免登出后的审计还记到上一个人头上。
    pub fn end(&mut self) {
        self.guard = None;
        self.teller_no = None;
    }

    /// 只读判定；不写审计、不持状态锁，调用方拿到结果后再决定怎么留痕。
    pub fn denial(&self) -> Option<(&'static str, String)> {
        match &self.guard {
            Some(g) if !g.is_locked() => None,
            None => Some(("尚未登录", "未登录即调用受保护命令".to_string())),
            Some(g) => Some((
                "会话已锁定",
                g.reason()
                    .map(|r| r.label())
                    .unwrap_or("会话已锁定")
                    .to_string(),
            )),
        }
    }

    pub fn view(&self) -> SessionView {
        match &self.guard {
            None => SessionView {
                locked: false,
                reason: None,
                idle_secs: 0,
            },
            Some(guard) => SessionView {
                locked: guard.is_locked(),
                reason: guard.reason().map(|r| r.label().to_string()),
                idle_secs: guard.idle_secs(self.clock.now_secs()),
            },
        }
    }

    pub fn touch(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        if let Some(guard) = self.guard.as_mut() {
            guard.touch(now);
        }
        self.view()
    }

    pub fn tick(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let newly_locked = self.guard.as_mut().and_then(|guard| guard.evaluate(now));
        let view = self.view();
        self.audit_lock(newly_locked);
        view
    }

    pub fn blur(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let newly_locked = self.guard.as_mut().and_then(|guard| guard.on_blur(now));
        let view = self.view();
        self.audit_lock(newly_locked);
        view
    }

    /// 锁屏事件留痕。锁中毒/账本坏都不能阻止会话进入锁定，所以只告警。
    fn audit_lock(&self, reason: Option<LockReason>) {
        let Some(reason) = reason else {
            return;
        };
        let Some(journal) = self.audit.as_ref() else {
            return;
        };
        let event = AuditEvent::new(
            &self.teller_no.clone().unwrap_or_else(|| "unknown".into()),
            "session_lock",
            "",
            "ok",
            reason.label(),
        );
        if let Err(e) = journal.audit(&event) {
            eprintln!("会话锁定审计写入失败：{e}");
        }
    }
}

/// 会话锁中毒时按**已锁定**处理，绝不因为读不到状态就放行资金操作。
fn poisoned_session_view() -> SessionView {
    SessionView {
        locked: true,
        reason: Some("会话状态异常".into()),
        idle_secs: 0,
    }
}

pub type SharedHostCalls = Arc<Mutex<HostCalls>>;

/// 主机调用的单飞表 + 已定论结果的短缓存。
///
/// 幂等索引记的是"主机回过之后的结论"，覆盖不到"第一笔还在飞"的窗口；
/// 放锁以后这个窗口才真正打开，所以在这里按业务键占位。
#[derive(Default)]
pub struct HostCalls {
    active: HashSet<String>,
    active_signatures: HashMap<String, String>,
    settled: HashMap<String, (String, TxResult)>,
}

/// 在途位的 RAII 租约：异常返回或 panic 都会释放，不会把幂等键永久占住。
pub struct HostLease {
    calls: SharedHostCalls,
    slot: String,
}

impl Drop for HostLease {
    fn drop(&mut self) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.active.remove(&self.slot);
            calls.active_signatures.remove(&self.slot);
        }
    }
}

impl HostCalls {
    /// 原子地做三件事：查已定论结论、查同键在途、占住本次在途位。
    /// 命中已定论结论时返回缓存结果且不占位（对应 core 幂等索引的短路）。
    pub fn claim(
        calls: &SharedHostCalls,
        slot: &str,
        signature: &str,
        collision: &str,
        busy: &str,
    ) -> Result<HostClaim, String> {
        let mut guard = calls
            .lock()
            .map_err(|_| "主机调用表已损坏，请重启客户端".to_string())?;
        if let Some((known, result)) = guard.settled.get(slot) {
            if known != signature {
                return Err(collision.to_string());
            }
            return Ok(HostClaim {
                cached: Some(result.clone()),
                lease: None,
            });
        }
        if !signature.is_empty() {
            if let Some(known) = guard.active_signatures.get(slot) {
                if known != signature {
                    return Err(collision.to_string());
                }
            }
        }
        if !guard.active.insert(slot.to_string()) {
            return Err(busy.to_string());
        }
        guard
            .active_signatures
            .insert(slot.to_string(), signature.to_string());
        Ok(HostClaim {
            cached: None,
            lease: Some(HostLease {
                calls: Arc::clone(calls),
                slot: slot.to_string(),
            }),
        })
    }

    /// 主机给出终态结论后放进短缓存，下一笔同键不必再打主机。
    pub fn settle(&mut self, slot: &str, signature: &str, result: &TxResult) {
        if matches!(result.status(), TxStatus::Committed | TxStatus::Rejected) {
            self.settled
                .insert(slot.to_string(), (signature.to_string(), result.clone()));
        }
    }

    pub fn hydrate(&mut self, rows: Vec<(String, String, TxResult)>) {
        for (key, signature, result) in rows {
            self.settled.insert(key, (signature, result));
        }
    }
}

/// 单次主机调用的准入结果：要么直接返回缓存，要么拿一张在途租约。
pub struct HostClaim {
    pub cached: Option<TxResult>,
    pub lease: Option<HostLease>,
}

/// 单次重放的准入凭据：到期请求快照 + 在途租约，不含 `AppState` 锁。
pub struct ReplayPlan {
    remote: Arc<dyn RemoteBanking>,
    requests: Vec<TxRequest>,
    now: u64,
    _lease: HostLease,
}

impl ReplayPlan {
    /// 阶段二：按到期顺序放锁打主机。
    pub fn call_host(&self) -> Vec<(String, Result<String, HostError>)> {
        self.requests
            .iter()
            .map(|request| {
                (
                    request.idempotency_key().to_string(),
                    self.remote.submit(request),
                )
            })
            .collect()
    }
}

/// 把放锁期间的响应按幂等键回放给 core；未预取的键按"仍不可达"处理，
/// 绝不会凭空造出一个成功结论。
struct ReplayedHost {
    outcomes: HashMap<String, Result<String, HostError>>,
}

impl TxHost for ReplayedHost {
    fn submit(&self, request: &TxRequest) -> Result<String, HostError> {
        self.outcomes
            .get(request.idempotency_key())
            .cloned()
            .unwrap_or(Err(HostError::Unavailable))
    }
}

/// 单次提交的准入凭据：能力校验与审计 intent 已完成，主机尚未调用。
pub struct TxSubmitPlan {
    remote: Arc<dyn RemoteBanking>,
    request: TxRequest,
    key: String,
    actor: String,
    target: String,
    code: String,
    signature: String,
    shortcut: Option<TxResult>,
    lease: Option<HostLease>,
}

impl TxSubmitPlan {
    /// 缓存/挂起重复时不打主机。
    pub fn needs_host(&self) -> bool {
        self.shortcut.is_none()
    }

    /// 阶段二：放锁后真正打主机。
    pub fn call_host(&self) -> Result<String, HostError> {
        self.remote.submit(&self.request)
    }
}

/// 只回放已取回的响应：core 的判定/落账逻辑原封不动，网络不再发生在锁内。
struct FixedHost {
    outcome: Result<String, HostError>,
}

impl TxHost for FixedHost {
    fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
        self.outcome.clone()
    }
}

/// 与 core 的规范化载荷签名同构：同键换交易码/金额/要素必须被认出来。
fn payload_signature(request: &TxRequest) -> String {
    let canonical = serde_json::to_string(&(
        request.tx_code(),
        request.amount_cents(),
        request.offline_capable(),
        request.elements(),
    ))
    .expect("字符串、整数、布尔与字符串映射的序列化不会失败");
    format!("v2|{canonical}")
}

/// 单次冲正的准入凭据：授权已完成、审计 intent 已写、主机尚未调用。
pub struct ReversalPlan {
    remote: Arc<dyn RemoteBanking>,
    key: String,
    reason: String,
    actor: String,
    audit_action: String,
    is_retry: bool,
    _lease: HostLease,
}

impl ReversalPlan {
    /// 阶段二：放锁后打主机。
    pub fn call_host(&self) -> Result<String, RemoteError> {
        self.remote.reverse(&self.key, &self.reason)
    }
}

/// 单次版本探测的准入凭据：持有主机句柄与在途租约，不持有 `AppState`。
pub struct UpdatePlan {
    local: Option<u64>,
    remote: Arc<dyn RemoteBanking>,
    _lease: HostLease,
}

impl UpdatePlan {
    /// 阶段二：放锁后真正打主机。只在可能更新时下载，保持原网络次数。
    pub fn prefetch(&self) -> MenuPrefetch {
        let probe = self.remote.probe_menu();
        let download = match &probe {
            Ok(p) if self.local.map_or(true, |v| p.latest() > v) => self.remote.download_menu(),
            _ => Ok(Vec::new()),
        };
        MenuPrefetch { probe, download }
    }
}

/// 放锁期间取回的远端响应，交给 core 原逻辑判版本/安装。
pub struct MenuPrefetch {
    probe: Result<MenuVersionProbe, RemoteError>,
    download: Result<Vec<u8>, RemoteError>,
}

struct PreparedMenuSource {
    prefetch: MenuPrefetch,
}

impl MenuSource for PreparedMenuSource {
    fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError> {
        self.prefetch.probe.clone()
    }

    fn download_menu(&self) -> Result<Vec<u8>, RemoteError> {
        self.prefetch.download.clone()
    }
}

/// 对账视图：聚合 + 最近明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    /// 统计起点（当日 00:00 的秒级字符串）
    since: String,
    summary: Vec<StatusRow>,
    recent: Vec<RecentTxView>,
}

impl Reconciliation {
    pub fn since(&self) -> &str {
        &self.since
    }

    pub fn summary(&self) -> &[StatusRow] {
        &self.summary
    }

    pub fn recent(&self) -> &[RecentTxView] {
        &self.recent
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    status: String,
    count: i64,
    amount_cents: i64,
}

impl StatusRow {
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn count(&self) -> i64 {
        self.count
    }

    pub fn amount_cents(&self) -> i64 {
        self.amount_cents
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTxView {
    key: String,
    tx_code: String,
    amount_cents: i64,
    status: String,
    reference: Option<String>,
}

impl RecentTxView {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn tx_code(&self) -> &str {
        &self.tx_code
    }

    pub fn amount_cents(&self) -> i64 {
        self.amount_cents
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn reference(&self) -> &Option<String> {
        &self.reference
    }
}

/// 当日 00:00 的秒级时间戳（零填充，与账本 `created_at` 同格式）。
fn start_of_day(now_secs: u64) -> String {
    let day = now_secs / 86_400 * 86_400;
    format!("{day:020}")
}

/// 待处置异常交易的展示结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxExceptionView {
    key: String,
    /// `marked_for_reversal` | `marked_for_roll_forward`
    kind: String,
}

impl TxExceptionView {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn kind(&self) -> &str {
        &self.kind
    }
}

/// 演示用授权端口：固定由"主管 T0101"批准。
///
/// 接 IAM 后替换为真实授权（主管刷卡/指纹），届时 `Authorizer` 的实现会去调远端。
struct DemoSupervisor;

impl Authorizer for DemoSupervisor {
    fn authorize(&self, _request: &ReversalRequest) -> Authorization {
        Authorization::new("T0101", true, "演示：主管授权")
    }
}

/// 用例执行器。借用请求期间的应用状态，并持有端口句柄。
pub struct ShellApp<'a> {
    state: &'a mut AppState,
    remote: Arc<dyn RemoteBanking>,
    clock: Arc<dyn Clock>,
}

impl<'a> ShellApp<'a> {
    pub fn new(state: &'a mut AppState) -> Self {
        let remote = Arc::clone(&state.remote);
        let clock = Arc::clone(&state.clock);
        Self {
            state,
            remote,
            clock,
        }
    }

    // ---------- 会话闸门 ----------

    /// 资金/打印/冲正/重放/业务包入口的统一会话校验。
    ///
    /// 锁屏与未登录的判定**只信 Rust 侧状态**——前端 tick 可以被跳过，
    /// 覆盖层下的 iframe 也能发 postMessage，只有这里是唯一真相。
    /// 判定失败即写审计（拒绝原因），且绝不触达主机。
    fn require_unlocked(&self, action: &str) -> Result<(), String> {
        // 只短暂持有会话锁；审计要取柜员号并写账本，不能持锁做。
        let denial = match self.state.session.lock() {
            Ok(slot) => slot.denial(),
            Err(_) => Some(("会话状态异常", "会话锁已损坏，请重启客户端".to_string())),
        };
        match denial {
            None => Ok(()),
            Some((reason, detail)) => {
                let actor = self.actor();
                let denial = format!("{reason}：{detail}。请回到锁屏界面解锁后再试");
                let audited = self.audit(&actor, action, "", "denied", &detail);
                match audited {
                    Ok(()) => Err(denial),
                    Err(e) => Err(format!("{denial}；审计写入失败：{e}")),
                }
            }
        }
    }

    /// 调用主机之前的审计可写性闸门：没有账本或写不进去就绝不触达主机。
    fn require_auditable(&self, action: &str, target: &str) -> Result<(), String> {
        if self.state.audit.is_none() {
            return Err(format!(
                "审计账本不可用，已拒绝 {action}（未触达主机）：请检查账本文件与磁盘后重启终端"
            ));
        }
        self.audit(
            &self.actor(),
            action,
            target,
            "intent",
            "写前留痕：确认审计链可写后才允许调用主机",
        )
        .map_err(|e| format!("审计写前留痕失败，已拒绝 {action}（未触达主机）：{e}"))
    }

    // ---------- 启动与菜单 ----------

    /// 启动：本地包优先，业务包合并，网络只在本地不可用时兜底。
    pub fn boot(&mut self) -> Result<BootSummary, String> {
        // 启动先查审计链：被删改的账本不能带着"营业正常"的面貌继续用。
        let verified = match self.state.audit.as_ref() {
            Some(journal) => journal.verify_chain(),
            None => Err("审计账本不可用：账本未能打开".to_string()),
        };
        let head = match verified {
            Ok(head) => head,
            Err(e) => {
                let reason = format!("审计链校验失败，已拒绝启动：{e}");
                self.state
                    .set_startup_fatal(StartupFatal(Some(reason.clone())));
                return Err(reason);
            }
        };
        let head_prefix = head.hash().get(..16).unwrap_or(head.hash()).to_string();

        self.state.store.clean_tmp();

        let started = std::time::Instant::now();
        let outcome = sync::boot_load(self.state.remote.as_ref(), &self.state.store)
            .map_err(|e| e.to_string())?;
        let load_ms = started.elapsed().as_secs_f64() * 1000.0;

        let (loaded, mut notes) = outcome.into_parts();
        let pack_bytes = loaded.bytes();
        let source = match loaded.source() {
            LoadSource::LocalFile { .. } => "local".to_string(),
            LoadSource::NetworkFallback { .. } => "network".to_string(),
        };
        let mut tree: MenuTree = loaded.into_tree();
        let menu_version = tree.menu_version();

        // 业务包：扫描 → 验签 → 依赖拓扑 → 合并菜单
        let pkg_dir = std::path::Path::new(&self.state.appdata_dir).join("packages");
        let (plan, failures) = pkg::scan_dir(
            &pkg_dir,
            &self.state.verifying_key,
            crate::ports::shell_version(),
        );
        let conflicts = pkg::merge_packages(&mut tree, plan.ordered());
        self.state.packages = plan.ordered().to_vec();

        for package in &self.state.packages {
            notes.push(format!(
                "已加载业务包 {} v{}",
                package.id(),
                package.version()
            ));
        }
        for failure in &failures {
            notes.push(format!("跳过业务包 {}：{}", failure.id(), failure.reason()));
        }
        for conflict in &conflicts {
            notes.push(format!("业务包菜单冲突，保留主壳版本：{conflict}"));
        }
        if let Some(w) = &self.state.journal_warning {
            notes.push(w.clone());
        }
        notes.push(format!(
            "审计链头部 seq={} hash={}",
            head.seq(),
            head_prefix
        ));

        let menu_nodes = tree.node_count();
        self.state.menu = Some(tree);

        Ok(BootSummary {
            shell_version: env!("CARGO_PKG_VERSION").to_string(),
            pubkey_hex: pubkey_to_hex(&self.state.verifying_key),
            appdata_dir: self.state.appdata_dir.clone(),
            offline: self.mock_offline(),
            local_versions: self.state.store.list_versions(),
            menu_version,
            source,
            menu_nodes,
            pack_bytes,
            load_ms,
            notes,
        })
    }

    /// 登录：成功与失败都留痕；任何一条审计写不进去都向上报错。
    pub fn login(&mut self, role_id: &str) -> Result<LoginOutcome, String> {
        match self.login_attempt(role_id) {
            Ok(outcome) => {
                let snapshot = outcome.snapshot();
                let detail = format!("角色 {}", snapshot.role_label());
                self.audit(
                    snapshot.teller_no(),
                    "login",
                    snapshot.outlet_code(),
                    "ok",
                    &detail,
                )
                .map_err(|e| format!("审计写入失败：{e}；登录已成功，请立即报告管理员并查账"))?;
                Ok(outcome)
            }
            Err(reason) => {
                let detail = format!("登录失败（角色 {role_id}）：{reason}");
                let audited = self.audit("unknown", "login", role_id, "failed", &detail);
                Err(match audited {
                    Ok(()) => reason,
                    Err(e) => format!("{reason}；审计写入失败：{e}"),
                })
            }
        }
    }

    fn login_attempt(&mut self, role_id: &str) -> Result<LoginOutcome, String> {
        let role = DemoRole::from_id(role_id).ok_or("未知角色")?;
        let role_index = DemoRole::ALL.iter().position(|r| *r == role).unwrap_or(0) as u64;
        let perm_version = self.mock_menu_version() * 100 + role_index;

        let snapshot = self
            .remote
            .login(role_id, perm_version)
            .map_err(|e| e.to_string())?;

        let menu_tree = self
            .state
            .menu
            .as_ref()
            .ok_or("菜单尚未加载，请先完成启动流程")?;
        let started = std::time::Instant::now();
        let menu = filter_visible(menu_tree, &snapshot);
        let filter_ms = started.elapsed().as_secs_f64() * 1000.0;

        self.state.snapshot = Some(snapshot.clone());
        self.state
            .session
            .lock()
            .map_err(|_| "会话状态已损坏，请重启客户端".to_string())?
            .begin(snapshot.teller_no());
        Ok(LoginOutcome {
            snapshot,
            menu,
            filter_ms,
        })
    }

    pub fn logout(&mut self) -> Result<(), String> {
        if let Some(snapshot) = self.state.snapshot.clone() {
            self.audit(snapshot.teller_no(), "logout", "", "ok", "")?;
        }
        if let Ok(mut slot) = self.state.session.lock() {
            slot.end();
        }
        self.state.clear_session();
        Ok(())
    }

    pub fn current_menu(&self) -> Result<Vec<MenuView>, String> {
        let snapshot = self.state.snapshot.as_ref().ok_or("尚未登录")?;
        let menu = self.state.menu.as_ref().ok_or("菜单尚未加载")?;
        Ok(filter_visible(menu, snapshot))
    }

    /// 版本探测的阶段一：校验/留痕/占单飞位，绝不在这里打主机。
    pub fn prepare_check_update(&mut self) -> Result<UpdatePlan, String> {
        self.require_auditable("check_update", "menu")?;
        let lease = HostCalls::claim(
            &self.state.host_calls,
            "check_update",
            "menu",
            "版本探测参数冲突",
            "版本探测正在进行中，请稍后重试",
        )?
        .lease
        .ok_or_else(|| "版本探测参数冲突".to_string())?;
        Ok(UpdatePlan {
            local: self.state.store.current_version(),
            remote: Arc::clone(&self.remote),
            _lease: lease,
        })
    }

    /// 阶段三：用放锁期间取回的响应喂给 core 原逻辑，落菜单与结论审计。
    pub fn finish_check_update(
        &mut self,
        plan: UpdatePlan,
        prefetch: MenuPrefetch,
    ) -> Result<sync::UpdateOutcome, String> {
        let outcome = sync::check_and_update(&PreparedMenuSource { prefetch }, &self.state.store);
        if outcome.updated() {
            if let Ok(loaded) = self.state.store.load_latest() {
                self.state.menu = Some(loaded.into_tree());
            }
        }
        self.audit_checked(
            &self.actor(),
            "check_update",
            "menu",
            if outcome.checked() { "ok" } else { "failed" },
            outcome.note(),
        )?;
        drop(plan);
        Ok(outcome)
    }

    /// 演示"本地包被篡改"后的回退：验签失败 → 服务端救回。
    pub fn reload_from_disk(&mut self) -> Result<ReloadOutcome, String> {
        let outcome = sync::reload(self.state.remote.as_ref(), &self.state.store)
            .map_err(|e| e.to_string())?;
        let note = outcome.notes().join("；");
        let verify_failed = outcome.verify_failed();
        let loaded = outcome.into_loaded();
        let menu = self
            .state
            .snapshot
            .as_ref()
            .map(|snap| filter_visible(loaded.tree(), snap));
        let menu_version = loaded.tree().menu_version();
        let source = match loaded.source() {
            LoadSource::LocalFile { .. } => "local".to_string(),
            LoadSource::NetworkFallback { .. } => "network".to_string(),
        };
        self.state.menu = Some(loaded.into_tree());

        Ok(ReloadOutcome {
            menu_version,
            source,
            verify_failed,
            note,
            menu,
        })
    }

    // ---------- 交易 ----------

    /// 状态不明的交易：标记待冲正。这是超时后的**唯一**正确处置。
    fn mark_if_uncertain(&mut self, key: &str, result: &TxResult) {
        if result.status() == TxStatus::MarkedForReversal {
            self.state
                .reversal
                .mark(key, ExceptionKind::MarkedForReversal);
        }
    }

    /// 待处置的异常交易。
    pub fn tx_exceptions(&self) -> Vec<TxExceptionView> {
        self.state
            .reversal
            .outstanding()
            .into_iter()
            .map(|(key, kind)| TxExceptionView {
                key,
                kind: match kind {
                    ExceptionKind::MarkedForReversal => "marked_for_reversal",
                    ExceptionKind::MarkedForRollForward => "marked_for_roll_forward",
                }
                .to_string(),
            })
            .collect()
    }

    /// 冲正阶段一：本地授权、审计 intent、占单飞位；不打主机。
    pub fn prepare_reversal(&mut self, key: &str, reason: &str) -> Result<ReversalPlan, String> {
        self.require_unlocked("reversal")?;
        self.require_auditable("reversal", key)?;
        let actor = self.actor();
        let is_retry = self.state.reversal.awaiting_confirmation_of(key).is_some();
        let audit_action = if is_retry {
            "reversal/retry"
        } else {
            "reversal"
        };

        let claim = match HostCalls::claim(
            &self.state.host_calls,
            &format!("reversal:{key}"),
            "",
            "冲正幂等键冲突",
            "同一笔冲正正在处理中，请勿重复申请",
        ) {
            Ok(claim) => claim,
            Err(e) => {
                let why = e.clone();
                return Err(self.audit_and_error(&actor, audit_action, key, "denied", &why, e));
            }
        };
        let _lease = claim.lease.ok_or_else(|| "冲正幂等键冲突".to_string())?;

        // 1. 首次申请才需要授权；重试沿用同一条凭证与同一个幂等键。
        if !is_retry {
            let request = ReversalRequest::new(key, &actor, "", 0, reason);
            if let Err(e) = self.state.reversal.request(&request, &DemoSupervisor) {
                let result = match &e {
                    ReversalError::NotAuthorized(_) | ReversalError::SelfApproval(_) => "denied",
                    _ => "failed",
                };
                let why = e.to_string();
                let err = self.audit_and_error(
                    &actor,
                    audit_action,
                    key,
                    result,
                    &why,
                    format!("冲正申请未通过：{why}"),
                );
                return Err(err);
            }
        }

        Ok(ReversalPlan {
            remote: Arc::clone(&self.remote),
            key: key.to_string(),
            reason: reason.to_string(),
            actor,
            audit_action: audit_action.to_string(),
            is_retry,
            _lease,
        })
    }

    /// 冲正阶段三：放锁期间主机回了流水号，才在锁内确认与写结论审计。
    pub fn finish_reversal(
        &mut self,
        plan: ReversalPlan,
        host_result: Result<String, RemoteError>,
    ) -> Result<String, String> {
        let ReversalPlan {
            key,
            actor,
            audit_action,
            is_retry,
            ..
        } = &plan;

        // 2. 主机受理。失败时保留"已授权未确认"，柜员仍可重试。
        let host_reference = match host_result {
            Ok(reference) => reference,
            Err(e) => {
                let why = e.to_string();
                if *is_retry {
                    let err = self.audit_and_error(
                        actor,
                        audit_action,
                        key,
                        "failed",
                        &why,
                        format!(
                            "冲正已获授权但主机仍未确认（{why}），原授权保留；请在异常清单中重试，不要另发新冲正"
                        ),
                    );
                    return Err(err);
                }
                return Err(self.audit_and_error(
                    actor,
                    audit_action,
                    key,
                    "failed",
                    &why,
                    format!("主机冲正失败：{why}"),
                ));
            }
        };

        // 3. 主机回了流水号才算处置完成
        if let Err(e) = self.state.reversal.confirm(key, &host_reference) {
            let why = e.to_string();
            return Err(self.audit_and_error(
                actor,
                audit_action,
                key,
                "failed",
                &format!("主机已受理但本地确认失败：{why}"),
                format!("主机已受理冲正（{host_reference}）但本地确认失败：{why}；请立即查账"),
            ));
        }

        self.audit_checked(actor, audit_action, key, "ok", &host_reference)?;
        drop(plan);
        Ok(host_reference)
    }

    /// 提交阶段一：会话/包能力/审计 intent、幂等与单飞校验，全部在锁内完成。
    pub fn prepare_submit_transaction(
        &mut self,
        mut request: TxRequest,
    ) -> Result<TxSubmitPlan, String> {
        self.require_unlocked("submit_tx")?;
        let code = request.tx_code().to_string();
        let call = self.take_call(request.elements_mut(), &code)?;
        if let Some(call) = &call {
            self.authorize_package_call(call, "submit", &code, None)?;
        }
        self.require_auditable("submit_tx", request.idempotency_key())?;
        // 契约 lifecycle.md 不变量 7：主机没声明幂等，就不许离线挂起。
        if request.offline_capable() && !self.remote.supports_idempotency() {
            let actor = self.actor();
            let code = request.tx_code().to_string();
            let err = self.audit_and_error(
                &actor,
                "submit_tx",
                &code,
                "failed",
                "主机未声明幂等，拒绝离线挂起",
                "该主机未声明幂等支持，不能离线挂起交易（请联系管理员确认主机契约）".to_string(),
            );
            return Err(err);
        }

        let key = request.idempotency_key().to_string();
        let actor = self.actor();
        let target = call
            .as_ref()
            .map(PackageCall::origin)
            .unwrap_or_else(|| code.clone());
        // core 的提交校验只拦明显不能发给主机的请求；放锁前先做一遍，
        // 免得非法请求白跑一趟网络。
        let invalid = if code.trim().is_empty() {
            Some("交易码为空".to_string())
        } else if key.trim().is_empty() {
            Some("缺少幂等键：无法保证重放不重复入账".to_string())
        } else if request.amount_cents() < 0 {
            Some("金额不能为负".to_string())
        } else {
            None
        };
        if let Some(msg) = invalid {
            return Ok(TxSubmitPlan {
                remote: Arc::clone(&self.remote),
                request,
                key,
                actor,
                target,
                code,
                signature: String::new(),
                shortcut: Some(TxResult::rejected(&msg)),
                lease: None,
            });
        }
        let signature = payload_signature(&request);
        let collision = "幂等键已用于另一笔交易（交易码或要素不同），请为该笔业务使用新的幂等键";
        let busy = "同一笔交易正在处理中，请勿重复提交；若上一笔超时，请先查账确认再决定是否重试";
        let claim = match HostCalls::claim(
            &self.state.host_calls,
            &format!("submit:{key}"),
            &signature,
            collision,
            busy,
        ) {
            Ok(claim) => claim,
            Err(e) => {
                let why = e.clone();
                return Err(self.audit_and_error(&actor, "submit_tx", &target, "denied", &why, e));
            }
        };
        // 已定论的同键交易不再打主机；core 对重复提交也会补同样的说明。
        let shortcut = if let Some(previous) = claim.cached {
            let message = format!("重复提交：返回首次结果（{}）", previous.message());
            Some(TxResult::restore(
                previous.status(),
                previous.reference().map(str::to_string),
                message,
                previous.replayable(),
            ))
        } else if self.state.tx.pending_keys().contains(&key) {
            Some(TxResult::pending("重复提交：交易已挂起，等待重放"))
        } else {
            None
        };

        Ok(TxSubmitPlan {
            remote: Arc::clone(&self.remote),
            request,
            key,
            actor,
            target,
            code,
            signature,
            shortcut,
            lease: claim.lease,
        })
    }

    /// 提交阶段三：把放锁期间拿到的主机结论交回 core 落账，再写结论审计。
    pub fn finish_submit_transaction(
        &mut self,
        plan: TxSubmitPlan,
        outcome: Option<Result<String, HostError>>,
    ) -> Result<TxResult, String> {
        let amount_cents = plan.request.amount_cents();
        let result = match &plan.shortcut {
            Some(shortcut) => shortcut.clone(),
            None => {
                let host = FixedHost {
                    outcome: outcome.unwrap_or(Err(HostError::Unavailable)),
                };
                // core 仍负责校验/幂等索引/挂起队列/账本收口；只是此刻的主机
                // 是"已取回响应"的假端口，真正网络调用发生在两阶段之间。
                self.state.tx.submit(&host, plan.request.clone())
            }
        };
        if plan.shortcut.is_none() {
            if let Ok(mut calls) = self.state.host_calls.lock() {
                calls.settle(&format!("submit:{}", plan.key), &plan.signature, &result);
            }
        }
        self.mark_if_uncertain(&plan.key, &result);
        let detail = format!(
            "code={} status={:?} amount_cents={amount_cents}",
            plan.code,
            result.status()
        );
        self.audit_checked(
            &plan.actor,
            "submit_tx",
            &plan.target,
            if result.status() == TxStatus::Committed {
                "ok"
            } else {
                "failed"
            },
            &detail,
        )?;
        drop(plan);
        Ok(result)
    }

    /// 重放阶段一：会话闸门、审计 intent、单飞位与到期清单，都在锁内。
    pub fn prepare_replay(&mut self) -> Result<ReplayPlan, String> {
        self.require_unlocked("replay")?;
        self.require_auditable("replay", "tx_queue")?;
        let claim = match HostCalls::claim(
            &self.state.host_calls,
            "replay:all",
            "",
            "",
            "上一轮重放仍在进行中，请稍后重试",
        ) {
            Ok(claim) => claim,
            Err(e) => return Err(e),
        };
        let lease = claim.lease.ok_or_else(|| "重放状态冲突".to_string())?;
        let now = self.clock.now_secs();
        // core 的 run_once 从账本读回挂起请求；这里读同一份账本算出到期键，
        // 才能在放锁期间按原请求打主机。
        let pending = match &self.state.audit {
            Some(journal) => TxJournal::pending(journal.as_ref())
                .map_err(|e| format!("读取待重放队列失败：{e}"))?,
            None => Vec::new(),
        };
        for request in &pending {
            self.state.replay.track(request.idempotency_key(), now);
        }
        let due = self.state.replay.due(now);
        let requests = pending
            .into_iter()
            .filter(|request| due.iter().any(|key| key == request.idempotency_key()))
            .collect();
        Ok(ReplayPlan {
            remote: Arc::clone(&self.remote),
            requests,
            now,
            _lease: lease,
        })
    }

    /// 重放阶段三：把放锁期间的结果喂回 core 调度器，落结论与审计。
    pub fn finish_replay(
        &mut self,
        plan: ReplayPlan,
        outcomes: Vec<(String, Result<String, HostError>)>,
    ) -> Result<String, String> {
        let host = ReplayedHost {
            outcomes: outcomes.into_iter().collect(),
        };
        let run = tellerkit_core::replay::run_once(
            &mut self.state.tx,
            &mut self.state.replay,
            &host,
            plan.now,
        );

        let message = if run.attempted() == 0 && run.still_pending() == 0 {
            "没有待重放的交易".to_string()
        } else if run.attempted() == 0 {
            format!("还有 {} 笔挂起，均未到重试时间", run.still_pending())
        } else {
            let detail = format!(
                "attempted={} committed={} pending={} gave_up={} keys={}",
                run.attempted(),
                run.committed(),
                run.still_pending(),
                run.gave_up().len(),
                run.gave_up().join(",")
            );
            let actor = self.actor();
            // 放弃自动重试是异常终态，必须按 failed 留痕，不能混在 ok 里。
            self.audit_checked(
                &actor,
                "replay",
                "tx_queue",
                if run.gave_up().is_empty() {
                    "ok"
                } else {
                    "failed"
                },
                &detail,
            )?;
            format!(
                "重放 {} 笔：成功 {} 笔，仍挂起 {} 笔{}",
                run.attempted(),
                run.committed(),
                run.still_pending(),
                if run.gave_up().is_empty() {
                    String::new()
                } else {
                    format!("，{} 笔已放弃自动重试，需人工处理", run.gave_up().len())
                }
            )
        };
        drop(plan);
        Ok(message)
    }

    pub fn tx_pending_count(&self) -> usize {
        self.state.tx.pending_count()
    }

    // ---------- 设备 ----------

    pub fn print_voucher(&mut self, mut voucher: Voucher) -> Result<PrintReceipt, String> {
        self.require_unlocked("print_voucher")?;
        let tx_code = voucher.tx_code().to_string();
        let template = voucher.template().to_string();
        let mut fields = voucher.fields().clone();
        let call = self.take_call(&mut fields, &tx_code)?;
        voucher.set_fields(fields);
        if let Some(call) = &call {
            self.authorize_package_call(call, "print", &tx_code, Some(&template))?;
        }
        self.require_auditable("print_voucher", voucher.tx_code())?;
        // 白名单先于动作：未登记的型号连纸都不该动
        self.state
            .devices
            .ensure_allowed(&self.state.policy)
            .map_err(|e| e.to_string())?;
        let outcome = self
            .state
            .devices
            .printer()
            .print(&voucher)
            .map_err(|e| e.to_string());
        let actor = self.actor();
        let detail = format!("code={tx_code} template={template}");
        let target = call
            .as_ref()
            .map(PackageCall::origin)
            .unwrap_or_else(|| tx_code.clone());
        self.audit_checked(
            &actor,
            "print",
            &target,
            if outcome.is_ok() { "ok" } else { "failed" },
            &detail,
        )?;
        outcome
    }

    // ---------- 会话 ----------

    // 这三个方法只是用例视角的薄包装；IPC 入口直接取会话槽，
    // 不经过 AppState 锁（见 commands::session_*）。
    pub fn session_touch(&mut self) -> SessionView {
        match self.state.session.lock() {
            Ok(mut slot) => slot.touch(),
            Err(_) => poisoned_session_view(),
        }
    }

    pub fn session_tick(&mut self) -> SessionView {
        match self.state.session.lock() {
            Ok(mut slot) => slot.tick(),
            Err(_) => poisoned_session_view(),
        }
    }

    pub fn session_blur(&mut self) -> SessionView {
        match self.state.session.lock() {
            Ok(mut slot) => slot.blur(),
            Err(_) => poisoned_session_view(),
        }
    }

    // ---------- 业务包 ----------

    /// 返回包源码 + 签名清单推导出的能力 + 审计用的来源标识。
    /// 前端只按 capabilities 暴露方法，不自行判断业务权限。
    pub fn package_module(&self, route: &str) -> Result<PackageModuleView, String> {
        self.require_unlocked("package_module")?;
        self.require_auditable("package_module", route)?;
        for package in &self.state.packages {
            if let Some(module) = package.route(route) {
                let source = package
                    .asset(&module)
                    .ok_or_else(|| format!("包 {} 的模块 {module} 未随包提供", package.id()))?;
                return Ok(PackageModuleView {
                    source,
                    origin: format!("pkg:{}", package.id()),
                    capabilities: package.capabilities(),
                });
            }
        }
        Err("该路由不是业务包提供的".to_string())
    }

    /// 取出调用来源；格式非法也要留痕，不能静默当成主壳调用。
    fn take_call(
        &self,
        fields: &mut BTreeMap<String, String>,
        code: &str,
    ) -> Result<Option<PackageCall>, String> {
        PackageCall::take(fields).map_err(|err| {
            self.audit_and_error(
                &self.actor(),
                "package_capability",
                "pkg:unknown",
                "denied",
                &format!("code={code} {err}"),
                err,
            )
        })
    }

    /// 所有包调用的真实放行点：包必须已加载验签，且方法/交易码/模板均在清单里。
    fn authorize_package_call(
        &self,
        call: &PackageCall,
        method: &str,
        tx_code: &str,
        template: Option<&str>,
    ) -> Result<(), String> {
        let package = self
            .state
            .packages
            .iter()
            .find(|p| p.id() == call.id)
            .ok_or_else(|| self.deny_package_call(call, tx_code, "来源包未加载或验签未通过"))?;
        let caps = package.capabilities();
        if !caps.allows_method(method) {
            return Err(self.deny_package_call(call, tx_code, &format!("未声明方法 {method}")));
        }
        if !caps.allows_tx_code(tx_code) {
            return Err(self.deny_package_call(call, tx_code, "未声明交易码"));
        }
        if let Some(template) = template {
            if !caps.allows_template(template) {
                return Err(self.deny_package_call(call, template, "未声明模板"));
            }
        }
        Ok(())
    }

    fn deny_package_call(&self, call: &PackageCall, code: &str, reason: &str) -> String {
        self.audit_and_error(
            &self.actor(),
            "package_capability",
            &call.origin(),
            "denied",
            &format!("code={code} {reason}"),
            format!("业务包能力校验未通过：{reason}"),
        )
    }

    // ---------- 审计 ----------

    /// 日终对账：按状态聚合 + 最近明细。
    ///
    /// 只读本地账本，不打主机。真正的对账以核心系统为准，
    /// 这里解决的是"柜员关账前先自查一遍"。
    pub fn reconciliation(&self) -> Result<Reconciliation, String> {
        let Some(journal) = &self.state.audit else {
            return Err("审计日志不可用：账本未能打开".to_string());
        };
        let today = start_of_day(self.clock.now_secs());
        let rows = journal.summary(&today)?;
        let recent = journal.recent(20)?;

        Ok(Reconciliation {
            since: today,
            summary: rows
                .into_iter()
                .map(|r| StatusRow {
                    status: r.status().to_string(),
                    count: r.count(),
                    amount_cents: r.amount_cents(),
                })
                .collect(),
            recent: recent
                .into_iter()
                .map(|r| RecentTxView {
                    key: r.idempotency_key().to_string(),
                    tx_code: r.tx_code().to_string(),
                    amount_cents: r.amount_cents(),
                    status: r.status().to_string(),
                    reference: r.reference().clone(),
                })
                .collect(),
        })
    }

    pub fn audit_tail(&self, limit: usize) -> Result<Vec<String>, String> {
        let Some(journal) = &self.state.audit else {
            return Ok(vec!["审计日志不可用：账本未能打开".into()]);
        };
        let events = journal.audit_tail(limit.min(500))?;
        Ok(events
            .into_iter()
            .map(|e| {
                format!(
                    "{} · {} {} {} · {}{}",
                    e.at(),
                    e.actor(),
                    e.action(),
                    e.target(),
                    e.result(),
                    if e.detail().is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", e.detail())
                    }
                )
            })
            .collect())
    }

    /// 记一条审计。账本不可用或写失败都向上返回，绝不静默放行。
    fn audit(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) -> Result<(), String> {
        #[cfg(test)]
        {
            let remaining = FAIL_AUDIT_CALLS.with(|cell| cell.get());
            if remaining != usize::MAX {
                if remaining == 0 {
                    return Err("模拟审计写入失败".to_string());
                }
                FAIL_AUDIT_CALLS.with(|cell| cell.set(remaining - 1));
            }
        }
        let Some(journal) = self.state.audit.as_deref() else {
            return Err(format!("审计日志不可用：账本未能打开（{action}/{result}）"));
        };
        let event = AuditEvent::new(actor, action, target, result, detail);
        journal
            .audit(&event)
            .map_err(|e| format!("追加审计日志失败（{action}/{result}）：{e}"))
    }

    /// 结论审计：业务已经发生，写不进去必须让用户知道去查账，不能谎报成功。
    fn audit_checked(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) -> Result<(), String> {
        self.audit(actor, action, target, result, detail)
            .map_err(|e| format!("审计结论写入失败：{e}；业务已发生，请立即查账，不要重复提交"))
    }

    /// 业务本来就被拒绝时，把审计失败一起报出来，不能掩盖"没留痕"这件事。
    fn audit_and_error(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
        reason: String,
    ) -> String {
        match self.audit(actor, action, target, result, detail) {
            Ok(()) => reason,
            Err(e) => format!("{reason}；审计写入失败：{e}"),
        }
    }

    /// 测试注入：前 `successes` 次写入照常，之后全部失败。
    #[cfg(test)]
    fn fail_audits_after(successes: usize) {
        FAIL_AUDIT_CALLS.with(|cell| cell.set(successes));
    }

    /// mock 专属观测：接入真实端点后没有"离线开关"这个概念。
    fn mock_offline(&self) -> bool {
        self.state
            .mock
            .as_ref()
            .map(|m| m.offline())
            .unwrap_or(false)
    }

    fn mock_menu_version(&self) -> u64 {
        self.state
            .mock
            .as_ref()
            .map(|m| m.menu_version())
            .unwrap_or(0)
    }

    fn actor(&self) -> String {
        self.state
            .snapshot
            .as_ref()
            .map(|s| s.teller_no().to_string())
            .unwrap_or_else(|| "unknown".into())
    }
}

// ---------------- 三段式编排（放锁打主机） ----------------
//
// 这里刻意不持有 MutexGuard 跨网络：每段单独加锁、统一走 lock_app_state，
// 启动拒绝在任意一段都能拦下。后台 worker 与命令共用同一入口。

/// 交易：锁内准备与审计 intent → 放锁打主机 → 回锁落结论与审计。
pub fn submit_transaction_with(
    shared: &SharedState,
    request: TxRequest,
) -> Result<TxResult, String> {
    let plan = {
        let mut guard = lock_app_state(shared)?;
        ShellApp::new(&mut guard).prepare_submit_transaction(request)?
    };
    let outcome = if plan.needs_host() {
        Some(plan.call_host())
    } else {
        None
    };
    let mut guard = lock_app_state(shared)?;
    ShellApp::new(&mut guard).finish_submit_transaction(plan, outcome)
}

/// 冲正：本地授权与审计 intent 在锁内，主机调用在锁外。
pub fn request_reversal_with(
    shared: &SharedState,
    key: &str,
    reason: &str,
) -> Result<String, String> {
    let plan = {
        let mut guard = lock_app_state(shared)?;
        ShellApp::new(&mut guard).prepare_reversal(key, reason)?
    };
    let outcome = plan.call_host();
    let mut guard = lock_app_state(shared)?;
    ShellApp::new(&mut guard).finish_reversal(plan, outcome)
}

/// 重放：命令与后台 worker 共用；到期清单在锁内取，主机调用在锁外。
pub fn replay_pending_with(shared: &SharedState) -> Result<String, String> {
    let plan = {
        let mut guard = lock_app_state(shared)?;
        ShellApp::new(&mut guard).prepare_replay()?
    };
    let outcomes = plan.call_host();
    let mut guard = lock_app_state(shared)?;
    ShellApp::new(&mut guard).finish_replay(plan, outcomes)
}

/// 版本探测：本地版本在锁内读，probe/download 在锁外，安装与结论在锁内。
pub fn check_update_with(shared: &SharedState) -> Result<sync::UpdateOutcome, String> {
    let plan = {
        let mut guard = lock_app_state(shared)?;
        ShellApp::new(&mut guard).prepare_check_update()?
    };
    let prefetch = plan.prefetch();
    let mut guard = lock_app_state(shared)?;
    ShellApp::new(&mut guard).finish_check_update(plan, prefetch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Condvar;
    use tellerkit_journal_sqlite::SqliteJournal;

    /// 冻结时钟：证明"时间来自端口"而不是系统调用。
    struct FrozenClock(AtomicU64);

    impl Clock for FrozenClock {
        fn now_secs(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
    }

    impl FrozenClock {
        fn set(&self, t: u64) {
            self.0.store(t, Ordering::SeqCst);
        }
    }

    fn state_with(clock: Arc<FrozenClock>) -> AppState {
        ShellApp::fail_audits_after(usize::MAX);
        let dir = std::env::temp_dir().join(format!("tk-app-test-{}", std::process::id()));
        let store = tellerkit_core::MenuStore::new(dir, tellerkit_core::mock::demo_verifying_key());
        let mock = Arc::new(crate::ports::MockRemote::default());
        let mut state = AppState::new(
            store,
            "unused".into(),
            TxEngine::new(),
            None,
            Some(Arc::new(
                tellerkit_journal_sqlite::SqliteJournal::in_memory().unwrap(),
            )),
            crate::ports::RemoteChoice::new(
                Arc::clone(&mock) as Arc<dyn RemoteBanking>,
                Some(mock),
            ),
            tellerkit_core::mock::demo_verifying_key(),
        );
        state.set_clock(clock);
        state
            .session
            .lock()
            .expect("测试里会话锁不应中毒")
            .begin("T0000");
        state
    }

    fn submit_sync(state: &mut AppState, request: TxRequest) -> Result<TxResult, String> {
        let plan = ShellApp::new(state).prepare_submit_transaction(request)?;
        let outcome = if plan.needs_host() {
            Some(plan.call_host())
        } else {
            None
        };
        ShellApp::new(state).finish_submit_transaction(plan, outcome)
    }

    fn reversal_sync(state: &mut AppState, key: &str, reason: &str) -> Result<String, String> {
        let plan = ShellApp::new(state).prepare_reversal(key, reason)?;
        let outcome = plan.call_host();
        ShellApp::new(state).finish_reversal(plan, outcome)
    }

    fn replay_sync(state: &mut AppState) -> Result<String, String> {
        let plan = ShellApp::new(state).prepare_replay()?;
        let outcomes = plan.call_host();
        ShellApp::new(state).finish_replay(plan, outcomes)
    }

    fn check_update_sync(state: &mut AppState) -> Result<sync::UpdateOutcome, String> {
        let plan = ShellApp::new(state).prepare_check_update()?;
        let prefetch = plan.prefetch();
        ShellApp::new(state).finish_check_update(plan, prefetch)
    }

    use tellerkit_core::tx::TxEngine;

    #[test]
    fn session_locks_by_injected_clock_not_wall_time() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));

        assert!(!ShellApp::new(&mut state).session_tick().locked);

        clock.set(180);
        let view = ShellApp::new(&mut state).session_tick();
        assert!(view.locked, "注入 180 秒后应锁定");
        assert_eq!(view.reason.as_deref(), Some("无操作超时"));

        // 锁定是**状态**，不会因为再 tick 而自动解除（"不重复上报"由 core 的 evaluate 保证：
        // 它只在新进入锁定时返回 Some，这里看不到，但状态必须一直是 locked）
        clock.set(200);
        assert!(
            ShellApp::new(&mut state).session_tick().locked,
            "锁定状态应持续到显式解锁"
        );
    }

    #[test]
    fn offline_capable_is_refused_when_host_lacks_idempotency() {
        // 主机声明"不支持幂等"：离线挂起意味着之后一定重发，重发就会重复入账
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        state.remote = Arc::new(tellerkit_core::banking::RoutedBanking::new(
            tellerkit_core::remote::Router::new(vec![]),
        ));

        let mut req = TxRequest::new("account.deposit", "k-offline");
        req.set_offline_capable(true);
        let err = submit_sync(&mut state, req).unwrap_err();
        assert!(err.contains("未声明幂等"), "实际：{err}");

        // 在线模式不走这条拦截：会一路到端点，端点不存在则是"主机拒绝"（交易结论），
        // 不是 Err——状态机把失败表达成 TxResult，这一点不能被测试写反。
        let mut online = TxRequest::new("account.deposit", "k-online");
        online.set_offline_capable(false);
        let result = submit_sync(&mut state, online).expect("在线模式不应被幂等规则拦下");
        assert_eq!(result.status(), TxStatus::Rejected);
        assert!(
            result.message().contains("没有为该服务登记任何端点"),
            "实际：{}",
            result.message()
        );
    }

    #[test]
    fn touch_while_locked_does_not_extend_session() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        clock.set(180);
        assert!(ShellApp::new(&mut state).session_tick().locked);

        clock.set(400);
        let view = ShellApp::new(&mut state).session_touch();
        assert!(view.locked, "锁定期间的操作不得解锁");
    }

    /// 锁定后所有资金/打印/冲正/重放/包入口一律拒绝，
    /// 且主机调用次数为 0——把 mock 设成"在线且有响应"，
    /// 若闸门失效就会返回成功，测试会失败。
    fn locked_state() -> AppState {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        clock.set(180);
        assert!(ShellApp::new(&mut state).session_tick().locked);
        state
    }

    #[test]
    fn locked_session_refuses_all_protected_commands() {
        let mut state = locked_state();

        let err = submit_sync(&mut state, TxRequest::new("account.deposit", "k-lock")).unwrap_err();
        assert!(err.contains("会话已锁定"), "提交应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state)
            .print_voucher(Voucher::new("voucher", "account.deposit"))
            .unwrap_err();
        assert!(err.contains("会话已锁定"), "打印应被闸门拒绝：{err}");

        let err = reversal_sync(&mut state, "k-lock", "冲正").unwrap_err();
        assert!(err.contains("会话已锁定"), "冲正应被闸门拒绝：{err}");

        let err = replay_sync(&mut state).unwrap_err();
        assert!(err.contains("会话已锁定"), "重放应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state)
            .package_module("pkg.any.route")
            .unwrap_err();
        assert!(err.contains("会话已锁定"), "业务包入口应被闸门拒绝：{err}");
    }

    #[test]
    fn missing_session_refuses_protected_commands() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        state.session.lock().expect("测试里会话锁不应中毒").end();

        let err = submit_sync(&mut state, TxRequest::new("account.deposit", "k-none")).unwrap_err();
        assert!(err.contains("尚未登录"), "未登录应被闸门拒绝：{err}");

        let err = replay_sync(&mut state).unwrap_err();
        assert!(err.contains("尚未登录"), "未登录重放应被闸门拒绝：{err}");

        let err = reversal_sync(&mut state, "k-none", "冲正").unwrap_err();
        assert!(err.contains("尚未登录"), "未登录冲正应被闸门拒绝：{err}");
    }

    #[test]
    fn unlocked_session_allows_commands_to_reach_host() {
        // 反向对照：解锁后必须真的走到主机，证明闸门不是"一律拒绝"。
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        let result = submit_sync(&mut state, TxRequest::new("account.deposit", "k-ok"))
            .expect("解锁后应放行到主机");
        assert_eq!(result.status(), TxStatus::Committed);
    }

    fn state_with_package(templates: Vec<String>) -> AppState {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        state.audit = Some(Arc::new(SqliteJournal::in_memory().expect("内存账本")));

        let mut routes = BTreeMap::new();
        routes.insert("/biz/demo/hello".to_string(), "ui/entry.js".to_string());
        let mut assets = BTreeMap::new();
        assets.insert(
            "ui/entry.js".to_string(),
            "export function mount() {}".to_string(),
        );
        let tx = tellerkit_core::pkg::TxMeta::new("biz.demo.hello", "client-uuid", false, vec![])
            .with_templates(templates);
        state.packages.push(tellerkit_core::BizPackage::new(
            tellerkit_core::pkg::BizManifest::new(
                "biz.demo",
                "示例包",
                "0.1.0",
                "0.1.0",
                "0.x",
                vec![],
                "ui/entry.js",
            ),
            vec![tellerkit_core::MenuNode::page(
                "biz.demo.hello",
                None,
                "示例交易",
                0,
            )],
            pkg::Permissions::default(),
            routes,
            vec![tx],
            assets,
        ));
        state
    }

    fn package_request(code: &str, key: &str) -> TxRequest {
        let mut request = TxRequest::new(code, key);
        request
            .elements_mut()
            .insert(PKG_ORIGIN_KEY.to_string(), "pkg:biz.demo".to_string());
        request
    }

    fn package_voucher(template: &str) -> Voucher {
        let mut voucher = Voucher::new(template, "biz.demo.hello");
        let mut fields = BTreeMap::new();
        fields.insert(PKG_ORIGIN_KEY.to_string(), "pkg:biz.demo".to_string());
        voucher.set_fields(fields);
        voucher
    }

    #[test]
    fn package_module_view_carries_source_and_capabilities() {
        let mut state = state_with_package(vec!["demo.receipt".to_string()]);
        let view = ShellApp::new(&mut state)
            .package_module("/biz/demo/hello")
            .expect("已加载的包应返回视图");
        assert_eq!(view.origin(), "pkg:biz.demo");
        assert!(view.source().contains("mount"));
        assert!(view.capabilities().allows_method("print"));
        assert!(view.capabilities().allows_method("submit"));
        assert!(view.capabilities().allows_method("log"));
        assert!(view.capabilities().allows_tx_code("biz.demo.hello"));
        assert!(view.capabilities().allows_template("demo.receipt"));
    }

    #[test]
    fn package_call_with_declared_tx_code_is_allowed_and_audited() {
        let mut state = state_with_package(vec!["demo.receipt".to_string()]);
        let result = submit_sync(&mut state, package_request("biz.demo.hello", "k-pkg-ok"))
            .expect("清单声明的交易码应放行");
        assert_eq!(result.status(), TxStatus::Committed);

        let tail = ShellApp::new(&mut state).audit_tail(3).expect("审计可用");
        assert!(
            tail.iter().any(|line| line.contains("pkg:biz.demo")),
            "实际：{tail:?}"
        );
        assert!(
            tail.iter().any(|line| line.contains("code=biz.demo.hello")),
            "实际：{tail:?}"
        );
    }

    fn audit_events(state: &AppState) -> Vec<(String, String, String)> {
        state
            .audit
            .as_ref()
            .unwrap()
            .audit_tail(100)
            .unwrap()
            .into_iter()
            .map(|e| {
                (
                    e.action().to_string(),
                    e.result().to_string(),
                    e.detail().to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn login_failure_is_audited_as_failed() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        let err = ShellApp::new(&mut state).login("no-such-role").unwrap_err();
        assert!(err.contains("未知角色"), "实际：{err}");

        let events = audit_events(&state);
        assert!(
            events
                .iter()
                .any(|(action, result, detail)| action == "login"
                    && result == "failed"
                    && detail.contains("no-such-role")),
            "登录失败必须留下 failed 审计：{events:?}"
        );
    }

    #[test]
    fn audit_write_failure_blocks_before_host() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        ShellApp::fail_audits_after(0);
        let err = submit_sync(
            &mut state,
            TxRequest::new("account.deposit", "k-audit-block"),
        )
        .unwrap_err();
        assert!(err.contains("审计写前留痕失败"), "实际：{err}");
        assert!(err.contains("未触达主机"), "实际：{err}");
        assert!(
            audit_events(&state).is_empty(),
            "写前留痕失败不得留下半条审计"
        );
    }

    #[test]
    fn conclusion_audit_failure_is_reported_after_business_event() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        ShellApp::fail_audits_after(1);
        let err = submit_sync(
            &mut state,
            TxRequest::new("account.deposit", "k-audit-conclusion"),
        )
        .unwrap_err();
        assert!(err.contains("审计结论写入失败"), "实际：{err}");
        assert!(err.contains("业务已发生"), "实际：{err}");
        assert!(err.contains("请立即查账"), "实际：{err}");
    }

    #[test]
    fn denied_audit_failure_is_not_swallowed() {
        let mut state = locked_state();
        ShellApp::fail_audits_after(0);
        let err = submit_sync(
            &mut state,
            TxRequest::new("account.deposit", "k-audit-denied"),
        )
        .unwrap_err();
        assert!(err.contains("会话已锁定"), "实际：{err}");
        assert!(err.contains("审计写入失败"), "实际：{err}");
    }

    #[test]
    fn reversal_host_failure_is_audited_as_failed() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        state
            .reversal
            .mark("k-rev-fail", ExceptionKind::MarkedForReversal);
        state.remote = Arc::new(tellerkit_core::banking::RoutedBanking::new(
            tellerkit_core::remote::Router::new(vec![]),
        ));
        let err = reversal_sync(&mut state, "k-rev-fail", "冲正").unwrap_err();
        assert!(
            audit_events(&state)
                .iter()
                .any(|(action, result, _)| action == "reversal" && result == "failed"),
            "冲正失败必须留痕：{err}"
        );
    }

    #[test]
    fn package_call_with_undeclared_tx_code_is_denied_and_audited() {
        let mut state = state_with_package(vec!["demo.receipt".to_string()]);
        let err =
            submit_sync(&mut state, package_request("account.withdraw", "k-pkg-bad")).unwrap_err();
        assert!(err.contains("未声明交易码"), "实际：{err}");

        let tail = ShellApp::new(&mut state).audit_tail(3).expect("审计可用");
        assert!(
            tail.iter()
                .any(|line| line.contains("pkg:biz.demo") && line.contains("account.withdraw")),
            "实际：{tail:?}"
        );
    }

    #[test]
    fn package_call_with_undeclared_template_is_denied_and_audited() {
        let mut state = state_with_package(vec!["demo.receipt".to_string()]);
        let err = ShellApp::new(&mut state)
            .print_voucher(package_voucher("foreign.receipt"))
            .unwrap_err();
        assert!(err.contains("未声明模板"), "实际：{err}");

        let tail = ShellApp::new(&mut state).audit_tail(3).expect("审计可用");
        assert!(
            tail.iter()
                .any(|line| line.contains("pkg:biz.demo") && line.contains("foreign.receipt")),
            "实际：{tail:?}"
        );
    }

    #[test]
    fn undeclared_method_is_denied_before_device_is_touched() {
        let mut state = state_with_package(vec![]);
        let err = ShellApp::new(&mut state)
            .print_voucher(package_voucher("demo.receipt"))
            .unwrap_err();
        assert!(err.contains("未声明方法 print"), "实际：{err}");
    }

    #[test]
    fn unknown_package_origin_is_denied() {
        let mut state = state_with_package(vec!["demo.receipt".to_string()]);
        let mut request = package_request("biz.demo.hello", "k-unknown");
        request
            .elements_mut()
            .insert(PKG_ORIGIN_KEY.to_string(), "pkg:biz.other".to_string());
        let err = submit_sync(&mut state, request).unwrap_err();
        assert!(err.contains("未加载或验签未通过"), "实际：{err}");
    }
    #[test]
    fn boot_verifies_chain_and_reports_its_head() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        let summary = ShellApp::new(&mut state).boot().unwrap();
        assert!(
            summary
                .notes()
                .iter()
                .any(|n| n.contains("审计链头部 seq=") && n.contains("hash=")),
            "启动摘要应报告链头：{:?}",
            summary.notes()
        );
    }

    #[test]
    fn awaiting_reversal_retries_with_same_key_and_clears_exception() {
        let mut state = state_with(Arc::new(FrozenClock(AtomicU64::new(0))));
        state.audit = Some(Arc::new(
            tellerkit_journal_sqlite::SqliteJournal::in_memory().unwrap(),
        ));
        state
            .reversal
            .mark("k-rev", ExceptionKind::MarkedForReversal);
        let mock = Arc::clone(state.mock.as_ref().unwrap());

        mock.set_offline(true);
        let err = reversal_sync(&mut state, "k-rev", "状态不明").unwrap_err();
        assert!(err.contains("断网"), "首次主机失败应透出原因：{err}");
        let authorized = state.reversal.reversal_of("k-rev").unwrap().clone();
        assert_eq!(authorized.host_reference(), None);

        // 已授权未确认时重试仍失败：不能撤销授权，也不能清异常清单。
        let retry_err = reversal_sync(&mut state, "k-rev", "状态不明").unwrap_err();
        assert!(retry_err.contains("已获授权"), "{retry_err}");
        assert!(retry_err.contains("重试"), "{retry_err}");
        assert_eq!(state.reversal.reversal_of("k-rev"), Some(&authorized));
        assert!(ShellApp::new(&mut state).tx_exceptions().len() == 1);

        // 恢复后再次重试：同一幂等键重打主机、确认成功、清单清空并留审计。
        mock.set_offline(false);
        let reference =
            reversal_sync(&mut state, "k-rev", "状态不明").expect("待确认冲正应能重试成功");
        assert_eq!(reference, "REV-k-rev", "重试必须使用同一幂等键");
        assert!(state.reversal.is_confirmed("k-rev"));
        assert_eq!(
            state.reversal.reversal_of("k-rev").unwrap().initiator(),
            authorized.initiator()
        );
        let app = ShellApp::new(&mut state);
        assert!(app.tx_exceptions().is_empty(), "主机确认后异常清单应清空");
        let audit = app.audit_tail(10).unwrap();
        assert!(
            audit.iter().any(|line| line.contains("reversal/retry")
                && line.contains("ok")
                && line.contains("REV-k-rev")),
            "重试成功需写 reversal/retry/ok 审计，实际：{audit:?}"
        );
    }

    /// 会话独立锁的验收点：交易持住 AppState 时，锁屏 tick 仍能拿到会话槽。
    #[test]
    fn session_tick_does_not_wait_for_app_state_lock() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let state = state_with(Arc::clone(&clock));
        let session = Arc::clone(&state.session);
        let shared = Mutex::new(state);

        let held = shared.lock().expect("测试里 AppState 锁不应中毒");
        clock.set(180);
        let mut slot = session
            .try_lock()
            .expect("会话槽必须与 AppState 分开，否则 tick 会排在交易后面");
        let view = slot.tick();
        assert!(view.locked, "注入 180 秒后应锁定");
        assert_eq!(view.reason.as_deref(), Some("无操作超时"));
        drop(slot);
        drop(held);
    }

    /// cfg(test) 的慢主机：进入 submit 后一直等到测试放行，用来证明放锁。
    struct SlowHost {
        entered: Arc<(Mutex<bool>, Condvar)>,
        release: Arc<(Mutex<bool>, Condvar)>,
        calls: Arc<AtomicUsize>,
    }

    impl SlowHost {
        fn new() -> Self {
            Self {
                entered: Arc::new((Mutex::new(false), Condvar::new())),
                release: Arc::new((Mutex::new(false), Condvar::new())),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn wait_until_entered(&self) {
            let (lock, cv) = &*self.entered;
            let mut seen = lock.lock().expect("测试锁不应中毒");
            while !*seen {
                seen = cv.wait(seen).expect("测试锁不应中毒");
            }
        }

        fn release(&self) {
            let (lock, cv) = &*self.release;
            let mut open = lock.lock().expect("测试锁不应中毒");
            *open = true;
            cv.notify_all();
        }
    }

    impl TxHost for SlowHost {
        fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
            let (entered_lock, entered_cv) = &*self.entered;
            *entered_lock.lock().expect("测试锁不应中毒") = true;
            entered_cv.notify_all();
            let (release_lock, release_cv) = &*self.release;
            let mut open = release_lock.lock().expect("测试锁不应中毒");
            while !*open {
                open = release_cv.wait(open).expect("测试锁不应中毒");
            }
            drop(open);
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok("TX-SLOW".into())
        }

        fn supports_idempotency(&self) -> bool {
            true
        }
    }

    impl MenuSource for SlowHost {
        fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError> {
            Err(RemoteError::Unavailable("慢主机".into()))
        }

        fn download_menu(&self) -> Result<Vec<u8>, RemoteError> {
            Err(RemoteError::Unavailable("慢主机".into()))
        }
    }

    impl RemoteBanking for SlowHost {
        fn login(
            &self,
            _role: &str,
            _version: u64,
        ) -> Result<tellerkit_core::PermSnapshot, RemoteError> {
            Err(RemoteError::Unavailable("慢主机".into()))
        }

        fn reverse(&self, _key: &str, _reason: &str) -> Result<String, RemoteError> {
            Err(RemoteError::Unavailable("慢主机".into()))
        }
    }

    fn state_with_slow_host() -> (SharedState, Arc<SlowHost>) {
        let state = state_with(Arc::new(FrozenClock(AtomicU64::new(0))));
        let slow = Arc::new(SlowHost::new());
        let mut state = state;
        state.remote = Arc::clone(&slow) as Arc<dyn RemoteBanking>;
        (Arc::new(Mutex::new(state)), slow)
    }

    /// 放锁证明：主机阻塞期间另一线程能拿到 AppState，锁屏 tick 也能返回。
    #[test]
    fn slow_host_call_releases_app_state_and_session_lock() {
        let (shared, slow) = state_with_slow_host();
        let worker_state = Arc::clone(&shared);
        let worker = std::thread::spawn(move || {
            ShellApp::fail_audits_after(usize::MAX);
            submit_transaction_with(&worker_state, TxRequest::new("account.deposit", "k-slow"))
        });

        slow.wait_until_entered();
        let guard = shared.try_lock().expect("主机调用期间 AppState 不应被占用");
        let session = Arc::clone(&guard.session);
        drop(guard);
        let mut slot = session.lock().expect("会话槽不应被交易占用");
        assert!(!slot.tick().locked, "交易中锁屏 tick 应立即返回");
        drop(slot);

        slow.release();
        let result = worker
            .join()
            .expect("交易线程不应 panic")
            .expect("交易应成功");
        assert_eq!(result.status(), TxStatus::Committed);
        assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
    }

    /// 同键并发：第一笔还在飞时，第二笔必须被单飞表拒绝且不再打主机。
    #[test]
    fn second_submit_of_same_key_is_rejected_while_first_in_flight() {
        let (shared, slow) = state_with_slow_host();
        let worker_state = Arc::clone(&shared);
        let worker = std::thread::spawn(move || {
            ShellApp::fail_audits_after(usize::MAX);
            submit_transaction_with(&worker_state, TxRequest::new("account.deposit", "k-dup"))
        });

        slow.wait_until_entered();
        let err = submit_transaction_with(&shared, TxRequest::new("account.deposit", "k-dup"))
            .expect_err("同键在途的第二笔必须被拒绝");
        assert!(err.contains("正在处理中"), "实际：{err}");
        slow.release();

        let result = worker
            .join()
            .expect("交易线程不应 panic")
            .expect("首笔应成功");
        assert_eq!(result.status(), TxStatus::Committed);
        assert_eq!(slow.calls.load(Ordering::SeqCst), 1, "第二笔不得再打主机");
    }

    #[test]
    fn in_flight_key_with_different_payload_is_refused() {
        let (shared, slow) = state_with_slow_host();
        let worker_state = Arc::clone(&shared);
        let worker = std::thread::spawn(move || {
            ShellApp::fail_audits_after(usize::MAX);
            submit_transaction_with(
                &worker_state,
                TxRequest::new("account.deposit", "k-change").amount(100),
            )
        });

        slow.wait_until_entered();
        let err = submit_transaction_with(
            &shared,
            TxRequest::new("account.deposit", "k-change").amount(200),
        )
        .expect_err("同键换要素必须被拒绝");
        assert!(err.contains("另一笔交易"), "实际：{err}");
        slow.release();
        worker
            .join()
            .expect("交易线程不应 panic")
            .expect("首笔应成功");
        assert_eq!(slow.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn check_update_prefetches_outside_lock_and_audits_conclusion() {
        let mut state = state_with(Arc::new(FrozenClock(AtomicU64::new(0))));
        let outcome = check_update_sync(&mut state).expect("版本探测应成功");
        assert!(outcome.checked(), "实际：{}", outcome.note());
        assert!(
            audit_events(&state)
                .iter()
                .any(|(action, result, _)| action == "check_update" && result == "ok"),
            "版本探测结论必须留痕"
        );
    }
}

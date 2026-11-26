//! L3 应用层：用例编排。
//!
//! 规则（docs/architecture.md §2）：
//! - 只依赖领域（`tellerkit-core`）与[端口](crate::ports)，**不知道** Tauri、不知道 DTO；
//! - 这里写"先做什么、再做什么、失败怎么降级"；
//! - 命令层（`commands.rs`）只负责加锁 → 调这里 → 映射成 IPC DTO。
//!
//! 为什么不用"一个用例一个 struct"：没有第二个实现，结构体只是把函数换了个形状
//! （见 §6 的三问结论）。

use std::collections::BTreeMap;
use std::sync::Arc;

use tellerkit_core as sync;
use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::mock::DemoRole;
use tellerkit_core::pkg;
use tellerkit_core::pubkey_to_hex;
use tellerkit_core::reversal::{
    Authorization, Authorizer, ExceptionKind, ReversalError, ReversalRequest,
};
use tellerkit_core::session::{SessionGuard, SessionPolicy};
use tellerkit_core::tx::{TxRequest, TxResult, TxStatus};
use tellerkit_core::MenuTree;
use tellerkit_core::MenuView;
use tellerkit_core::{filter_visible, LoadSource};
use tellerkit_journal_sqlite::AuditEvent;

use crate::bootstrap::StartupFatal;
use crate::ports::{Clock, RemoteBanking};
use crate::AppState;

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
        match &self.state.session {
            Some(g) if !g.is_locked() => Ok(()),
            other => {
                let (reason, detail) = match other {
                    None => ("尚未登录", "未登录即调用受保护命令"),
                    Some(g) => {
                        let why = g.reason().map(|r| r.label()).unwrap_or("会话已锁定");
                        ("会话已锁定", why)
                    }
                };
                let actor = self.actor();
                let denial = format!("{reason}：{detail}。请回到锁屏界面解锁后再试");
                let audited = self.audit(&actor, action, "", "denied", detail);
                match audited {
                    Ok(()) => Err(denial),
                    // 审计失败不能吞：调用方必须知道"拒绝"这件事没留下证据。
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
                self.state.startup_fatal = StartupFatal(Some(reason.clone()));
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
        self.state.session = Some(SessionGuard::new(
            SessionPolicy::default(),
            self.clock.now_secs(),
        ));
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
        self.state.session = None;
        self.state.clear_session();
        Ok(())
    }

    pub fn current_menu(&self) -> Result<Vec<MenuView>, String> {
        let snapshot = self.state.snapshot.as_ref().ok_or("尚未登录")?;
        let menu = self.state.menu.as_ref().ok_or("菜单尚未加载")?;
        Ok(filter_visible(menu, snapshot))
    }

    /// 后台版本探测。菜单版本更新不重置权限快照——两者独立演进。
    pub fn check_update(&mut self) -> Result<sync::UpdateOutcome, String> {
        let outcome = sync::check_and_update(self.state.remote.as_ref(), &self.state.store);
        if outcome.updated() {
            if let Ok(loaded) = self.state.store.load_latest() {
                self.state.menu = Some(loaded.into_tree());
            }
        }
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

    /// 申请冲正：本地授权 → 主机受理 → 拿到流水号后才算处置完成。
    pub fn request_reversal(&mut self, key: &str, reason: &str) -> Result<String, String> {
        self.require_unlocked("reversal")?;
        self.require_auditable("reversal", key)?;
        let actor = self.actor();
        let request = ReversalRequest::new(key, &actor, "", 0, reason);

        // 1. 授权（当前是演示主管；接 IAM 后换成真实授权端口）
        if let Err(e) = self.state.reversal.request(&request, &DemoSupervisor) {
            let result = match &e {
                ReversalError::NotAuthorized(_) | ReversalError::SelfApproval(_) => "denied",
                _ => "failed",
            };
            let why = e.to_string();
            let err = self.audit_and_error(
                &actor,
                "reversal",
                key,
                result,
                &why,
                format!("冲正申请未通过：{why}"),
            );
            return Err(err);
        }

        // 2. 主机受理。失败时保留"已授权未确认"，柜员可以重试
        let host_reference = match self.remote.reverse(key, reason) {
            Ok(reference) => reference,
            Err(e) => {
                let why = e.to_string();
                return Err(self.audit_and_error(
                    &actor,
                    "reversal",
                    key,
                    "failed",
                    &format!("主机冲正失败：{why}"),
                    format!("主机冲正失败：{why}"),
                ));
            }
        };

        // 3. 主机回了流水号才算处置完成
        if let Err(e) = self.state.reversal.confirm(key, &host_reference) {
            let why = e.to_string();
            return Err(self.audit_and_error(
                &actor,
                "reversal",
                key,
                "failed",
                &format!("主机已受理但本地确认失败：{why}"),
                format!("主机已受理冲正（{host_reference}）但本地确认失败：{why}；请立即查账"),
            ));
        }

        self.audit_checked(&actor, "reversal", key, "ok", &host_reference)?;
        Ok(host_reference)
    }

    pub fn submit_transaction(&mut self, mut request: TxRequest) -> Result<TxResult, String> {
        self.require_unlocked("submit_tx")?;
        let code = request.tx_code().to_string();
        let call = self.take_call(request.elements_mut(), &code)?;
        if let Some(call) = &call {
            self.authorize_package_call(call, "submit", &code, None)?;
        }
        self.require_auditable("submit_tx", request.idempotency_key())?;
        // 契约 lifecycle.md 不变量 7：主机没声明幂等，就不许离线挂起。
        // 挂起意味着"以后一定会重发"，而重发只有服务端按幂等键去重才不会重复入账。
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
        let amount_cents = request.amount_cents();
        let code = request.tx_code().to_string();
        let key = request.idempotency_key().to_string();
        let actor = self.actor();

        let result = self.state.tx.submit(self.remote.as_ref(), request);
        self.mark_if_uncertain(&key, &result);

        let detail = format!(
            "code={code} status={:?} amount_cents={amount_cents}",
            result.status()
        );
        let target = call
            .as_ref()
            .map(PackageCall::origin)
            .unwrap_or_else(|| code.clone());
        self.audit_checked(
            &actor,
            "submit_tx",
            &target,
            if result.status() == TxStatus::Committed {
                "ok"
            } else {
                "failed"
            },
            &detail,
        )?;
        Ok(result)
    }

    pub fn replay_pending(&mut self) -> Result<String, String> {
        self.require_unlocked("replay")?;
        self.require_auditable("replay", "tx_queue")?;
        let remote = Arc::clone(&self.remote);
        let now = self.clock.now_secs();
        let run = tellerkit_core::replay::run_once(
            &mut self.state.tx,
            &mut self.state.replay,
            remote.as_ref(),
            now,
        );

        if run.attempted() == 0 && run.still_pending() == 0 {
            return Ok("没有待重放的交易".to_string());
        }
        if run.attempted() == 0 {
            return Ok(format!(
                "还有 {} 笔挂起，均未到重试时间",
                run.still_pending()
            ));
        }

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

        Ok(format!(
            "重放 {} 笔：成功 {} 笔，仍挂起 {} 笔{}",
            run.attempted(),
            run.committed(),
            run.still_pending(),
            if run.gave_up().is_empty() {
                String::new()
            } else {
                format!("，{} 笔已放弃自动重试，需人工处理", run.gave_up().len())
            }
        ))
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

    pub fn session_touch(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        if let Some(guard) = self.state.session.as_mut() {
            guard.touch(now);
        }
        self.session_view()
    }

    pub fn session_tick(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let actor = self.actor();
        let locked = self
            .state
            .session
            .as_mut()
            .and_then(|guard| guard.evaluate(now));
        if let Some(reason) = locked {
            if let Err(e) = self.audit(&actor, "session_lock", "", "ok", reason.label()) {
                eprintln!("会话锁定审计写入失败：{e}");
            }
        }
        self.session_view()
    }

    pub fn session_blur(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let actor = self.actor();
        let locked = self
            .state
            .session
            .as_mut()
            .and_then(|guard| guard.on_blur(now));
        if let Some(reason) = locked {
            if let Err(e) = self.audit(&actor, "session_lock", "", "ok", reason.label()) {
                eprintln!("会话锁定审计写入失败：{e}");
            }
        }
        self.session_view()
    }

    fn session_view(&self) -> SessionView {
        match &self.state.session {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
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
        state.clock = clock;
        state.session = Some(SessionGuard::new(SessionPolicy::default(), 0));
        state
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
        let err = ShellApp::new(&mut state)
            .submit_transaction(req)
            .unwrap_err();
        assert!(err.contains("未声明幂等"), "实际：{err}");

        // 在线模式不走这条拦截：会一路到端点，端点不存在则是"主机拒绝"（交易结论），
        // 不是 Err——状态机把失败表达成 TxResult，这一点不能被测试写反。
        let mut online = TxRequest::new("account.deposit", "k-online");
        online.set_offline_capable(false);
        let result = ShellApp::new(&mut state)
            .submit_transaction(online)
            .expect("在线模式不应被幂等规则拦下");
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

        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-lock"))
            .unwrap_err();
        assert!(err.contains("会话已锁定"), "提交应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state)
            .print_voucher(Voucher::new("voucher", "account.deposit"))
            .unwrap_err();
        assert!(err.contains("会话已锁定"), "打印应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state)
            .request_reversal("k-lock", "冲正")
            .unwrap_err();
        assert!(err.contains("会话已锁定"), "冲正应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state).replay_pending().unwrap_err();
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
        state.session = None;

        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-none"))
            .unwrap_err();
        assert!(err.contains("尚未登录"), "未登录应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state).replay_pending().unwrap_err();
        assert!(err.contains("尚未登录"), "未登录重放应被闸门拒绝：{err}");

        let err = ShellApp::new(&mut state)
            .request_reversal("k-none", "冲正")
            .unwrap_err();
        assert!(err.contains("尚未登录"), "未登录冲正应被闸门拒绝：{err}");
    }

    #[test]
    fn unlocked_session_allows_commands_to_reach_host() {
        // 反向对照：解锁后必须真的走到主机，证明闸门不是"一律拒绝"。
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        let result = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-ok"))
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
        let result = ShellApp::new(&mut state)
            .submit_transaction(package_request("biz.demo.hello", "k-pkg-ok"))
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
        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-audit-block"))
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
        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-audit-conclusion"))
            .unwrap_err();
        assert!(err.contains("审计结论写入失败"), "实际：{err}");
        assert!(err.contains("业务已发生"), "实际：{err}");
        assert!(err.contains("请立即查账"), "实际：{err}");
    }

    #[test]
    fn denied_audit_failure_is_not_swallowed() {
        let mut state = locked_state();
        ShellApp::fail_audits_after(0);
        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-audit-denied"))
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
        let err = ShellApp::new(&mut state)
            .request_reversal("k-rev-fail", "冲正")
            .unwrap_err();
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
        let err = ShellApp::new(&mut state)
            .submit_transaction(package_request("account.withdraw", "k-pkg-bad"))
            .unwrap_err();
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
        let err = ShellApp::new(&mut state)
            .submit_transaction(request)
            .unwrap_err();
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
}

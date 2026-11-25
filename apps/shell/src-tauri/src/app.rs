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

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::filter::MenuView;
use tellerkit_core::mock::DemoRole;
use tellerkit_core::model::MenuTree;
use tellerkit_core::pack::pubkey_to_hex;
use tellerkit_core::pkg;
use tellerkit_core::recovery::{exception_kind_str, RecoveryJournal};
use tellerkit_core::reversal::{Authorization, Authorizer, ExceptionKind, ReversalRequest};
use tellerkit_core::session::{LockReason, SessionGuard, SessionPolicy};
use tellerkit_core::sync;
use tellerkit_core::tx::{HostError, SubmitStart, TxHost, TxRequest, TxResult, TxStatus};
use tellerkit_core::{filter, LoadSource};
use tellerkit_journal_sqlite::{AuditEvent, SqliteJournal};

use crate::ports::{Clock, RemoteBanking};
use crate::AppState;

/// 包身份借用既有 IPC 参数 map 的保留键，不新增契约字段。
const PKG_IDENTITY_SOURCE_KEY: &str = "__pkg_source";
const PKG_IDENTITY_CODE_KEY: &str = "__pkg_code";
/// 打印凭证需要的设备能力；对应 `DeviceKind::Printer` 的 serde 值。
const PKG_DEVICE_PRINTER: &str = "printer";
/// 模块源码首行的身份注释，由 `package_module` 写在包代码之前。
const PKG_IDENTITY_HEADER: &str = "// @tk-package-identity ";

/// 一次包调用的身份：由 Rust 注入、随 IPC 回到 Rust，前端不判定权限。
#[derive(Debug, Clone, PartialEq, Eq)]
struct PackageCall {
    source: String,
    code: String,
    id: String,
}

impl PackageCall {
    /// 取出并移除身份键。身份缺失视为普通主壳调用；格式不完整/非法一律拒绝，
    /// 防止包伪造一个坏 source 来绕过能力校验。
    fn take_from(fields: &mut BTreeMap<String, String>) -> Result<Option<Self>, String> {
        let source = fields.remove(PKG_IDENTITY_SOURCE_KEY);
        let code = fields.remove(PKG_IDENTITY_CODE_KEY);
        match (source, code) {
            (None, None) => Ok(None),
            (Some(source), Some(code)) => {
                if source.starts_with("pkg:") && source.len() > 4 {
                    Ok(Some(Self {
                        id: source[4..].to_string(),
                        source,
                        code,
                    }))
                } else {
                    Err(format!("业务包身份格式非法：source={source}"))
                }
            }
            _ => Err("业务包身份不完整：source 与 code 必须同时提供".to_string()),
        }
    }

    fn label(&self) -> String {
        format!("{} / {}", self.source, self.code)
    }
}

/// 启动结果。命令层据此拼 `BootInfo`。
#[derive(Debug, Clone)]
pub struct BootSummary {
    pub shell_version: String,
    pub pubkey_hex: String,
    pub appdata_dir: String,
    pub offline: bool,
    pub local_versions: Vec<u64>,
    pub menu_version: u64,
    pub source: String,
    pub menu_nodes: usize,
    pub pack_bytes: usize,
    pub load_ms: f64,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub snapshot: tellerkit_core::PermSnapshot,
    pub menu: Vec<MenuView>,
    pub filter_ms: f64,
}

#[derive(Debug, Clone)]
pub struct ReloadOutcome {
    pub menu_version: u64,
    pub source: String,
    pub verify_failed: bool,
    pub note: String,
    pub menu: Option<Vec<MenuView>>,
}

/// 会话视图（给界面看的三元组）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub locked: bool,
    pub reason: Option<String>,
    pub idle_secs: u64,
}

/// 会话状态的独立锁（P0-10 第 2 步）。
///
/// 为什么单独一把锁：锁屏 tick 每秒一次，而交易要持 `AppState` 锁打主机
/// （默认 3s 超时、按优先级切端点更久）。共用一把锁时，一笔慢交易会让锁屏判定
/// 停摆同样的秒数——那是安全检查停摆，不是体验问题。所以 tick/touch/blur
/// 只碰这把锁，完全不碰 `AppState`。
///
/// **锁序约定**：`AppState` → `SessionSlot`，任何路径都不许反向加锁。
/// 反向（先会话后状态）会在"交易持状态锁等会话锁、tick 持会话锁等状态锁"时死锁。
///
/// `clock` / `audit` 与 `AppState` 里的必须是**同一份**（`AppState::new` 保证）；
/// 复制一份句柄是为了让 tick 不必回头去拿状态锁。
pub struct SessionSlot {
    guard: Option<SessionGuard>,
    /// 当前柜员号。锁屏审计要用；登录成功写入、登出清空。
    teller_no: Option<String>,
    clock: Arc<dyn Clock>,
    audit: Option<Arc<SqliteJournal>>,
}

impl SessionSlot {
    pub fn new(clock: Arc<dyn Clock>, audit: Option<Arc<SqliteJournal>>) -> Self {
        Self {
            guard: None,
            teller_no: None,
            clock,
            audit,
        }
    }

    /// 测试用：换时钟。锁屏判定必须用注入的时钟，只换 `AppState.clock` 不够。
    #[cfg(test)]
    pub fn set_clock(&mut self, clock: Arc<dyn Clock>) {
        self.clock = clock;
    }

    /// 登录成功：开一段新会话，计时从此刻起算。
    pub fn begin(&mut self, teller_no: &str) {
        self.guard = Some(SessionGuard::new(
            SessionPolicy::default(),
            self.clock.now_secs(),
        ));
        self.teller_no = Some(teller_no.to_string());
    }

    /// 登出：身份与会话一起清。先清身份，避免登出后的审计还记到上一个人头上。
    pub fn end(&mut self) {
        self.guard = None;
        self.teller_no = None;
    }

    /// 资金类命令的会话判定。`None` 表示放行，`Some((原因, 细节))` 表示拒绝。
    ///
    /// 只读判定、不写审计：调用方（`ShellApp::require_unlocked`）还要拼柜员号，
    /// 并且不能持着会话锁去写账本。
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

    /// 记录一次操作。锁定期间不续命由 `SessionGuard` 保证。
    pub fn touch(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        if let Some(guard) = self.guard.as_mut() {
            guard.touch(now);
        }
        self.view()
    }

    /// 周期性评估：新进入锁定时写一条 `session_lock` 审计。
    pub fn tick(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let newly_locked = self.guard.as_mut().and_then(|guard| guard.evaluate(now));
        let view = self.view();
        self.audit_lock(newly_locked);
        view
    }

    /// 窗口失焦：按策略锁定，新进入锁定时写审计。
    pub fn blur(&mut self) -> SessionView {
        let now = self.clock.now_secs();
        let newly_locked = self.guard.as_mut().and_then(|guard| guard.on_blur(now));
        let view = self.view();
        self.audit_lock(newly_locked);
        view
    }

    /// 锁屏事件留痕。持锁期间只做一条 INSERT（亚毫秒级）；真正的长阻塞是主机调用，
    /// 已经不在会话锁里了。写不进去只告警：锁屏是安全动作，不能因为账本坏就不锁。
    fn audit_lock(&self, reason: Option<LockReason>) {
        let Some(reason) = reason else {
            return;
        };
        let Some(journal) = self.audit.as_ref() else {
            return;
        };
        let event = AuditEvent {
            actor: self.teller_no.clone().unwrap_or_else(|| "unknown".into()),
            action: "session_lock".to_string(),
            target: String::new(),
            result: "ok".to_string(),
            detail: reason.label().to_string(),
            at: String::new(),
        };
        if let Err(e) = journal.audit(&event) {
            eprintln!("审计写入失败：{e}");
        }
    }
}

/// 会话锁中毒时的兜底视图：按**已锁定**处理。
///
/// 读不到会话状态就想放行资金操作，是最不该有的降级方向。
fn poisoned_session_view() -> SessionView {
    SessionView {
        locked: true,
        reason: Some("会话状态异常".into()),
        idle_secs: 0,
    }
}

/// 对账视图：聚合 + 最近明细。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    /// 统计起点（当日 00:00 的秒级字符串）
    pub since: String,
    pub summary: Vec<StatusRow>,
    pub recent: Vec<RecentTxView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusRow {
    pub status: String,
    pub count: i64,
    pub amount_cents: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTxView {
    pub key: String,
    pub tx_code: String,
    pub amount_cents: i64,
    pub status: String,
    pub reference: Option<String>,
}

/// 当日 00:00 的秒级时间戳（零填充，与账本 `created_at` 同格式）。
fn start_of_day(now_secs: u64) -> String {
    let day = now_secs / 86_400 * 86_400;
    format!("{day:020}")
}

/// 待处置异常交易的展示结构。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxExceptionView {
    pub key: String,
    /// `marked_for_reversal` | `marked_for_roll_forward`
    pub kind: String,
}

/// 演示用授权端口：固定由"主管 T0101"批准。
///
/// 接 IAM 后替换为真实授权（主管刷卡/指纹），届时 `Authorizer` 的实现会去调远端。
struct DemoSupervisor;

impl Authorizer for DemoSupervisor {
    fn authorize(&self, _request: &ReversalRequest) -> Authorization {
        Authorization {
            approver: "T0101".to_string(),
            granted: true,
            reason: "演示：主管授权".to_string(),
        }
    }
}

/// 两阶段提交的上下文：放锁之后仍然需要的值，**不含任何 `&AppState` 引用**。
///
/// 为什么只带值：阶段二（打主机）期间状态锁是放开的，下一个命令可能已经换了菜单、
/// 锁了屏、甚至登了出；这时再回头读状态字段，审计就会记错人、记错账。
pub struct SubmitContext {
    /// 幂等键。阶段一占住"在途"位，阶段三释放——期间同一笔再来会被拒。
    key: String,
    tx_code: String,
    amount_cents: i64,
    actor: String,
    /// 包身份前缀（`pkg=... `），非包调用时为空。审计里要能看出是谁发起的。
    origin: String,
}

/// 已通过校验、可以打主机的提交。
pub struct PreparedSubmit {
    context: SubmitContext,
    request: TxRequest,
    signature: String,
    remote: Arc<dyn RemoteBanking>,
}

impl PreparedSubmit {
    /// 阶段二：打主机。**不碰任何锁**——拆两阶段的理由就是这个（P0-10）。
    ///
    /// 这期间幂等键仍占着"在途"位：同一笔再提交会被 `begin_submit` 直接拒绝。
    pub fn call_host(&self) -> Result<String, HostError> {
        let host: &dyn TxHost = self.remote.as_ref();
        host.submit(&self.request)
    }
}

/// 提交阶段一的结果。
pub enum SubmitPrep {
    /// 可以打主机。
    Proceed(PreparedSubmit),
    /// 不必打主机：结论已落账并写完审计（重复提交、幂等键冲突、在途、校验失败）。
    Done(TxResult),
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
        // 会话锁只短暂持有：审计要拿柜员号、要写账本，这两件事都不能在持锁时做，
        // 否则交易会和每秒一次的锁屏 tick 互相等。
        let denial = match self.state.session.lock() {
            Ok(slot) => slot.denial(),
            Err(_) => Some(("会话状态异常", "会话锁已损坏，请重启客户端".to_string())),
        };
        match denial {
            None => Ok(()),
            Some((reason, detail)) => {
                let actor = self.actor();
                self.audit(&actor, action, "", "denied", &detail);
                Err(format!("{reason}：{detail}。请回到锁屏界面解锁后再试"))
            }
        }
    }

    // ---------- 启动与菜单 ----------

    /// 启动：本地包优先，业务包合并，网络只在本地不可用时兜底。
    pub fn boot(&mut self) -> Result<BootSummary, String> {
        self.state.store.clean_tmp();

        let started = std::time::Instant::now();
        let outcome = sync::boot_load(self.state.remote.as_ref(), &self.state.store)
            .map_err(|e| e.to_string())?;
        let load_ms = started.elapsed().as_secs_f64() * 1000.0;

        let mut tree: MenuTree = outcome.loaded.tree;
        let menu_version = tree.menu_version;
        let pack_bytes = outcome.loaded.bytes;
        let source = match outcome.loaded.source {
            LoadSource::LocalFile { .. } => "local".to_string(),
            LoadSource::NetworkFallback { .. } => "network".to_string(),
        };

        // 业务包：扫描 → 验签 → 依赖拓扑 → 合并菜单
        let mut notes = outcome.notes;
        let pkg_dir = std::path::Path::new(&self.state.appdata_dir).join("packages");
        let (plan, failures) = pkg::scan_dir(
            &pkg_dir,
            &self.state.verifying_key,
            crate::ports::shell_version(),
        );
        let conflicts = pkg::merge_packages(&mut tree, &plan.ordered);
        self.state.packages = plan.ordered;

        for package in &self.state.packages {
            notes.push(format!(
                "已加载业务包 {} v{}",
                package.manifest.id, package.manifest.version
            ));
            self.audit(
                "system",
                "package_load",
                &package.manifest.id,
                "ok",
                &format!("v{}", package.manifest.version),
            );
        }
        for failure in &failures {
            notes.push(format!("跳过业务包 {}：{}", failure.id, failure.reason));
            self.audit(
                "system",
                "package_load",
                &failure.id,
                "failed",
                &failure.reason.to_string(),
            );
        }
        for conflict in &conflicts {
            notes.push(format!("业务包菜单冲突，保留主壳版本：{conflict}"));
        }
        if let Some(w) = &self.state.journal_warning {
            notes.push(w.clone());
        }

        let menu_nodes = tree.nodes.len();
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

    /// 登录：取快照 → 本地剪枝 → 建会话。全程无网络往返（mock 除外）。
    /// 登录。所有失败（未知角色 / 主机拒绝 / 菜单未就绪）统一计入安全事件审计（S-15/T-13）。
    pub fn login(&mut self, role_id: &str) -> Result<LoginOutcome, String> {
        let result = self.try_login(role_id);
        if let Err(reason) = &result {
            self.record_login_failure(role_id, reason);
        }
        result
    }

    /// 累计次数以审计日志为准，不在进程内另存一份会漂移的计数。
    fn failed_login_count(&self) -> usize {
        self.state
            .audit
            .as_ref()
            .and_then(|journal| journal.audit_tail(usize::MAX).ok())
            .map(|events| {
                events
                    .iter()
                    .filter(|e| e.action == "login" && e.result == "failed")
                    .count()
            })
            .unwrap_or(0)
    }

    fn record_login_failure(&self, role_id: &str, reason: &str) {
        let count = self.failed_login_count() + 1;
        self.audit(
            role_id,
            "login",
            "",
            "failed",
            &format!("累计第 {count} 次登录失败：{reason}"),
        );
    }

    fn try_login(&mut self, role_id: &str) -> Result<LoginOutcome, String> {
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
        let menu = filter::filter_visible(menu_tree, &snapshot);
        let filter_ms = started.elapsed().as_secs_f64() * 1000.0;

        self.state.snapshot = Some(snapshot.clone());
        self.state
            .session
            .lock()
            .map_err(|_| "会话状态已损坏，请重启客户端".to_string())?
            .begin(&snapshot.teller_no);
        self.audit(
            &snapshot.teller_no,
            "login",
            &snapshot.outlet_code,
            "ok",
            &format!("角色 {}", snapshot.role_label),
        );

        Ok(LoginOutcome {
            snapshot,
            menu,
            filter_ms,
        })
    }

    pub fn logout(&mut self) -> Result<(), String> {
        if let Some(snapshot) = self.state.snapshot.clone() {
            self.audit(&snapshot.teller_no, "logout", "", "ok", "");
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
        Ok(filter::filter_visible(menu, snapshot))
    }

    /// 后台版本探测。菜单版本更新不重置权限快照——两者独立演进。
    pub fn check_update(&mut self) -> Result<sync::UpdateOutcome, String> {
        let outcome = sync::check_and_update(self.state.remote.as_ref(), &self.state.store);
        if outcome.updated {
            if let Ok(loaded) = self.state.store.load_latest() {
                self.state.menu = Some(loaded.tree);
            }
        }
        Ok(outcome)
    }

    /// 演示"本地包被篡改"后的回退：验签失败 → 服务端救回。
    pub fn reload_from_disk(&mut self) -> Result<ReloadOutcome, String> {
        let outcome = sync::reload(self.state.remote.as_ref(), &self.state.store)
            .map_err(|e| e.to_string())?;
        let note = outcome.notes.join("；");
        let menu = self
            .state
            .snapshot
            .as_ref()
            .map(|snap| filter::filter_visible(&outcome.loaded.tree, snap));
        let menu_version = outcome.loaded.tree.menu_version;
        let source = match outcome.loaded.source {
            LoadSource::LocalFile { .. } => "local".to_string(),
            LoadSource::NetworkFallback { .. } => "network".to_string(),
        };
        self.state.menu = Some(outcome.loaded.tree);

        Ok(ReloadOutcome {
            menu_version,
            source,
            verify_failed: outcome.verify_failed,
            note,
            menu,
        })
    }

    // ---------- 交易 ----------

    /// 状态不明的交易：标记待冲正。这是超时后的**唯一**正确处置。
    fn mark_if_uncertain(&mut self, key: &str, result: &TxResult) {
        if result.status != TxStatus::MarkedForReversal {
            return;
        }
        let kind = ExceptionKind::MarkedForReversal;
        self.state.reversal.mark(key, kind);
        // 状态不明跨重启必须还在：标记内存成功但落盘失败时，也要让柜员看见告警。
        if let Some(journal) = self.state.audit.as_deref() {
            if let Err(e) = journal.save_exception(key, exception_kind_str(kind)) {
                self.audit(&self.actor(), "exception", key, "failed", &e);
            }
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
        // 冲正要素必须从原交易回捞，不能让柜员凭记忆填；账本里查不到就拒绝冲正。
        let (original, lookup_error) = match self.state.audit.as_deref() {
            Some(journal) => match journal.tx_request(key) {
                Ok(request) => (request, None),
                Err(e) => (None, Some(e)),
            },
            None => (None, Some("账本不可用".to_string())),
        };
        let original = match original {
            Some(request) => request,
            None => {
                if let Some(e) = lookup_error {
                    self.audit(&actor, "reversal", key, "failed", &e);
                }
                return Err(format!("找不到原交易 {key} 的落盘要素，无法冲正；请先查账"));
            }
        };
        let request = ReversalRequest {
            idempotency_key: key.to_string(),
            initiator: actor.clone(),
            tx_code: original.tx_code,
            amount_cents: original.amount_cents,
            reason: reason.to_string(),
        };

        // 1. 授权（当前是演示主管；接 IAM 后换成真实授权端口）
        let record = self
            .state
            .reversal
            .request(&request, &DemoSupervisor)
            .map_err(|e| e.to_string())?;
        if let Some(journal) = self.state.audit.as_deref() {
            if let Err(e) = journal.save_reversal(&record) {
                return Err(format!("冲正申请未能落盘，已中止：{e}"));
            }
        }

        // 2. 主机受理。失败时保留"已授权未确认"，柜员可以重试
        let host_reference = self
            .remote
            .reverse(key, reason)
            .map_err(|e| e.to_string())?;

        // 3. 主机回了流水号才算处置完成
        self.state
            .reversal
            .confirm(key, &host_reference)
            .map_err(|e| e.to_string())?;
        if let Some(journal) = self.state.audit.as_deref() {
            if let Some(record) = self.state.reversal.reversal_of(key).cloned() {
                if let Err(e) = journal.save_reversal(&record) {
                    return Err(format!("冲正确认未能落盘，主机已受理：{e}；请立即查账"));
                }
            }
        }

        self.audit_checked(&actor, "reversal", key, "ok", &host_reference)?;
        Ok(host_reference)
    }

    /// 提交阶段一（锁内）：会话闸门、审计写前留痕、包能力与主机契约校验、
    /// 写前意图落盘 + 占住"在途"位。
    ///
    /// 产物里没有任何状态引用，所以调用方可以放掉锁去打主机（`commands` 就是这么做的）。
    pub fn prepare_submit(&mut self, mut request: TxRequest) -> Result<SubmitPrep, String> {
        self.require_unlocked("submit_tx")?;
        self.require_auditable("submit_tx", &request.tx_code)?;
        let identity = match PackageCall::take_from(&mut request.elements) {
            Ok(identity) => identity,
            Err(message) => {
                self.audit(
                    &self.actor(),
                    "package_capability",
                    &request.tx_code,
                    "denied",
                    &message,
                );
                return Err(message);
            }
        };
        if let Some(call) = &identity {
            self.authorize_package_call(call, &request.tx_code, None, request.offline_capable)?;
        }
        // 契约 lifecycle.md 不变量 7：主机没声明幂等，就不许离线挂起。
        // 挂起意味着"以后一定会重发"，而重发只有服务端按幂等键去重才不会重复入账。
        if request.offline_capable && !self.remote.supports_idempotency() {
            let actor = self.actor();
            let code = request.tx_code.clone();
            self.audit(
                &actor,
                "submit_tx",
                &code,
                "failed",
                "主机未声明幂等，拒绝离线挂起",
            );
            return Err(
                "该主机未声明幂等支持，不能离线挂起交易（请联系管理员确认主机契约）".to_string(),
            );
        }

        let context = SubmitContext {
            key: request.idempotency_key.clone(),
            tx_code: request.tx_code.clone(),
            amount_cents: request.amount_cents,
            actor: self.actor(),
            origin: identity
                .as_ref()
                .map(|call| format!("pkg={} ", call.label()))
                .unwrap_or_default(),
        };

        let signature = match self.state.tx.begin_submit(&request) {
            SubmitStart::Proceed { signature } => signature,
            SubmitStart::ShortCircuited(result) => {
                // 没有主机调用，但结论照样要落审计、照样要标待冲正——口径与走完主机一致。
                return Ok(SubmitPrep::Done(self.conclude(&context, result)?));
            }
        };
        Ok(SubmitPrep::Proceed(PreparedSubmit {
            context,
            request,
            signature,
            remote: Arc::clone(&self.remote),
        }))
    }

    /// 提交阶段三（重新上锁后）：把主机结论落成账，并写结论审计。
    pub fn complete_submit(
        &mut self,
        prepared: PreparedSubmit,
        outcome: Result<String, HostError>,
    ) -> Result<TxResult, String> {
        let result = self
            .state
            .tx
            .finish_submit(&prepared.request, &prepared.signature, outcome);
        self.conclude(&prepared.context, result)
    }

    /// 结论收口：待冲正标记 + 结论审计。
    ///
    /// 短路路径与走完主机的路径共用这里，避免两条路写出不同的审计口径。
    fn conclude(&mut self, context: &SubmitContext, result: TxResult) -> Result<TxResult, String> {
        self.mark_if_uncertain(&context.key, &result);
        let detail = format!(
            "{}status={:?} amount_cents={}",
            context.origin, result.status, context.amount_cents
        );
        self.audit_checked(
            &context.actor,
            "submit_tx",
            &context.tx_code,
            if result.status == TxStatus::Committed {
                "ok"
            } else {
                "failed"
            },
            &detail,
        )?;
        Ok(result)
    }

    /// 锁内一次做完提交（测试、重放与旧调用点用）。
    ///
    /// 命令层不走这条路：它要的是"两阶段 + 中间放锁"，
    /// 见 `commands::submit_transaction`。
    pub fn submit_transaction(&mut self, request: TxRequest) -> Result<TxResult, String> {
        match self.prepare_submit(request)? {
            SubmitPrep::Done(result) => Ok(result),
            SubmitPrep::Proceed(prepared) => {
                let outcome = prepared.call_host();
                self.complete_submit(prepared, outcome)
            }
        }
    }

    /// 重放进度写回：先删已不在调度表的键，再 upsert 当前进度。
    ///
    /// 顺序很重要：先删后写，避免"重放成功后写回失败"时残留旧 attempts，
    /// 下次启动又把已经冲正的交易当成未放弃。
    fn persist_replay_attempts(&self) {
        let Some(journal) = self.state.audit.as_deref() else {
            return;
        };
        let before = match journal.load_attempts() {
            Ok(rows) => rows,
            Err(e) => {
                self.audit(&self.actor(), "replay", "tx_queue", "failed", &e);
                return;
            }
        };
        let current = self.state.replay.snapshot();
        for old in before {
            if !current.iter().any(|a| a.key == old.key) {
                if let Err(e) = journal.remove_attempt(&old.key) {
                    self.audit(&self.actor(), "replay", "tx_queue", "failed", &e);
                }
            }
        }
        for attempt in current {
            if let Err(e) = journal.save_attempt(&attempt) {
                self.audit(&self.actor(), "replay", "tx_queue", "failed", &e);
            }
        }
    }

    pub fn replay_pending(&mut self) -> Result<String, String> {
        self.require_unlocked("replay")?;
        let remote = Arc::clone(&self.remote);
        let now = self.clock.now_secs();
        let run = tellerkit_core::replay::run_once(
            &mut self.state.tx,
            &mut self.state.replay,
            remote.as_ref(),
            now,
        );
        self.persist_replay_attempts();
        let journal_error = self.state.tx.take_journal_error();
        let actor = self.actor();

        if let Some(err) = journal_error {
            self.audit(&actor, "replay", "tx_queue", "failed", &err);
        }
        if run.attempted == 0 && run.still_pending == 0 {
            return Ok("没有待重放的交易".to_string());
        }
        if run.attempted == 0 {
            return Ok(format!("还有 {} 笔挂起，均未到重试时间", run.still_pending));
        }

        let detail = format!(
            "attempted={} committed={} pending={} gave_up={}",
            run.attempted,
            run.committed,
            run.still_pending,
            run.gave_up.len()
        );
        self.audit(&actor, "replay", "tx_queue", "ok", &detail);

        Ok(format!(
            "重放 {} 笔：成功 {} 笔，仍挂起 {} 笔{}",
            run.attempted,
            run.committed,
            run.still_pending,
            if run.gave_up.is_empty() {
                String::new()
            } else {
                format!("，{} 笔已放弃自动重试，需人工处理", run.gave_up.len())
            }
        ))
    }

    pub fn tx_pending_count(&self) -> usize {
        self.state.tx.pending_requests().len()
    }

    // ---------- 设备 ----------

    pub fn print_voucher(&mut self, mut voucher: Voucher) -> Result<PrintReceipt, String> {
        self.require_unlocked("print_voucher")?;
        let identity = match PackageCall::take_from(&mut voucher.fields) {
            Ok(identity) => identity,
            Err(message) => {
                self.audit(
                    &self.actor(),
                    "package_capability",
                    &voucher.tx_code,
                    "denied",
                    &message,
                );
                return Err(message);
            }
        };
        if let Some(call) = &identity {
            self.authorize_package_call(call, &voucher.tx_code, Some(PKG_DEVICE_PRINTER), false)?;
        }
        // 白名单先于动作：未登记的型号连纸都不该动
        self.state
            .devices
            .ensure_allowed(&self.state.policy)
            .map_err(|e| e.to_string())?;
        let tx_code = voucher.tx_code.clone();
        let template = voucher.template.clone();
        let outcome = self
            .state
            .devices
            .printer
            .print(&voucher)
            .map_err(|e| e.to_string());
        let actor = self.actor();
        let detail = match &identity {
            Some(call) => format!("pkg={} · {template}", call.label()),
            None => template.clone(),
        };
        self.audit(
            &actor,
            "print",
            &tx_code,
            if outcome.is_ok() { "ok" } else { "failed" },
            &detail,
        );
        outcome
    }

    // ---------- 会话 ----------
    //
    // 这三个方法是"用例视角"的薄包装，供本文件测试与内部复用；真正的 IPC 入口
    // 不经过 `AppState` 锁（见 `commands::session_*`），否则慢交易会拖住锁屏判定。

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

    /// 所有包调用的真实放行点：先认包，再按清单校验交易码/设备/离线能力。
    /// 拒绝一律走既有审计通道，`detail` 里带包身份，便于事后追溯"谁发起的"。
    fn authorize_package_call(
        &self,
        call: &PackageCall,
        tx_code: &str,
        device: Option<&str>,
        offline_capable: bool,
    ) -> Result<(), String> {
        let package = match self
            .state
            .packages
            .iter()
            .find(|p| p.manifest.id == call.id)
        {
            Some(package) => package,
            None => {
                let detail = format!("来源 {} 的包未加载或验签未通过", call.label());
                self.audit(
                    &self.actor(),
                    "package_capability",
                    tx_code,
                    "denied",
                    &detail,
                );
                return Err(format!("{detail}，拒绝调用"));
            }
        };
        match package.authorize_call(&call.source, &call.code, tx_code, device, offline_capable) {
            Ok(_) => Ok(()),
            Err(err) => {
                let detail = format!("{} 调用 {}：{}", call.label(), tx_code, err);
                self.audit(
                    &self.actor(),
                    "package_capability",
                    tx_code,
                    "denied",
                    &detail,
                );
                Err(format!("业务包能力校验未通过：{detail}"))
            }
        }
    }

    /// 返回模块源码，并在最前面写入由 Rust 生成的身份注释。
    /// 包代码自身无法伪造这一行：它在包代码之前，而且是 JS 注释。
    pub fn package_module(&self, route: &str) -> Result<String, String> {
        self.require_unlocked("package_module")?;
        for package in &self.state.packages {
            if let Some(module) = package.routes.get(route) {
                let source = package.assets.get(module).cloned().ok_or_else(|| {
                    format!("包 {} 的模块 {module} 未随包提供", package.manifest.id)
                })?;
                let menu_code = package
                    .menus
                    .iter()
                    .find(|node| node.route.as_deref() == Some(route))
                    .map(|node| node.code.clone())
                    .unwrap_or_else(|| package.manifest.id.clone());
                let identity = serde_json::json!({
                    "source": format!("pkg:{}", package.manifest.id),
                    "code": menu_code,
                });
                return Ok(format!("{PKG_IDENTITY_HEADER}{identity}\n{source}"));
            }
        }
        Err("该路由不是业务包提供的".to_string())
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
                    status: r.status,
                    count: r.count,
                    amount_cents: r.amount_cents,
                })
                .collect(),
            recent: recent
                .into_iter()
                .map(|r| RecentTxView {
                    key: r.idempotency_key,
                    tx_code: r.tx_code,
                    amount_cents: r.amount_cents,
                    status: r.status,
                    reference: r.reference,
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
                    e.at,
                    e.actor,
                    e.action,
                    e.target,
                    e.result,
                    if e.detail.is_empty() {
                        String::new()
                    } else {
                        format!(" · {}", e.detail)
                    }
                )
            })
            .collect())
    }

    /// S-09：资金类操作触达主机**之前**的闸门。
    /// 先证明"审计能写进去"——账本打不开或写前留痕失败就拒绝，绝不只打印日志后继续成交。
    /// 这里不跑 `audit_verify()` 全表校验：那是 O(n) 扫描，放进每笔交易会退化成 O(n²)
    /// （见架构评审 C 类）；链头一致性由 `journal.audit()` 的写事务内读取保证，
    /// 全量篡改检测属于启动自检与日终对账，不在本闸门。
    fn require_auditable(&self, action: &str, target: &str) -> Result<(), String> {
        self.state.audit.as_ref().ok_or_else(|| {
            format!("审计账本不可用，已拒绝 {action}（未触达主机）：请检查账本文件与磁盘后重启终端")
        })?;
        self.audit_write(&self.actor(), action, target, "attempt", "写前留痕")
            .map_err(|e| format!("审计写前留痕失败，已拒绝 {action}（未触达主机）：{e}"))
    }

    /// 真正的落盘动作，错误可传递。供闸门与结论审计共用；`at` 由账本覆盖，业务层不能伪造时间。
    fn audit_write(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) -> Result<(), String> {
        let journal = self
            .state
            .audit
            .as_ref()
            .ok_or_else(|| "审计日志不可用：账本未能打开".to_string())?;
        let event = AuditEvent {
            actor: actor.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            detail: detail.to_string(),
            at: String::new(),
        };
        journal.audit(&event)
    }

    /// S-09：资金类操作的结论审计必须能落盘。写失败就返回可读错误，
    /// 不能像普通日志那样吞掉后当成成交成功——业务已发生，明确要求柜员查账。
    fn audit_checked(
        &self,
        actor: &str,
        action: &str,
        target: &str,
        result: &str,
        detail: &str,
    ) -> Result<(), String> {
        self.audit_write(actor, action, target, result, detail)
            .map_err(|e| format!("审计结论写入失败：{e}；业务已发生，请立即查账，不要重复提交"))
    }

    /// 记一条审计。写不进去不影响业务，但要暴露出来（审计坏了不能静默）。
    fn audit(&self, actor: &str, action: &str, target: &str, result: &str, detail: &str) {
        if let Err(e) = self.audit_write(actor, action, target, result, detail) {
            eprintln!("审计写入失败：{e}");
        }
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
            .map(|s| s.teller_no.clone())
            .unwrap_or_else(|| "unknown".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

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
        let dir = std::env::temp_dir().join(format!("tk-app-test-{}", std::process::id()));
        let store =
            tellerkit_core::store::MenuStore::new(dir, tellerkit_core::mock::demo_verifying_key());
        let mock = Arc::new(crate::ports::MockRemote::default());
        let mut state = AppState::new(
            store,
            "unused".into(),
            TxEngine::new(),
            None,
            Some(Arc::new(
                tellerkit_journal_sqlite::SqliteJournal::in_memory().unwrap(),
            )),
            crate::ports::RemoteChoice {
                remote: Arc::clone(&mock) as Arc<dyn RemoteBanking>,
                mock: Some(mock),
            },
            tellerkit_core::mock::demo_verifying_key(),
        );
        // 时钟要同时注入 AppState 与会话槽：锁屏判定用的是会话槽那一份。
        state.set_clock(clock);
        state
            .session
            .lock()
            .expect("测试里会话锁不应中毒")
            .begin("T0000");
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

    /// P0-10 第 2 步的验收点：锁屏判定不能排在交易后面。
    ///
    /// 这里模拟"交易正持着 AppState 锁打主机"——`held` 就是那把锁。若会话仍是
    /// AppState 的字段，下面的 `try_lock` 必然 WouldBlock；拆成独立锁后两把锁互不相干。
    /// 用 `try_lock` 而不是 `lock`：真回归时测试要报错，而不是把 CI 挂死。
    #[test]
    fn session_tick_does_not_wait_for_app_state_lock() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let state = state_with(Arc::clone(&clock));
        let session = Arc::clone(&state.session);
        let shared = std::sync::Mutex::new(state);

        // 交易侧：持 AppState 锁不放（相当于一笔 3s 的主机调用）。
        let held = shared.lock().expect("测试里 AppState 锁不应中毒");
        clock.set(180);

        let mut slot = session
            .try_lock()
            .expect("会话槽必须与 AppState 分开，否则锁屏 tick 会排在交易后面");
        let view = slot.tick();
        assert!(view.locked, "注入 180 秒后应锁定");
        assert_eq!(view.reason.as_deref(), Some("无操作超时"));

        drop(slot);
        drop(held);
    }

    /// P0-10 第 2b 步的验收点：主机调用发生在锁**外**。
    ///
    /// 阶段一返回后锁必须已经放掉——否则"网络调用不持全局锁"只是句口号。
    /// 这里用 `try_lock` 断言而不是"看代码"：真回归时会红，而不是把 CI 挂死。
    #[test]
    fn host_call_between_phases_does_not_hold_the_app_state_lock() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let state = state_with(clock);
        let shared = std::sync::Mutex::new(state);

        let prepared = {
            let mut guard = shared.lock().expect("测试里 AppState 锁不应中毒");
            match ShellApp::new(&mut guard)
                .prepare_submit(TxRequest::new("account.deposit", "k-two-phase"))
                .expect("阶段一不应失败")
            {
                SubmitPrep::Proceed(prepared) => prepared,
                SubmitPrep::Done(result) => panic!("首笔应放行，实际：{}", result.message),
            }
        };

        // 阶段一结束后锁已释放：锁屏 tick、对账、审计查询都能正常拿锁。
        assert!(
            shared.try_lock().is_ok(),
            "主机调用期间不能还握着 AppState 锁"
        );

        // 阶段二：不持锁打主机；阶段三：重新加锁落结论。
        let outcome = prepared.call_host();
        let mut guard = shared.lock().expect("测试里 AppState 锁不应中毒");
        let result = ShellApp::new(&mut guard)
            .complete_submit(prepared, outcome)
            .expect("阶段三不应失败");
        assert_eq!(result.status, TxStatus::Committed);
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
        req.offline_capable = true;
        let err = ShellApp::new(&mut state)
            .submit_transaction(req)
            .unwrap_err();
        assert!(err.contains("未声明幂等"), "实际：{err}");

        // 在线模式不走这条拦截：会一路到端点，端点不存在则是"主机拒绝"（交易结论），
        // 不是 Err——状态机把失败表达成 TxResult，这一点不能被测试写反。
        let mut online = TxRequest::new("account.deposit", "k-online");
        online.offline_capable = false;
        let result = ShellApp::new(&mut state)
            .submit_transaction(online)
            .expect("在线模式不应被幂等规则拦下");
        assert_eq!(result.status, TxStatus::Rejected);
        assert!(
            result.message.contains("没有为该服务登记任何端点"),
            "实际：{}",
            result.message
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
        state.session.lock().expect("测试里会话锁不应中毒").end();

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
        assert_eq!(result.status, TxStatus::Committed);
    }

    fn identity_fields() -> BTreeMap<String, String> {
        let mut fields = BTreeMap::new();
        fields.insert(PKG_IDENTITY_SOURCE_KEY.into(), "pkg:biz.demo".into());
        fields.insert(PKG_IDENTITY_CODE_KEY.into(), "biz.demo.home".into());
        fields
    }

    fn demo_package_with(devices: Vec<String>) -> tellerkit_core::BizPackage {
        tellerkit_core::BizPackage {
            manifest: tellerkit_core::BizManifest {
                id: "biz.demo".into(),
                name: "示例包".into(),
                version: "0.1.0".into(),
                min_shell: "0.1.0".into(),
                max_shell: "0.x".into(),
                deps: vec![],
                entry: "ui/entry.js".into(),
            },
            menus: vec![tellerkit_core::MenuNode::page(
                "biz.demo.home",
                None,
                "示例",
                0,
            )],
            permissions: tellerkit_core::pkg::Permissions::default(),
            routes: BTreeMap::new(),
            transactions: vec![tellerkit_core::pkg::TxMeta {
                code: "biz.demo.hello".into(),
                idempotency: String::new(),
                offline_capable: false,
                devices,
            }],
            assets: BTreeMap::new(),
        }
    }

    #[test]
    fn package_call_checks_manifest_declared_capability() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);
        state
            .packages
            .push(demo_package_with(vec!["printer".into()]));

        let mut allowed = TxRequest::new("biz.demo.hello", "k-pkg-ok");
        allowed.elements = identity_fields();
        assert!(ShellApp::new(&mut state)
            .submit_transaction(allowed)
            .is_ok());

        let mut undeclared = TxRequest::new("biz.other.pay", "k-pkg-bad");
        undeclared.elements = identity_fields();
        let err = ShellApp::new(&mut state)
            .submit_transaction(undeclared)
            .unwrap_err();
        assert!(err.contains("未声明交易码"), "实际：{err}");

        let mut offline = TxRequest::new("biz.demo.hello", "k-pkg-offline");
        offline.elements = identity_fields();
        offline.offline_capable = true;
        let err = ShellApp::new(&mut state)
            .submit_transaction(offline)
            .unwrap_err();
        assert!(err.contains("未声明离线能力"), "实际：{err}");

        let mut voucher = Voucher::new("voucher", "biz.demo.hello");
        voucher.fields = identity_fields();
        assert!(ShellApp::new(&mut state).print_voucher(voucher).is_ok());

        // 设备声明为空时打印被默认拒绝：清单没写的能力不能靠调用方补上。
        let mut no_device = demo_package_with(vec![]);
        no_device.transactions[0].code = "biz.demo.print".into();
        state.packages.clear();
        state.packages.push(no_device);
        let mut voucher = Voucher::new("voucher", "biz.demo.print");
        voucher.fields = identity_fields();
        let err = ShellApp::new(&mut state)
            .print_voucher(voucher)
            .unwrap_err();
        assert!(err.contains("未声明设备能力"), "实际：{err}");
    }

    #[test]
    fn package_identity_is_validated_before_dispatch() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(clock);

        let mut missing = TxRequest::new("biz.demo.hello", "k-no-package");
        missing
            .elements
            .insert(PKG_IDENTITY_SOURCE_KEY.into(), "pkg:biz.missing".into());
        missing
            .elements
            .insert(PKG_IDENTITY_CODE_KEY.into(), "biz.missing.home".into());
        let err = ShellApp::new(&mut state)
            .submit_transaction(missing)
            .unwrap_err();
        assert!(err.contains("未加载或验签未通过"), "实际：{err}");

        let mut partial = TxRequest::new("biz.demo.hello", "k-partial");
        partial
            .elements
            .insert(PKG_IDENTITY_SOURCE_KEY.into(), "pkg:biz.demo".into());
        let err = ShellApp::new(&mut state)
            .submit_transaction(partial)
            .unwrap_err();
        assert!(err.contains("身份不完整"), "实际：{err}");

        let mut malformed = TxRequest::new("biz.demo.hello", "k-malformed");
        malformed
            .elements
            .insert(PKG_IDENTITY_SOURCE_KEY.into(), "biz.demo".into());
        malformed
            .elements
            .insert(PKG_IDENTITY_CODE_KEY.into(), "biz.demo.home".into());
        let err = ShellApp::new(&mut state)
            .submit_transaction(malformed)
            .unwrap_err();
        assert!(err.contains("身份格式非法"), "实际：{err}");
    }

    /// S-09/T-12：账本不可用时，资金类操作在触达主机**之前**被拒绝，
    /// 返回的是可读错误，不是"打印日志后继续成交"。
    #[test]
    fn missing_audit_refuses_fund_operations_before_host() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        state.audit = None;

        let err = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-no-audit"))
            .unwrap_err();
        assert!(err.contains("审计账本不可用"), "实际：{err}");
        assert!(err.contains("未触达主机"), "实际：{err}");

        let err = ShellApp::new(&mut state)
            .request_reversal("k-no-audit", "冲正")
            .unwrap_err();
        assert!(err.contains("审计账本不可用"), "实际：{err}");
        assert!(err.contains("未触达主机"), "实际：{err}");
    }

    /// 闸门不是"一律拒绝"：账本可用时交易照常到主机，写前留痕 + 结论两条都在。
    #[test]
    fn healthy_audit_records_attempt_and_conclusion() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        let result = ShellApp::new(&mut state)
            .submit_transaction(TxRequest::new("account.deposit", "k-audited"))
            .expect("账本可用时应放行到主机");
        assert_eq!(result.status, TxStatus::Committed);

        let tail = ShellApp::new(&mut state).audit_tail(20).unwrap();
        let entries = tail
            .iter()
            .filter(|line| line.contains("submit_tx"))
            .count();
        assert_eq!(entries, 2, "应有 attempt 与结论两条：{tail:?}");
        assert!(
            tail.iter().any(|line| line.contains("attempt")),
            "实际：{tail:?}"
        );
    }

    /// S-15/T-13：每次登录失败都留痕，且带累计次数。
    #[test]
    fn login_failures_are_audited_with_cumulative_count() {
        let clock = Arc::new(FrozenClock(AtomicU64::new(0)));
        let mut state = state_with(Arc::clone(&clock));
        for _ in 0..3 {
            assert!(ShellApp::new(&mut state).login("not-a-role").is_err());
        }
        let tail = ShellApp::new(&mut state).audit_tail(20).unwrap();
        let failed: Vec<_> = tail.iter().filter(|line| line.contains("login")).collect();
        assert_eq!(failed.len(), 3, "三次失败逐条可见：{tail:?}");
        for n in 1..=3 {
            assert!(
                failed
                    .iter()
                    .any(|l| l.contains(&format!("累计第 {n} 次登录失败"))),
                "缺第 {n} 次：{tail:?}"
            );
        }
    }
}

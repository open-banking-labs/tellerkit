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
use tellerkit_core::session::{SessionGuard, SessionPolicy};
use tellerkit_core::sync;
use tellerkit_core::tx::{TxRequest, TxResult, TxStatus};
use tellerkit_core::{filter, LoadSource};
use tellerkit_journal_sqlite::AuditEvent;

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
                self.audit(&actor, action, "", "denied", detail);
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
        }
        for failure in &failures {
            notes.push(format!("跳过业务包 {}：{}", failure.id, failure.reason));
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
    pub fn login(&mut self, role_id: &str) -> Result<LoginOutcome, String> {
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
        self.state.session = Some(SessionGuard::new(
            SessionPolicy::default(),
            self.clock.now_secs(),
        ));
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
        self.state.session = None;
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

        self.audit(&actor, "reversal", key, "ok", &host_reference);
        Ok(host_reference)
    }

    pub fn submit_transaction(&mut self, mut request: TxRequest) -> Result<TxResult, String> {
        self.require_unlocked("submit_tx")?;
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
        let amount_cents = request.amount_cents;
        let code = request.tx_code.clone();
        let key = request.idempotency_key.clone();
        let actor = self.actor();

        let result = self.state.tx.submit(self.remote.as_ref(), request);
        self.mark_if_uncertain(&key, &result);

        let origin = identity
            .as_ref()
            .map(|call| format!("pkg={} ", call.label()))
            .unwrap_or_default();
        let detail = format!(
            "{origin}status={:?} amount_cents={amount_cents}",
            result.status
        );
        self.audit(
            &actor,
            "submit_tx",
            &code,
            if result.status == TxStatus::Committed {
                "ok"
            } else {
                "failed"
            },
            &detail,
        );
        Ok(result)
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
            self.audit(&actor, "session_lock", "", "ok", reason.label());
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
            self.audit(&actor, "session_lock", "", "ok", reason.label());
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

    /// 记一条审计。写不进去不影响业务，但要暴露出来（审计坏了不能静默）。
    fn audit(&self, actor: &str, action: &str, target: &str, result: &str, detail: &str) {
        let Some(journal) = &self.state.audit else {
            return;
        };
        let event = AuditEvent {
            actor: actor.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            detail: detail.to_string(),
            at: String::new(),
        };
        if let Err(e) = journal.audit(&event) {
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
            None,
            crate::ports::RemoteChoice {
                remote: Arc::clone(&mock) as Arc<dyn RemoteBanking>,
                mock: Some(mock),
            },
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
}

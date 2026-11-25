//! L3 应用层：用例编排。
//!
//! 规则（docs/architecture.md §2）：
//! - 只依赖领域（`tellerkit-core`）与[端口](crate::ports)，**不知道** Tauri、不知道 DTO；
//! - 这里写"先做什么、再做什么、失败怎么降级"；
//! - 命令层（`commands.rs`）只负责加锁 → 调这里 → 映射成 IPC DTO。
//!
//! 为什么不用"一个用例一个 struct"：没有第二个实现，结构体只是把函数换了个形状
//! （见 §6 的三问结论）。

use std::sync::Arc;

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::filter::MenuView;
use tellerkit_core::mock::DemoRole;
use tellerkit_core::model::MenuTree;
use tellerkit_core::pkg;
use tellerkit_core::session::{SessionGuard, SessionPolicy};
use tellerkit_core::sync;
use tellerkit_core::tx::{TxRequest, TxResult, TxStatus};
use tellerkit_core::{filter, LoadSource};
use tellerkit_journal_sqlite::AuditEvent;

use crate::ports::{Clock, RemoteBanking};
use crate::AppState;

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
            &tellerkit_core::mock::demo_verifying_key(),
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
            pubkey_hex: tellerkit_core::mock::demo_pubkey_hex(),
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

    pub fn submit_transaction(&mut self, request: TxRequest) -> Result<TxResult, String> {
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
        let actor = self.actor();

        let result = self.state.tx.submit(self.remote.as_ref(), request);

        let detail = format!("status={:?} amount_cents={amount_cents}", result.status);
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

    pub fn replay_pending(&mut self) -> Result<String, String> {
        let remote = Arc::clone(&self.remote);
        let now = self.clock.now_secs();
        let run = tellerkit_core::replay::run_once(
            &mut self.state.tx,
            &mut self.state.replay,
            remote.as_ref(),
            now,
        );
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

    pub fn print_voucher(&mut self, voucher: Voucher) -> Result<PrintReceipt, String> {
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
        self.audit(
            &actor,
            "print",
            &tx_code,
            if outcome.is_ok() { "ok" } else { "failed" },
            &template,
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

    pub fn package_module(&self, route: &str) -> Result<String, String> {
        for package in &self.state.packages {
            if let Some(module) = package.routes.get(route) {
                return package.assets.get(module).cloned().ok_or_else(|| {
                    format!("包 {} 的模块 {module} 未随包提供", package.manifest.id)
                });
            }
        }
        Err("该路由不是业务包提供的".to_string())
    }

    // ---------- 审计 ----------

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
            Arc::clone(&mock) as Arc<dyn RemoteBanking>,
            Some(mock),
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
}

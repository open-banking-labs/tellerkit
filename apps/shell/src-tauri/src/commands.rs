//! L2 IPC 边界：**只做四件事**——搬参数、把阻塞工作挪出主线程、调用例、映射成 DTO。
//!
//! 编排在 [`crate::app::ShellApp`]；决策在 `tellerkit-core`。
//! 这里出现 `if 断网 { ... }` 之类的分支就是分层被破坏（docs/architecture.md §9）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::filter::MenuView;
use tellerkit_core::tx::{TxRequest, TxResult};
use tellerkit_core::PermSnapshot;

use crate::app::{LoginOutcome, SessionSlot, ShellApp, SubmitPrep};
use crate::{AppState, SharedSession};

/// 托管状态的共享句柄。
///
/// 命令是 `async` 的，阻塞工作要搬进 `spawn_blocking`，而闭包必须 `'static`：
/// `State<'_, T>` 借的是 `AppHandle`，搬不进闭包，所以先克隆一个 `Arc` 再 move。
type SharedState = Arc<Mutex<AppState>>;

fn lock(state: &Mutex<AppState>) -> Result<MutexGuard<'_, AppState>, String> {
    let guard = state
        .lock()
        .map_err(|_| "应用状态已损坏，请重启客户端".to_string())?;
    // 启动被拒绝时连 boot 在内一律失败：界面只能停在错误页，
    // 不能让"降级成功的演示远端"被当成真实通道用。
    guard.startup_fatal.check()?;
    Ok(guard)
}

/// 在阻塞线程池里持锁执行一段编排，结果搬回异步上下文。
///
/// 为什么必须这样：Tauri 2 的**同步**命令在主线程执行，而这里每条命令都可能
/// 读账本、验签菜单包、或按 3s 超时打主机——放在主线程就是界面冻结
/// （架构评审 A-03）。改成 `async` 后主线程只做参数搬运与错误文案翻译。
///
/// 锁在闭包内部获取、由调用方只拿 `&mut AppState`，是为了让
/// `MutexGuard` 不可能跨 `await` 存活：跨了就是编译错误，不靠人记。
async fn with_state<T, F>(state: &State<'_, SharedState>, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut AppState) -> Result<T, String> + Send + 'static,
{
    let shared = Arc::clone(state.inner());
    blocking(move || {
        let mut guard = lock(&shared)?;
        work(&mut guard)
    })
    .await
}

/// 会话命令的统一入口：**只锁会话槽**，不碰 `AppState` 锁。
///
/// 这是 P0-10 第 2 步的落点：`session_tick` 每秒一次，如果还要等交易那把锁，
/// 一笔 3s 的主机调用就能让锁屏判定停摆 3 秒。
async fn with_session<T, F>(session: &State<'_, SharedSession>, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut SessionSlot) -> T + Send + 'static,
{
    let shared = Arc::clone(session.inner());
    blocking(move || {
        let mut slot = shared
            .lock()
            .map_err(|_| "会话状态已损坏，请重启客户端".to_string())?;
        Ok(work(&mut slot))
    })
    .await
}

/// 把阻塞工作交给 Tokio 阻塞线程池，结果搬回异步上下文。
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("后台任务异常退出：{e}"))?
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LoadReport {
    #[ts(type = "number")]
    pub menu_version: u64,
    /// `local` = 本地文件命中；`network` = 回退服务端拉取。
    pub source: String,
    pub menu_nodes: usize,
    pub pack_bytes: usize,
    /// 读文件 + 验签 + 反序列化耗时（毫秒）。
    pub load_ms: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BootInfo {
    pub shell_version: String,
    pub pubkey_hex: String,
    pub appdata_dir: String,
    pub offline: bool,
    #[ts(type = "Array<number>")]
    pub local_versions: Vec<u64>,
    pub load: LoadReport,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LoginResult {
    pub snapshot: PermSnapshot,
    pub menu: Vec<MenuView>,
    pub filter_ms: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct UpdateReport {
    pub checked: bool,
    #[ts(type = "number")]
    pub local: u64,
    #[ts(type = "number")]
    pub latest: u64,
    pub updated: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReloadReport {
    #[ts(type = "number")]
    pub menu_version: u64,
    pub source: String,
    pub recovered: bool,
    pub verify_failed: bool,
    pub note: String,
    pub menu: Option<Vec<MenuView>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DevState {
    pub offline: bool,
    pub tampered: bool,
    #[ts(type = "number")]
    pub server_version: u64,
    #[ts(type = "Array<number>")]
    pub local_versions: Vec<u64>,
}

/// 会话状态。锁屏是授权状态的一部分，只能由 Rust 持有。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SessionState {
    pub locked: bool,
    pub reason: Option<String>,
    #[ts(type = "number")]
    pub idle_secs: u64,
}

impl From<crate::app::SessionView> for SessionState {
    fn from(v: crate::app::SessionView) -> Self {
        Self {
            locked: v.locked,
            reason: v.reason,
            idle_secs: v.idle_secs,
        }
    }
}

/// 待处置的异常交易（状态不明，需查账/冲正）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TxException {
    pub key: String,
    /// `marked_for_reversal` | `marked_for_roll_forward`
    pub kind: String,
}

#[tauri::command]
pub async fn tx_exceptions(state: State<'_, SharedState>) -> Result<Vec<TxException>, String> {
    with_state(&state, |app| {
        Ok(ShellApp::new(app)
            .tx_exceptions()
            .into_iter()
            .map(|v| TxException {
                key: v.key,
                kind: v.kind,
            })
            .collect())
    })
    .await
}

/// 申请冲正：本地授权 → 主机受理 → 返回主机流水号。
///
/// 与提交交易同样是三段式：主机受理那一下不持 `AppState` 锁。
#[tauri::command]
pub async fn request_reversal(
    key: String,
    reason: String,
    state: State<'_, SharedState>,
) -> Result<String, String> {
    let shared = Arc::clone(state.inner());
    blocking(move || {
        // 阶段一（锁内）：会话闸门、写前审计、回捞原交易要素、本地授权并落盘。
        let prepared = {
            let mut guard = lock(&shared)?;
            ShellApp::new(&mut guard).prepare_reversal(&key, &reason)?
        };

        // 阶段二（**不持锁**）：向主机申请冲正。这期间该笔是"已授权未确认"，
        // 并发的第二次冲正会在阶段一被拒——不需要额外的在途表。
        let host_reference = prepared.call_host()?;

        // 阶段三（重新加锁）：落主机回执。拿不到锁也要说清"主机可能已受理"。
        let mut guard =
            lock(&shared).map_err(|e| format!("{e}；主机可能已受理冲正，请立即查账确认"))?;
        ShellApp::new(&mut guard).complete_reversal(&prepared, &host_reference)
    })
    .await
}

/// 对账聚合行。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StatusRow {
    pub status: String,
    #[ts(type = "number")]
    pub count: i64,
    #[ts(type = "number")]
    pub amount_cents: i64,
}

/// 最近一笔交易（对账明细）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RecentTx {
    pub key: String,
    pub tx_code: String,
    #[ts(type = "number")]
    pub amount_cents: i64,
    pub status: String,
    pub reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Reconciliation {
    pub since: String,
    pub summary: Vec<StatusRow>,
    pub recent: Vec<RecentTx>,
}

/// 日终对账：只读本地账本，不打主机。
#[tauri::command]
pub async fn tx_reconciliation(state: State<'_, SharedState>) -> Result<Reconciliation, String> {
    with_state(&state, |app| {
        let view = ShellApp::new(app).reconciliation()?;
        Ok(Reconciliation {
            since: view.since,
            summary: view
                .summary
                .into_iter()
                .map(|r| StatusRow {
                    status: r.status,
                    count: r.count,
                    amount_cents: r.amount_cents,
                })
                .collect(),
            recent: view
                .recent
                .into_iter()
                .map(|r| RecentTx {
                    key: r.key,
                    tx_code: r.tx_code,
                    amount_cents: r.amount_cents,
                    status: r.status,
                    reference: r.reference,
                })
                .collect(),
        })
    })
    .await
}

fn dev_state_of(st: &AppState) -> DevState {
    DevState {
        offline: st.mock.as_ref().map(|m| m.offline()).unwrap_or(false),
        tampered: st.mock.as_ref().map(|m| m.tampered()).unwrap_or(false),
        server_version: st.mock.as_ref().map(|m| m.menu_version()).unwrap_or(0),
        local_versions: st.store.list_versions(),
    }
}

/// 演示开关只在 mock 模式下有意义：接入真实端点后没有"离线开关"。
/// 演示开关只在开发构建可用。
///
/// 正式构建里暴露"断网/篡改本地包"这类开关，等于给攻击者一个现成的降级按钮。
fn dev_only() -> Result<(), String> {
    if cfg!(debug_assertions) {
        Ok(())
    } else {
        Err("演示开关只在开发构建中可用".to_string())
    }
}

fn mock_handle(st: &AppState) -> Result<&Arc<crate::ports::MockRemote>, String> {
    st.mock
        .as_ref()
        .ok_or_else(|| "当前已接入真实端点，演示开关不可用".to_string())
}

// ---------------- 启动与菜单 ----------------

#[tauri::command]
pub async fn boot(state: State<'_, SharedState>) -> Result<BootInfo, String> {
    with_state(&state, |app| {
        let summary = ShellApp::new(app).boot()?;
        Ok(BootInfo {
            shell_version: summary.shell_version,
            pubkey_hex: summary.pubkey_hex,
            appdata_dir: summary.appdata_dir,
            offline: summary.offline,
            local_versions: summary.local_versions,
            load: LoadReport {
                menu_version: summary.menu_version,
                source: summary.source,
                menu_nodes: summary.menu_nodes,
                pack_bytes: summary.pack_bytes,
                load_ms: summary.load_ms,
            },
            notes: summary.notes,
        })
    })
    .await
}

#[tauri::command]
pub async fn login(role_id: String, state: State<'_, SharedState>) -> Result<LoginResult, String> {
    with_state(&state, move |app| {
        let LoginOutcome {
            snapshot,
            menu,
            filter_ms,
        } = ShellApp::new(app).login(&role_id)?;
        Ok(LoginResult {
            snapshot,
            menu,
            filter_ms,
        })
    })
    .await
}

#[tauri::command]
pub async fn logout(state: State<'_, SharedState>) -> Result<(), String> {
    with_state(&state, |app| ShellApp::new(app).logout()).await
}

#[tauri::command]
pub async fn current_menu(state: State<'_, SharedState>) -> Result<Vec<MenuView>, String> {
    with_state(&state, |app| ShellApp::new(app).current_menu()).await
}

#[tauri::command]
pub async fn check_update(state: State<'_, SharedState>) -> Result<UpdateReport, String> {
    with_state(&state, |app| {
        let outcome = ShellApp::new(app).check_update()?;
        Ok(UpdateReport {
            checked: outcome.checked,
            local: outcome.local.unwrap_or(0),
            latest: outcome.latest.unwrap_or(0),
            updated: outcome.updated,
            note: outcome.note,
        })
    })
    .await
}

#[tauri::command]
pub async fn reload_from_disk(state: State<'_, SharedState>) -> Result<ReloadReport, String> {
    with_state(&state, |app| {
        let outcome = ShellApp::new(app).reload_from_disk()?;
        Ok(ReloadReport {
            menu_version: outcome.menu_version,
            source: outcome.source,
            recovered: outcome.verify_failed,
            verify_failed: outcome.verify_failed,
            note: outcome.note,
            menu: outcome.menu,
        })
    })
    .await
}

// ---------------- 交易 ----------------

#[tauri::command]
pub async fn submit_transaction(
    tx_code: String,
    idempotency_key: String,
    amount_cents: i64,
    elements: BTreeMap<String, String>,
    offline_capable: bool,
    state: State<'_, SharedState>,
) -> Result<TxResult, String> {
    let shared = Arc::clone(state.inner());
    blocking(move || {
        let request = TxRequest {
            tx_code,
            idempotency_key,
            amount_cents,
            elements,
            offline_capable,
        };

        // 阶段一（锁内）：会话闸门、审计写前留痕、包能力/主机契约校验、
        // 写前意图落盘，并占住该幂等键的"在途"位。
        let prep = {
            let mut guard = lock(&shared)?;
            ShellApp::new(&mut guard).prepare_submit(request)?
        };
        let prepared = match prep {
            SubmitPrep::Done(result) => return Ok(result),
            SubmitPrep::Proceed(prepared) => prepared,
        };

        // 阶段二（**不持锁**）：主机调用。这一行就是 P0-10 的"网络调用不持全局锁"。
        // 期间别的命令（锁屏 tick、对账、审计查询）可以正常拿锁；
        // 同一笔再来会被阶段一已占的在途位拒绝。
        let outcome = prepared.call_host();

        // 阶段三（重新加锁）：落结论 + 写结论审计。
        // 拿不到锁也必须说清"钱可能已经动了"，不能只报一句状态已损坏。
        let mut guard = lock(&shared)
            .map_err(|e| format!("{e}；主机调用可能已经受理，请立即查账确认，不要重复提交"))?;
        ShellApp::new(&mut guard).complete_submit(prepared, outcome)
    })
    .await
}

#[tauri::command]
pub async fn print_voucher(
    template: String,
    tx_code: String,
    reference: Option<String>,
    fields: BTreeMap<String, String>,
    state: State<'_, SharedState>,
) -> Result<PrintReceipt, String> {
    with_state(&state, move |app| {
        let mut voucher = Voucher::new(&template, &tx_code);
        voucher.reference = reference;
        voucher.fields = fields;
        ShellApp::new(app).print_voucher(voucher)
    })
    .await
}

/// 手动重放一轮：同样是三段式——重放是"多笔 × 3s 超时"，
/// 整段持锁会把柜员操作挡住 N×3 秒。
#[tauri::command]
pub async fn replay_pending(state: State<'_, SharedState>) -> Result<String, String> {
    let shared = Arc::clone(state.inner());
    blocking(move || {
        let prepared = {
            let mut guard = lock(&shared)?;
            ShellApp::new(&mut guard).prepare_replay()?
        };
        // 阶段二（不持锁）：逐笔打主机。
        let outcomes = prepared.call_hosts();
        let mut guard =
            lock(&shared).map_err(|e| format!("{e}；重放可能已部分提交，请查账确认"))?;
        ShellApp::new(&mut guard).complete_replay(prepared, outcomes)
    })
    .await
}

#[tauri::command]
pub async fn tx_pending_count(state: State<'_, SharedState>) -> Result<usize, String> {
    with_state(&state, |app| Ok(ShellApp::new(app).tx_pending_count())).await
}

#[tauri::command]
pub async fn package_module(
    route: String,
    state: State<'_, SharedState>,
) -> Result<String, String> {
    with_state(&state, move |app| ShellApp::new(app).package_module(&route)).await
}

// ---------------- 审计与会话 ----------------

#[tauri::command]
pub async fn audit_tail(
    limit: usize,
    state: State<'_, SharedState>,
) -> Result<Vec<String>, String> {
    with_state(&state, move |app| ShellApp::new(app).audit_tail(limit)).await
}

#[tauri::command]
pub async fn session_touch(session: State<'_, SharedSession>) -> Result<SessionState, String> {
    with_session(&session, |slot| slot.touch().into()).await
}

#[tauri::command]
pub async fn session_tick(session: State<'_, SharedSession>) -> Result<SessionState, String> {
    with_session(&session, |slot| slot.tick().into()).await
}

#[tauri::command]
pub async fn session_blur(session: State<'_, SharedSession>) -> Result<SessionState, String> {
    with_session(&session, |slot| slot.blur().into()).await
}

// ---------------- 演示开关（不发生在真实逻辑里） ----------------

#[tauri::command]
pub async fn dev_set_offline(
    offline: bool,
    state: State<'_, SharedState>,
) -> Result<DevState, String> {
    dev_only()?;
    with_state(&state, move |app| {
        mock_handle(app)?.set_offline(offline);
        Ok(dev_state_of(app))
    })
    .await
}

#[tauri::command]
pub async fn dev_set_tampered(
    tampered: bool,
    state: State<'_, SharedState>,
) -> Result<DevState, String> {
    dev_only()?;
    with_state(&state, move |app| {
        mock_handle(app)?.set_tampered(tampered);
        Ok(dev_state_of(app))
    })
    .await
}

#[tauri::command]
pub async fn dev_bump_server_version(state: State<'_, SharedState>) -> Result<DevState, String> {
    dev_only()?;
    with_state(&state, |app| {
        mock_handle(app)?.bump_menu_version();
        Ok(dev_state_of(app))
    })
    .await
}

#[tauri::command]
pub async fn dev_corrupt_local(state: State<'_, SharedState>) -> Result<String, String> {
    dev_only()?;
    with_state(&state, |app| {
        let version = app.store.current_version().ok_or("本地没有菜单包可篡改")?;
        let path = app.store.pack_path(version);
        let mut raw = std::fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
        if raw.len() < 8 {
            return Err("文件太小".into());
        }
        let idx = raw.len() / 2;
        raw[idx] ^= 0xFF;
        std::fs::write(&path, &raw).map_err(|e| format!("写入失败：{e}"))?;
        Ok(format!(
            "已篡改 {}（翻转第 {} 字节），下次加载应验签失败",
            path.display(),
            idx
        ))
    })
    .await
}

#[tauri::command]
pub async fn dev_reset_local(state: State<'_, SharedState>) -> Result<DevState, String> {
    dev_only()?;
    with_state(&state, |app| {
        for version in app.store.list_versions() {
            let _ = std::fs::remove_file(app.store.pack_path(version));
        }
        Ok(dev_state_of(app))
    })
    .await
}

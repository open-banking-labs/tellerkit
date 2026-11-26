//! L2 IPC 边界：搬参数 → 把阻塞工作挪出主线程 → 调用例 → 映射成 DTO。
//!
//! 编排在 [`crate::app::ShellApp`]；决策在 `tellerkit-core`。
//! 这里出现 `if 断网 { ... }` 之类的分支就是分层被破坏（docs/architecture.md §9）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::tx::{TxRequest, TxResult};
use tellerkit_core::MenuView;
use tellerkit_core::PermSnapshot;

use crate::app::{SessionSlot, ShellApp};
use crate::{AppState, SharedSession, SharedState};

/// 所有命令取状态都得过这里：启动失败在唯一入口拦一次。
fn lock(state: &Mutex<AppState>) -> Result<MutexGuard<'_, AppState>, String> {
    let guard = state
        .lock()
        .map_err(|_| "应用状态已损坏，请重启客户端".to_string())?;
    // 启动被拒绝时连 boot 在内一律失败：界面只能停在错误页，
    // 不能让降级成功的演示远端被当成真实通道用。
    guard.startup_fatal.check()?;
    Ok(guard)
}

/// 把阻塞工作交给 Tokio 阻塞线程池；Tauri 2 的同步命令跑主线程，
/// 打主机/读账本放在主线程就是界面冻结。
async fn blocking<T, F>(work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| format!("后台任务异常退出：{e}"))?
}

/// 在阻塞线程池里执行一段不与托管状态打交道的工作（三段式的放锁窗口）。
async fn with_shared<T, F>(state: &State<'_, SharedState>, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&SharedState) -> Result<T, String> + Send + 'static,
{
    let shared = Arc::clone(state.inner());
    blocking(move || work(&shared)).await
}

/// 持锁执行一次编排；`MutexGuard` 只活在闭包里，不可能跨 `await`。
async fn with_state<T, F>(state: &State<'_, SharedState>, work: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&mut AppState) -> Result<T, String> + Send + 'static,
{
    with_shared(state, move |shared| {
        let mut guard = lock(shared)?;
        work(&mut guard)
    })
    .await
}

/// 会话命令只锁会话槽：锁屏 tick 不能排在慢交易后面。
/// 锁序固定 `AppState` → `SessionSlot`，这里不碰状态锁就不会反向。
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
        slot.check_startup()?;
        Ok(work(&mut slot))
    })
    .await
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LoadReport {
    #[ts(type = "number")]
    menu_version: u64,
    /// `local` = 本地文件命中；`network` = 回退服务端拉取。
    source: String,
    menu_nodes: usize,
    pack_bytes: usize,
    /// 读文件 + 验签 + 反序列化耗时（毫秒）。
    load_ms: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct BootInfo {
    shell_version: String,
    pubkey_hex: String,
    appdata_dir: String,
    offline: bool,
    #[ts(type = "Array<number>")]
    local_versions: Vec<u64>,
    load: LoadReport,
    notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LoginResult {
    snapshot: PermSnapshot,
    menu: Vec<MenuView>,
    filter_ms: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct UpdateReport {
    checked: bool,
    #[ts(type = "number")]
    local: u64,
    #[ts(type = "number")]
    latest: u64,
    updated: bool,
    note: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ReloadReport {
    #[ts(type = "number")]
    menu_version: u64,
    source: String,
    recovered: bool,
    verify_failed: bool,
    note: String,
    menu: Option<Vec<MenuView>>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct DevState {
    offline: bool,
    tampered: bool,
    #[ts(type = "number")]
    server_version: u64,
    #[ts(type = "Array<number>")]
    local_versions: Vec<u64>,
}

/// 会话状态。锁屏是授权状态的一部分，只能由 Rust 持有。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SessionState {
    locked: bool,
    reason: Option<String>,
    #[ts(type = "number")]
    idle_secs: u64,
}

impl From<crate::app::SessionView> for SessionState {
    fn from(v: crate::app::SessionView) -> Self {
        Self {
            locked: v.locked(),
            reason: v.reason().clone(),
            idle_secs: v.idle_secs(),
        }
    }
}

/// 待处置的异常交易（状态不明，需查账/冲正）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct TxException {
    key: String,
    /// `marked_for_reversal` | `marked_for_roll_forward`
    kind: String,
}

#[tauri::command]
pub async fn tx_exceptions(state: State<'_, SharedState>) -> Result<Vec<TxException>, String> {
    with_state(&state, |app| {
        Ok(ShellApp::new(app)
            .tx_exceptions()
            .into_iter()
            .map(|v| TxException {
                key: v.key().to_string(),
                kind: v.kind().to_string(),
            })
            .collect())
    })
    .await
}

/// 申请冲正：本地授权 → 主机受理 → 返回主机流水号。
#[tauri::command]
pub async fn request_reversal(
    key: String,
    reason: String,
    state: State<'_, SharedState>,
) -> Result<String, String> {
    with_shared(&state, move |shared| {
        crate::app::request_reversal_with(shared, &key, &reason)
    })
    .await
}

/// 对账聚合行。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct StatusRow {
    status: String,
    #[ts(type = "number")]
    count: i64,
    #[ts(type = "number")]
    amount_cents: i64,
}

/// 最近一笔交易（对账明细）。
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RecentTx {
    key: String,
    tx_code: String,
    #[ts(type = "number")]
    amount_cents: i64,
    status: String,
    reference: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct Reconciliation {
    since: String,
    summary: Vec<StatusRow>,
    recent: Vec<RecentTx>,
}

/// 日终对账：只读本地账本，不打主机。
#[tauri::command]
pub async fn tx_reconciliation(state: State<'_, SharedState>) -> Result<Reconciliation, String> {
    with_state(&state, |app| {
        let view = ShellApp::new(app).reconciliation()?;
        Ok(Reconciliation {
            since: view.since().to_string(),
            summary: view
                .summary()
                .iter()
                .map(|r| StatusRow {
                    status: r.status().to_string(),
                    count: r.count(),
                    amount_cents: r.amount_cents(),
                })
                .collect(),
            recent: view
                .recent()
                .iter()
                .map(|r| RecentTx {
                    key: r.key().to_string(),
                    tx_code: r.tx_code().to_string(),
                    amount_cents: r.amount_cents(),
                    status: r.status().to_string(),
                    reference: r.reference().clone(),
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

/// 演示开关只在 mock 模式下有意义；正式构建里暴露这些开关等于给攻击者降级按钮。
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
            shell_version: summary.shell_version().to_string(),
            pubkey_hex: summary.pubkey_hex().to_string(),
            appdata_dir: summary.appdata_dir().to_string(),
            offline: summary.offline(),
            local_versions: summary.local_versions().to_vec(),
            load: LoadReport {
                menu_version: summary.menu_version(),
                source: summary.source().to_string(),
                menu_nodes: summary.menu_nodes(),
                pack_bytes: summary.pack_bytes(),
                load_ms: summary.load_ms(),
            },
            notes: summary.notes().to_vec(),
        })
    })
    .await
}

#[tauri::command]
pub async fn login(role_id: String, state: State<'_, SharedState>) -> Result<LoginResult, String> {
    with_state(&state, move |app| {
        let outcome = ShellApp::new(app).login(&role_id)?;
        Ok(LoginResult {
            snapshot: outcome.snapshot().clone(),
            menu: outcome.menu().to_vec(),
            filter_ms: outcome.filter_ms(),
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
    let outcome = with_shared(&state, crate::app::check_update_with).await?;
    Ok(UpdateReport {
        checked: outcome.checked(),
        local: outcome.local().unwrap_or(0),
        latest: outcome.latest().unwrap_or(0),
        updated: outcome.updated(),
        note: outcome.note().to_string(),
    })
}

#[tauri::command]
pub async fn reload_from_disk(state: State<'_, SharedState>) -> Result<ReloadReport, String> {
    with_state(&state, |app| {
        let outcome = ShellApp::new(app).reload_from_disk()?;
        Ok(ReloadReport {
            menu_version: outcome.menu_version(),
            source: outcome.source().to_string(),
            recovered: outcome.verify_failed(),
            verify_failed: outcome.verify_failed(),
            note: outcome.note().to_string(),
            menu: outcome.menu().clone(),
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
    with_shared(&state, move |shared| {
        crate::app::submit_transaction_with(
            shared,
            TxRequest::new(&tx_code, &idempotency_key)
                .amount(amount_cents)
                .with_elements(elements)
                .offline(offline_capable),
        )
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
        voucher.set_reference(reference);
        voucher.set_fields(fields);
        ShellApp::new(app).print_voucher(voucher)
    })
    .await
}

#[tauri::command]
pub async fn replay_pending(state: State<'_, SharedState>) -> Result<String, String> {
    with_shared(&state, crate::app::replay_pending_with).await
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
    with_state(&state, move |app| {
        let view = ShellApp::new(app).package_module(&route)?;
        // IPC 契约仍是 string：源码与能力装进一个字符串信封，避免改契约/生成物。
        serde_json::to_string(&view).map_err(|e| format!("业务包视图编码失败：{e}"))
    })
    .await
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

//! L2 IPC 边界：**只做三件事**——加锁、调用例、映射成 DTO。
//!
//! 编排在 [`crate::app::ShellApp`]；决策在 `tellerkit-core`。
//! 这里出现 `if 断网 { ... }` 之类的分支就是分层被破坏（docs/architecture.md §9）。

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::filter::MenuView;
use tellerkit_core::tx::{TxRequest, TxResult};
use tellerkit_core::PermSnapshot;

use crate::app::{LoginOutcome, ShellApp};
use crate::AppState;

fn lock<'a>(
    state: &'a State<'_, Mutex<AppState>>,
) -> Result<std::sync::MutexGuard<'a, AppState>, String> {
    let guard = state
        .lock()
        .map_err(|_| "应用状态已损坏，请重启客户端".to_string())?;
    // 启动被拒绝时连 boot 在内一律失败：界面只能停在错误页，
    // 不能让"降级成功的演示远端"被当成真实通道用。
    guard.startup_fatal.check()?;
    Ok(guard)
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
    key: String,
    /// `marked_for_reversal` | `marked_for_roll_forward`
    kind: String,
}

#[tauri::command]
pub fn tx_exceptions(state: State<'_, Mutex<AppState>>) -> Result<Vec<TxException>, String> {
    let mut guard = lock(&state)?;
    Ok(ShellApp::new(&mut guard)
        .tx_exceptions()
        .into_iter()
        .map(|v| TxException {
            key: v.key,
            kind: v.kind,
        })
        .collect())
}

/// 申请冲正：本地授权 → 主机受理 → 返回主机流水号。
#[tauri::command]
pub fn request_reversal(
    key: String,
    reason: String,
    state: State<'_, Mutex<AppState>>,
) -> Result<String, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).request_reversal(&key, &reason)
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
pub fn tx_reconciliation(state: State<'_, Mutex<AppState>>) -> Result<Reconciliation, String> {
    let mut guard = lock(&state)?;
    let view = ShellApp::new(&mut guard).reconciliation()?;
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

fn mock_handle(st: &AppState) -> Result<&std::sync::Arc<crate::ports::MockRemote>, String> {
    st.mock
        .as_ref()
        .ok_or_else(|| "当前已接入真实端点，演示开关不可用".to_string())
}

// ---------------- 启动与菜单 ----------------

#[tauri::command]
pub fn boot(state: State<'_, Mutex<AppState>>) -> Result<BootInfo, String> {
    let mut guard = lock(&state)?;
    let summary = ShellApp::new(&mut guard).boot()?;
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
}

#[tauri::command]
pub fn login(role_id: String, state: State<'_, Mutex<AppState>>) -> Result<LoginResult, String> {
    let mut guard = lock(&state)?;
    let LoginOutcome {
        snapshot,
        menu,
        filter_ms,
    } = ShellApp::new(&mut guard).login(&role_id)?;
    Ok(LoginResult {
        snapshot,
        menu,
        filter_ms,
    })
}

#[tauri::command]
pub fn logout(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).logout()
}

#[tauri::command]
pub fn current_menu(state: State<'_, Mutex<AppState>>) -> Result<Vec<MenuView>, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).current_menu()
}

#[tauri::command]
pub fn check_update(state: State<'_, Mutex<AppState>>) -> Result<UpdateReport, String> {
    let mut guard = lock(&state)?;
    let outcome = ShellApp::new(&mut guard).check_update()?;
    Ok(UpdateReport {
        checked: outcome.checked,
        local: outcome.local.unwrap_or(0),
        latest: outcome.latest.unwrap_or(0),
        updated: outcome.updated,
        note: outcome.note,
    })
}

#[tauri::command]
pub fn reload_from_disk(state: State<'_, Mutex<AppState>>) -> Result<ReloadReport, String> {
    let mut guard = lock(&state)?;
    let outcome = ShellApp::new(&mut guard).reload_from_disk()?;
    Ok(ReloadReport {
        menu_version: outcome.menu_version,
        source: outcome.source,
        recovered: outcome.verify_failed,
        verify_failed: outcome.verify_failed,
        note: outcome.note,
        menu: outcome.menu,
    })
}

// ---------------- 交易 ----------------

#[tauri::command]
pub fn submit_transaction(
    tx_code: String,
    idempotency_key: String,
    amount_cents: i64,
    elements: BTreeMap<String, String>,
    offline_capable: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<TxResult, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).submit_transaction(
        TxRequest::new(&tx_code, &idempotency_key)
            .amount(amount_cents)
            .with_elements(elements)
            .offline(offline_capable),
    )
}

#[tauri::command]
pub fn print_voucher(
    template: String,
    tx_code: String,
    reference: Option<String>,
    fields: BTreeMap<String, String>,
    state: State<'_, Mutex<AppState>>,
) -> Result<PrintReceipt, String> {
    let mut guard = lock(&state)?;
    let mut voucher = Voucher::new(&template, &tx_code);
    voucher.reference = reference;
    voucher.fields = fields;
    ShellApp::new(&mut guard).print_voucher(voucher)
}

#[tauri::command]
pub fn replay_pending(state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).replay_pending()
}

#[tauri::command]
pub fn tx_pending_count(state: State<'_, Mutex<AppState>>) -> Result<usize, String> {
    let mut guard = lock(&state)?;
    Ok(ShellApp::new(&mut guard).tx_pending_count())
}

#[tauri::command]
pub fn package_module(route: String, state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).package_module(&route)
}

// ---------------- 审计与会话 ----------------

#[tauri::command]
pub fn audit_tail(limit: usize, state: State<'_, Mutex<AppState>>) -> Result<Vec<String>, String> {
    let mut guard = lock(&state)?;
    ShellApp::new(&mut guard).audit_tail(limit)
}

#[tauri::command]
pub fn session_touch(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut guard = lock(&state)?;
    Ok(ShellApp::new(&mut guard).session_touch().into())
}

#[tauri::command]
pub fn session_tick(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut guard = lock(&state)?;
    Ok(ShellApp::new(&mut guard).session_tick().into())
}

#[tauri::command]
pub fn session_blur(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut guard = lock(&state)?;
    Ok(ShellApp::new(&mut guard).session_blur().into())
}

// ---------------- 演示开关（不发生在真实逻辑里） ----------------

#[tauri::command]
pub fn dev_set_offline(
    offline: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<DevState, String> {
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    let st = lock(&state)?;
    mock_handle(&st)?.set_offline(offline);
    Ok(dev_state_of(&st))
}

#[tauri::command]
pub fn dev_set_tampered(
    tampered: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<DevState, String> {
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    let st = lock(&state)?;
    mock_handle(&st)?.set_tampered(tampered);
    Ok(dev_state_of(&st))
}

#[tauri::command]
pub fn dev_bump_server_version(state: State<'_, Mutex<AppState>>) -> Result<DevState, String> {
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    dev_only()?;
    let st = lock(&state)?;
    mock_handle(&st)?.bump_menu_version();
    Ok(dev_state_of(&st))
}

#[tauri::command]
pub fn dev_corrupt_local(state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    dev_only()?;
    let st = lock(&state)?;
    let version = st.store.current_version().ok_or("本地没有菜单包可篡改")?;
    let path = st.store.pack_path(version);
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
}

#[tauri::command]
pub fn dev_reset_local(state: State<'_, Mutex<AppState>>) -> Result<DevState, String> {
    dev_only()?;
    let st = lock(&state)?;
    for version in st.store.list_versions() {
        let _ = std::fs::remove_file(st.store.pack_path(version));
    }
    Ok(dev_state_of(&st))
}

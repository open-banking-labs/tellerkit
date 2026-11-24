//! Tauri 命令层。
//!
//! 这一层刻意保持很薄：加锁、计时、把错误翻成中文、提供演示开关。
//! 所有决策逻辑都在 `tellerkit-core::sync` 与 `filter`，因此重要路径可以被单测覆盖。

use std::fs;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use tauri::State;

use tellerkit_core::filter::{filter_visible, MenuView};
use tellerkit_core::mock::DemoRole;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::sync;
use tellerkit_core::LoadSource;

use crate::AppState;

fn lock<'a>(
    state: &'a State<'_, Mutex<AppState>>,
) -> Result<std::sync::MutexGuard<'a, AppState>, String> {
    state
        .lock()
        .map_err(|_| "应用状态已损坏，请重启客户端".to_string())
}

#[derive(Debug, Clone, Serialize)]
pub struct LoadReport {
    pub menu_version: u64,
    /// `local` = 本地文件命中；`network` = 回退服务端拉取。
    pub source: String,
    pub menu_nodes: usize,
    pub pack_bytes: usize,
    /// 读文件 + 验签 + 反序列化耗时（毫秒）。
    pub load_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BootInfo {
    pub shell_version: String,
    pub pubkey_hex: String,
    pub appdata_dir: String,
    pub offline: bool,
    pub local_versions: Vec<u64>,
    pub load: LoadReport,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoginResult {
    pub snapshot: PermSnapshot,
    pub menu: Vec<MenuView>,
    /// 全量树递归剪枝耗时（毫秒），P0 验收要求 < 1ms 量级。
    pub filter_ms: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UpdateReport {
    pub checked: bool,
    pub local: Option<u64>,
    pub latest: Option<u64>,
    pub updated: bool,
    pub note: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReloadReport {
    pub menu_version: u64,
    pub source: String,
    pub recovered: bool,
    pub verify_failed: bool,
    pub note: String,
    pub menu: Option<Vec<MenuView>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DevState {
    pub offline: bool,
    pub tampered: bool,
    pub server_version: u64,
    pub local_versions: Vec<u64>,
}

fn source_label(source: &LoadSource) -> String {
    match source {
        LoadSource::LocalFile { .. } => "local".to_string(),
        LoadSource::NetworkFallback { .. } => "network".to_string(),
    }
}

fn dev_state_of(st: &AppState) -> DevState {
    DevState {
        offline: st.host.offline,
        tampered: st.host.serve_tampered,
        server_version: st.host.menu_version,
        local_versions: st.store.list_versions(),
    }
}

/// 启动：本地包优先，网络只在"本地不可用"时兜底。这是 P0 的核心路径。
#[tauri::command]
pub fn boot(state: State<'_, Mutex<AppState>>) -> Result<BootInfo, String> {
    let mut st = lock(&state)?;
    st.store.clean_tmp();

    let started = Instant::now();
    let outcome = sync::boot_load(&st.host, &st.store).map_err(|e| e.to_string())?;
    let load_ms = started.elapsed().as_secs_f64() * 1000.0;

    let report = LoadReport {
        menu_version: outcome.loaded.tree.menu_version,
        source: source_label(&outcome.loaded.source),
        menu_nodes: outcome.loaded.tree.nodes.len(),
        pack_bytes: outcome.loaded.bytes,
        load_ms,
    };
    st.menu = Some(outcome.loaded.tree);
    st.last_load = Some(report.clone());

    Ok(BootInfo {
        shell_version: env!("CARGO_PKG_VERSION").to_string(),
        pubkey_hex: tellerkit_core::mock::demo_pubkey_hex(),
        appdata_dir: st.appdata_dir.clone(),
        offline: st.host.offline,
        local_versions: st.store.list_versions(),
        load: report,
        notes: outcome.notes,
    })
}

/// 登录：拿权限快照 → 本地剪枝 → 返回可见菜单。全程无网络。
#[tauri::command]
pub fn login(role_id: String, state: State<'_, Mutex<AppState>>) -> Result<LoginResult, String> {
    let mut st = lock(&state)?;
    let role = DemoRole::from_id(&role_id).ok_or("未知角色")?;
    // 权限版本与菜单版本独立演进：这里用角色序号造一个稳定值。
    let role_index = DemoRole::ALL.iter().position(|r| *r == role).unwrap_or(0) as u64;
    let perm_version = st.host.menu_version * 100 + role_index;
    let snapshot = st
        .host
        .login(&role_id, perm_version)
        .ok_or("服务端拒绝了该角色的登录")?;

    let menu = st.menu.as_ref().ok_or("菜单尚未加载，请先完成启动流程")?;
    let started = Instant::now();
    let view = filter_visible(menu, &snapshot);
    let filter_ms = started.elapsed().as_secs_f64() * 1000.0;

    st.snapshot = Some(snapshot.clone());
    Ok(LoginResult {
        snapshot,
        menu: view,
        filter_ms,
    })
}

/// 登出：立刻丢弃权限快照，防止下一个账号看到上一个账号的菜单。
#[tauri::command]
pub fn logout(state: State<'_, Mutex<AppState>>) -> Result<(), String> {
    let mut st = lock(&state)?;
    st.clear_session();
    Ok(())
}

/// 当前会话的可见菜单（前端重新渲染时用）。
#[tauri::command]
pub fn current_menu(state: State<'_, Mutex<AppState>>) -> Result<Vec<MenuView>, String> {
    let st = lock(&state)?;
    let snapshot = st.snapshot.as_ref().ok_or("尚未登录")?;
    let menu = st.menu.as_ref().ok_or("菜单尚未加载")?;
    Ok(filter_visible(menu, snapshot))
}

/// 渲染之后的"后台"版本探测。不阻塞首屏，不一致才下载新包。
#[tauri::command]
pub fn check_update(state: State<'_, Mutex<AppState>>) -> Result<UpdateReport, String> {
    let mut st = lock(&state)?;
    let outcome = sync::check_and_update(&st.host, &st.store);

    if outcome.updated {
        if let Ok(loaded) = st.store.load_latest() {
            st.menu = Some(loaded.tree);
        }
    }

    Ok(UpdateReport {
        checked: outcome.checked,
        local: outcome.local,
        latest: outcome.latest,
        updated: outcome.updated,
        note: outcome.note,
    })
}

/// 演示"本地文件被篡改"后的回退路径：验签失败 → 服务端拉取 → 恢复。
#[tauri::command]
pub fn reload_from_disk(state: State<'_, Mutex<AppState>>) -> Result<ReloadReport, String> {
    let mut st = lock(&state)?;
    let outcome = sync::reload(&st.host, &st.store).map_err(|e| e.to_string())?;
    let note = outcome.notes.join("；");
    let menu = st
        .snapshot
        .as_ref()
        .map(|snap| filter_visible(&outcome.loaded.tree, snap));
    let version = outcome.loaded.tree.menu_version;
    let source = source_label(&outcome.loaded.source);
    st.menu = Some(outcome.loaded.tree);

    Ok(ReloadReport {
        menu_version: version,
        source,
        recovered: outcome.verify_failed,
        verify_failed: outcome.verify_failed,
        note,
        menu,
    })
}

// ---- 演示开关：只影响 mock 宿主，不改变真实逻辑 ----

#[tauri::command]
pub fn dev_set_offline(
    offline: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<DevState, String> {
    let mut st = lock(&state)?;
    st.host.offline = offline;
    Ok(dev_state_of(&st))
}

#[tauri::command]
pub fn dev_set_tampered(
    tampered: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<DevState, String> {
    let mut st = lock(&state)?;
    st.host.serve_tampered = tampered;
    Ok(dev_state_of(&st))
}

#[tauri::command]
pub fn dev_bump_server_version(state: State<'_, Mutex<AppState>>) -> Result<DevState, String> {
    let mut st = lock(&state)?;
    st.host.menu_version += 1;
    Ok(dev_state_of(&st))
}

/// 模拟"有人用十六进制编辑器改了本地 .mtree"。
#[tauri::command]
pub fn dev_corrupt_local(state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let st = lock(&state)?;
    let version = st.store.current_version().ok_or("本地没有菜单包可篡改")?;
    let path = st.store.pack_path(version);
    let mut raw = fs::read(&path).map_err(|e| format!("读取失败：{e}"))?;
    if raw.len() < 8 {
        return Err("文件太小".into());
    }
    let idx = raw.len() / 2;
    raw[idx] ^= 0xFF;
    fs::write(&path, &raw).map_err(|e| format!("写入失败：{e}"))?;
    Ok(format!(
        "已篡改 {}（翻转第 {} 字节），下次加载应验签失败",
        path.display(),
        idx
    ))
}

/// 删除本地所有菜单包，模拟"全新终端首次启动"。
#[tauri::command]
pub fn dev_reset_local(state: State<'_, Mutex<AppState>>) -> Result<DevState, String> {
    let st = lock(&state)?;
    for version in st.store.list_versions() {
        let _ = fs::remove_file(st.store.pack_path(version));
    }
    Ok(dev_state_of(&st))
}

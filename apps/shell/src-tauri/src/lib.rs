//! TellerKit 主壳。
//!
//! 职责边界（PLAN.md §4）：
//! - Rust 侧持有唯一真相源：菜单树、权限快照、mock 宿主、本地菜单包仓库；
//! - 前端只拿"已经剪枝好的可见菜单"，不拿全量树，也不做权限判断；
//! - 启动路径上**没有**任何网络等待：本地包 → 验签 → 反序列化；
//!   版本探测是渲染之后的独立调用。

pub mod commands;

use std::sync::Mutex;

use tellerkit_core::mock::demo_verifying_key;
use tellerkit_core::mock::MockHost;
use tellerkit_core::model::MenuTree;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::store::MenuStore;

use tauri::Manager;

/// 应用状态。所有命令都通过 `Mutex<AppState>` 访问，避免 Rust/前端各持一份真相。
pub struct AppState {
    pub host: MockHost,
    pub store: MenuStore,
    /// 当前登录用户的权限快照。登出/切号必须置空。
    pub snapshot: Option<PermSnapshot>,
    /// 全量菜单树（不做权限过滤），供每次过滤复用。
    pub menu: Option<MenuTree>,
    pub last_load: Option<commands::LoadReport>,
    pub appdata_dir: String,
}

impl AppState {
    pub fn new(store: MenuStore, appdata_dir: String) -> Self {
        Self {
            host: MockHost::default(),
            store,
            snapshot: None,
            menu: None,
            last_load: None,
            appdata_dir,
        }
    }

    /// 清空权限快照。登出、切换账号、收到 401 时都必须调用。
    pub fn clear_session(&mut self) {
        self.snapshot = None;
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let appdata = app.path().app_data_dir()?;
            let menu_dir = appdata.join("menu");
            // P0：公钥内置在客户端；生产环境由构建配置注入，私钥永不进客户端。
            let store = MenuStore::new(menu_dir, demo_verifying_key());
            let state = AppState::new(store, appdata.display().to_string());
            app.manage(Mutex::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::boot,
            commands::login,
            commands::logout,
            commands::current_menu,
            commands::check_update,
            commands::reload_from_disk,
            commands::dev_set_offline,
            commands::dev_set_tampered,
            commands::dev_bump_server_version,
            commands::dev_corrupt_local,
            commands::dev_reset_local,
        ])
        .run(tauri::generate_context!())
        .expect("TellerKit 启动失败");
}

//! TellerKit 主壳。
//!
//! 职责边界（PLAN.md §4）：
//! - Rust 侧持有唯一真相源：菜单树、权限快照、mock 宿主、本地菜单包仓库；
//! - 前端只拿"已经剪枝好的可见菜单"，不拿全量树，也不做权限判断；
//! - 启动路径上**没有**任何网络等待：本地包 → 验签 → 反序列化；
//!   版本探测是渲染之后的独立调用。

pub mod commands;

use std::sync::Mutex;

use std::sync::Arc;

use tellerkit_core::device::{mock_devices, DevicePolicy, Devices};
use tellerkit_core::mock::demo_verifying_key;
use tellerkit_core::mock::MockHost;
use tellerkit_core::model::MenuTree;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::pkg::BizPackage;
use tellerkit_core::session::{SessionGuard, SessionPolicy};
use tellerkit_core::store::MenuStore;
use tellerkit_core::tx::{DemoTxHost, TxEngine};
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult};
use tellerkit_journal_sqlite::SqliteJournal;

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
    /// 账本打不开时的告警。柜面宁可"能开但提示异常"，也不要"打不开"。
    pub journal_warning: Option<String>,
    /// 交易引擎：幂等索引 + 待重放队列（P1.4 会落盘）。
    pub tx: TxEngine,
    /// 演示主机：可切换在线/离线/业务拒绝。
    pub tx_host: DemoTxHost,
    /// 设备集合：P1.2 的 mock 适配器，P3 换真机只换这一处构造。
    pub devices: Devices,
    /// 已加载的业务包（按依赖拓扑序）。前端按 route 找模块。
    pub packages: Vec<BizPackage>,
    /// 审计日志句柄。`None` 表示账本不可用（降级为内存模式）。
    pub audit: Option<Arc<SqliteJournal>>,
    /// 外设型号白名单。默认只允许 mock 型号——没登记就拒绝。
    pub policy: DevicePolicy,
    /// 会话锁屏状态。`None` 表示未登录。
    pub session: Option<SessionGuard>,
}

impl AppState {
    pub fn new(
        store: MenuStore,
        appdata_dir: String,
        tx: TxEngine,
        journal_warning: Option<String>,
        audit: Option<Arc<SqliteJournal>>,
    ) -> Self {
        Self {
            host: MockHost::default(),
            store,
            snapshot: None,
            menu: None,
            last_load: None,
            appdata_dir,
            journal_warning,
            tx,
            tx_host: DemoTxHost::default(),
            devices: mock_devices(),
            packages: Vec::new(),
            audit,
            policy: DevicePolicy::allow_mocks(),
            session: None,
        }
    }

    /// 清空权限快照。登出、切换账号、收到 401 时都必须调用。
    pub fn clear_session(&mut self) {
        self.snapshot = None;
    }
}

/// 让同一个 SQLite 连接既做交易账本、又做审计查询。
///
/// `TxEngine` 需要 `Box<dyn TxJournal>` 的所有权，而命令层还要按需查审计，
/// 所以用 Arc 共享一份连接——这是唯一不引入连接池的简单做法。
struct ArcJournal(Arc<SqliteJournal>);

impl TxJournal for ArcJournal {
    fn record(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        self.0.record(request, result)
    }
    fn enqueue(&self, request: &TxRequest) -> Result<(), String> {
        self.0.enqueue(request)
    }
    fn dequeue(&self, idempotency_key: &str) -> Result<(), String> {
        self.0.dequeue(idempotency_key)
    }
    fn pending(&self) -> Result<Vec<TxRequest>, String> {
        self.0.pending()
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let appdata = app.path().app_data_dir()?;
            let menu_dir = appdata.join("menu");
            // P0：公钥内置在客户端；生产环境由构建配置注入，私钥永不进客户端。
            let store = MenuStore::new(menu_dir, demo_verifying_key());

            // 交易账本：打不开也不阻断启动，但要留下告警让上层显示。
            // 同一个连接既做交易账本又做审计查询，所以用 Arc 共享。
            let (tx, journal_warning, audit) = match SqliteJournal::open(appdata.join("tx.db")) {
                Ok(journal) => {
                    let shared = Arc::new(journal);
                    let engine =
                        TxEngine::new().with_journal(Box::new(ArcJournal(Arc::clone(&shared))));
                    (engine, None, Some(shared))
                }
                Err(e) => (
                    TxEngine::new(),
                    Some(format!("交易账本不可用（{e}）：本会话不会落盘")),
                    None,
                ),
            };
            let state = AppState::new(
                store,
                appdata.display().to_string(),
                tx,
                journal_warning,
                audit,
            );
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
            commands::submit_transaction,
            commands::print_voucher,
            commands::tx_pending_count,
            commands::replay_pending,
            commands::package_module,
            commands::audit_tail,
            commands::session_touch,
            commands::session_tick,
            commands::session_blur,
        ])
        .run(tauri::generate_context!())
        .expect("TellerKit 启动失败");
}

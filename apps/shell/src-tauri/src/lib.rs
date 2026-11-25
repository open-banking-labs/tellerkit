//! TellerKit 主壳。
//!
//! 职责边界（PLAN.md §4）：
//! - Rust 侧持有唯一真相源：菜单树、权限快照、mock 宿主、本地菜单包仓库；
//! - 前端只拿"已经剪枝好的可见菜单"，不拿全量树，也不做权限判断；
//! - 启动路径上**没有**任何网络等待：本地包 → 验签 → 反序列化；
//!   版本探测是渲染之后的独立调用。

pub mod app;
pub mod commands;
pub mod config_sig;
pub mod endpoint_config;
pub mod ports;

use std::sync::Mutex;

use std::sync::Arc;

use std::sync::Arc as StdArc;
use tellerkit_core::banking::RemoteBanking;
use tellerkit_core::device::{mock_devices, DevicePolicy, Devices};
use tellerkit_core::mock::demo_verifying_key;
use tellerkit_core::model::MenuTree;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::pkg::BizPackage;
use tellerkit_core::replay::{ReplayPolicy, ReplayScheduler};
use tellerkit_core::reversal::ReversalBook;
use tellerkit_core::session::SessionGuard;
use tellerkit_core::store::MenuStore;
use tellerkit_core::tx::TxEngine;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult};
use tellerkit_journal_sqlite::SqliteJournal;

use tauri::Manager;

/// 应用状态。所有命令都通过 `Mutex<AppState>` 访问，避免 Rust/前端各持一份真相。
pub struct AppState {
    /// ③ 边界：远端后台。菜单、登录、交易都走这一个端口。
    ///
    /// 默认是内置 mock；配置了 `endpoints.json` 时换成 `RoutedBanking`。
    pub remote: Arc<dyn RemoteBanking>,
    /// 演示开关句柄。只有 mock 模式存在；接入真实端点后为 None。
    pub mock: Option<Arc<ports::MockRemote>>,
    pub store: MenuStore,
    /// 当前登录用户的权限快照。登出/切号必须置空。
    pub snapshot: Option<PermSnapshot>,
    /// 全量菜单树（不做权限过滤），供每次过滤复用。
    pub menu: Option<MenuTree>,
    pub appdata_dir: String,
    /// 账本打不开时的告警。柜面宁可"能开但提示异常"，也不要"打不开"。
    pub journal_warning: Option<String>,
    /// 交易引擎：幂等索引 + 待重放队列（P1.4 会落盘）。
    pub tx: TxEngine,
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
    /// 时钟端口。锁屏/退避/审计都靠它，注入才能在测试里控制时间。
    pub clock: StdArc<dyn ports::Clock>,
    /// 自动重放调度。挂起交易按退避节奏逐笔重试，不再一次全推。
    pub replay: ReplayScheduler,
    /// 冲正账本：记录"状态不明"的交易与冲正申请。
    pub reversal: ReversalBook,
}

impl AppState {
    pub fn new(
        store: MenuStore,
        appdata_dir: String,
        tx: TxEngine,
        journal_warning: Option<String>,
        audit: Option<Arc<SqliteJournal>>,
        remote: Arc<dyn RemoteBanking>,
        mock: Option<Arc<ports::MockRemote>>,
    ) -> Self {
        Self {
            remote,
            mock,
            store,
            snapshot: None,
            menu: None,
            appdata_dir,
            journal_warning,
            tx,
            devices: mock_devices(),
            packages: Vec::new(),
            audit,
            policy: DevicePolicy::allow_mocks(),
            session: None,
            clock: StdArc::new(ports::SystemClock),
            replay: ReplayScheduler::new(ReplayPolicy::default()),
            reversal: ReversalBook::new(),
        }
    }

    /// 清空权限快照。登出、切换账号、收到 401 时都必须调用。
    pub fn clear_session(&mut self) {
        self.snapshot = None;
    }
}

/// 选择远端实现：配置了端点就用 `RoutedBanking`，否则回退内置 mock。
///
/// 配置非法时**回退而不是崩溃**——柜面宁可跑在演示模式，也不能白屏。
fn build_remote(
    appdata: &std::path::Path,
) -> (
    Arc<dyn RemoteBanking>,
    Option<Arc<ports::MockRemote>>,
    Option<String>,
) {
    let path = std::env::var("TK_ENDPOINTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| appdata.join("endpoints.json"));

    if path.exists() {
        match endpoint_config::load_router(&path) {
            Ok(loaded) => {
                let plaintext = !std::env::var("TK_REQUIRE_TLS")
                    .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                    .unwrap_or(false);
                return (
                    Arc::new(
                        tellerkit_core::banking::RoutedBanking::new(loaded.router)
                            .with_idempotency(loaded.idempotent_tx),
                    ),
                    None,
                    Some(format!(
                        "已按 {} 接入真实端点{}",
                        path.display(),
                        if plaintext {
                            "（未强制 TLS：生产部署请设置 TK_REQUIRE_TLS=1 并补 TLS 支持）"
                        } else {
                            ""
                        }
                    )),
                );
            }
            Err(e) => {
                eprintln!("端点配置不可用，回退内置 mock：{e}");
                let mock = Arc::new(ports::MockRemote::default());
                return (
                    Arc::clone(&mock) as Arc<dyn RemoteBanking>,
                    Some(mock),
                    Some(format!("端点配置无效（{e}），已回退内置 mock")),
                );
            }
        }
    }

    let mock = Arc::new(ports::MockRemote::default());
    (
        Arc::clone(&mock) as Arc<dyn RemoteBanking>,
        Some(mock),
        None,
    )
}

/// 后台重放线程。
///
/// 为什么需要：挂起交易的自动重放以前只在前端 tick 时发生，
/// 界面关掉就等于停摆。柜面终端下班后常留着进程，这时候更需要它自己跑。
///
/// 间隔由 `TK_REPLAY_INTERVAL_SECS` 控制，默认 60 秒；`0` 关闭。
fn spawn_replay_worker(handle: tauri::AppHandle) {
    let secs: u64 = std::env::var("TK_REPLAY_INTERVAL_SECS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(60);
    if secs == 0 {
        return;
    }

    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        let state = handle.state::<Mutex<AppState>>();
        let Ok(mut guard) = state.lock() else {
            continue;
        };
        // 未登录不重放：没有权限快照时重放会在审计里留下 unknown 操作者
        if guard.snapshot.is_none() {
            continue;
        }
        let mut app = app::ShellApp::new(&mut guard);
        let _ = app.replay_pending();
    });
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
            // 端点配置（changes/008）：有配置就用真实路由，否则回退内置 mock
            let (remote, mock, remote_note) = build_remote(&appdata);
            let app_handle = app.handle().clone();
            let state = AppState::new(
                store,
                appdata.display().to_string(),
                tx,
                journal_warning,
                audit,
                remote,
                mock,
            );
            if let Some(note) = remote_note {
                eprintln!("{note}");
            }
            app.manage(Mutex::new(state));
            // 必须在 manage 之后：线程的第一次 tick 要能拿到状态
            spawn_replay_worker(app_handle);
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
            commands::tx_exceptions,
            commands::tx_reconciliation,
            commands::request_reversal,
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

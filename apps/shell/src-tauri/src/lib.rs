//! TellerKit 主壳。
//!
//! 职责边界（PLAN.md §4）：
//! - Rust 侧持有唯一真相源：菜单树、权限快照、mock 宿主、本地菜单包仓库；
//! - 前端只拿"已经剪枝好的可见菜单"，不拿全量树，也不做权限判断；
//! - 启动路径上**没有**任何网络等待：本地包 → 验签 → 反序列化；
//!   版本探测是渲染之后的独立调用。

pub mod app;
pub mod bootstrap;
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
use tellerkit_core::pack::pubkey_from_hex;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::pkg::BizPackage;
use tellerkit_core::replay::{ReplayPolicy, ReplayScheduler};
use tellerkit_core::reversal::ReversalBook;
use tellerkit_core::session::SessionGuard;
use tellerkit_core::store::MenuStore;
use tellerkit_core::tx::TxEngine;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult};
use tellerkit_journal_sqlite::SqliteJournal;

use bootstrap::StartupFatal;
use ed25519_dalek::VerifyingKey;
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
    /// 启动被拒绝的原因。非空时**所有命令**都直接失败，只保留可读错误页。
    ///
    /// 放在这里而不是单独的托管状态：`lock()` 是所有命令的唯一入口，
    /// 在这里拦一次，比在 23 个命令里各写一遍可靠。
    pub startup_fatal: bootstrap::StartupFatal,
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
    /// 菜单包 / 业务包的验签公钥。
    ///
    /// 生产由构建期 `TK_MENU_PUBKEY_HEX` 注入；调试构建没有注入时才回退演示公钥。
    pub verifying_key: VerifyingKey,
}

impl AppState {
    pub fn new(
        store: MenuStore,
        appdata_dir: String,
        tx: TxEngine,
        journal_warning: Option<String>,
        audit: Option<Arc<SqliteJournal>>,
        remote: ports::RemoteChoice,
        verifying_key: VerifyingKey,
    ) -> Self {
        let ports::RemoteChoice { remote, mock } = remote;
        Self {
            remote,
            mock,
            store,
            snapshot: None,
            menu: None,
            appdata_dir,
            startup_fatal: bootstrap::StartupFatal::default(),
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
            verifying_key,
        }
    }

    /// 清空权限快照。登出、切换账号、收到 401 时都必须调用。
    pub fn clear_session(&mut self) {
        self.snapshot = None;
    }
}

/// 解析构建期注入的菜单包验签公钥。
///
/// 生产：`build.rs` 要求 release 构建必须提供 `TK_MENU_PUBKEY_HEX`，并把它注入编译环境；
/// 调试：没有注入时回退演示公钥，保证 `cargo run` 开箱可用。该回退在 release 不存在。
fn resolve_menu_verifying_key(injected: Option<&str>, debug: bool) -> Result<VerifyingKey, String> {
    if let Some(hex) = injected {
        if hex.trim().is_empty() {
            return Err("TK_MENU_PUBKEY_HEX 为空：拒绝用空公钥启动".into());
        }
        return pubkey_from_hex(hex)
            .map_err(|e| format!("TK_MENU_PUBKEY_HEX 不是合法 Ed25519 公钥：{e}"));
    }
    if debug {
        Ok(demo_verifying_key())
    } else {
        Err("release 构建缺少 TK_MENU_PUBKEY_HEX：拒绝使用演示公钥启动".into())
    }
}

fn menu_verifying_key() -> Result<VerifyingKey, String> {
    resolve_menu_verifying_key(option_env!("TK_MENU_PUBKEY_HEX"), cfg!(debug_assertions))
}

/// 选择远端实现。策略与判定在 [`bootstrap`]：这里只负责读环境、组装、落状态。
///
/// release 构建下，任何"配置不成立"都变成启动拒绝（可读错误页）；
/// debug 构建才允许回退演示远端，并且一定带 note。
fn build_remote(appdata: &std::path::Path) -> (ports::RemoteChoice, Option<String>, StartupFatal) {
    let release = !cfg!(debug_assertions);
    let path = std::env::var("TK_ENDPOINTS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| appdata.join("endpoints.json"));

    let outcome = bootstrap::policy_from_env(release)
        .and_then(|policy| bootstrap::resolve_remote(&path, release, &policy));

    match outcome {
        Ok(setup) => (setup.choice, setup.note, StartupFatal::default()),
        Err(reason) => {
            // 进程还要起来：只有起来，才能把"为什么被拒绝"显示成人能读的页面。
            // 业务命令一律先过 `StartupFatal::check`，所以演示远端不会被真正使用。
            eprintln!("启动被拒绝：{reason}");
            let mock = Arc::new(ports::MockRemote::default());
            (
                ports::RemoteChoice {
                    remote: Arc::clone(&mock) as Arc<dyn RemoteBanking>,
                    mock: Some(mock),
                },
                None,
                StartupFatal(Some(reason)),
            )
        }
    }
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
        // 启动被拒绝时绝不重放：兜底的演示远端不能碰任何挂起交易。
        if guard.startup_fatal.check().is_err() {
            continue;
        }
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
            // 注入缺失/非法时直接拒绝启动，不允许静默回退到演示公钥。
            let verifying_key = menu_verifying_key()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
            let store = MenuStore::new(menu_dir, verifying_key);

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
            // 端点配置（changes/008）：release 缺配置/配置非法即拒绝启动
            let (remote, remote_note, startup_fatal) = build_remote(&appdata);
            let app_handle = app.handle().clone();
            let mut state = AppState::new(
                store,
                appdata.display().to_string(),
                tx,
                journal_warning,
                audit,
                remote,
                verifying_key,
            );
            state.startup_fatal = startup_fatal;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injected_key_wins_over_demo() {
        let hex = tellerkit_core::mock::demo_pubkey_hex();
        let key = resolve_menu_verifying_key(Some(&hex), true).expect("演示公钥十六进制必须可解析");
        assert_eq!(key, demo_verifying_key());
    }

    #[test]
    fn invalid_injected_key_is_rejected_even_in_debug() {
        let err = resolve_menu_verifying_key(Some("not-hex"), true).unwrap_err();
        assert!(
            err.contains("TK_MENU_PUBKEY_HEX"),
            "错误信息要指向配置项：{err}"
        );
    }

    #[test]
    fn empty_injected_key_is_rejected() {
        let err = resolve_menu_verifying_key(Some("  "), true).unwrap_err();
        assert!(err.contains("为空"), "空值不能被当作未配置：{err}");
    }

    #[test]
    fn debug_without_injection_falls_back_to_demo() {
        assert_eq!(
            resolve_menu_verifying_key(None, true).unwrap(),
            demo_verifying_key()
        );
    }

    #[test]
    fn release_without_injection_refuses_to_start() {
        let err = resolve_menu_verifying_key(None, false).unwrap_err();
        assert!(err.contains("release"), "错误信息要说明是发布构建：{err}");
    }
}

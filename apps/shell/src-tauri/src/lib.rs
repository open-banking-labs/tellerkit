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
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::pkg::BizPackage;
use tellerkit_core::pubkey_from_hex;
use tellerkit_core::replay::{ReplayAttempt, ReplayPolicy, ReplayScheduler};
use tellerkit_core::reversal::{ExceptionKind, ReversalBook, ReversalRecord};
use tellerkit_core::session::SessionGuard;
use tellerkit_core::tx::TxEngine;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult};
use tellerkit_core::MenuStore;
use tellerkit_core::MenuTree;
use tellerkit_journal_sqlite::SqliteJournal;

use bootstrap::StartupFatal;
use ed25519_dalek::VerifyingKey;
use tauri::Manager;

/// 应用状态。所有命令都通过 `Mutex<AppState>` 访问，避免 Rust/前端各持一份真相。
pub struct AppState {
    /// ③ 边界：远端后台。菜单、登录、交易都走这一个端口。
    ///
    /// 默认是内置 mock；配置了 `endpoints.json` 时换成 `RoutedBanking`。
    pub(crate) remote: Arc<dyn RemoteBanking>,
    /// 演示开关句柄。只有 mock 模式存在；接入真实端点后为 None。
    pub(crate) mock: Option<Arc<ports::MockRemote>>,
    pub(crate) store: MenuStore,
    /// 当前登录用户的权限快照。登出/切号必须置空。
    pub(crate) snapshot: Option<PermSnapshot>,
    /// 全量菜单树（不做权限过滤），供每次过滤复用。
    pub(crate) menu: Option<MenuTree>,
    pub(crate) appdata_dir: String,
    /// 启动被拒绝的原因。非空时**所有命令**都直接失败，只保留可读错误页。
    ///
    /// 放在这里而不是单独的托管状态：`lock()` 是所有命令的唯一入口，
    /// 在这里拦一次，比在 23 个命令里各写一遍可靠。
    pub(crate) startup_fatal: bootstrap::StartupFatal,
    /// 账本打不开时的告警。柜面宁可"能开但提示异常"，也不要"打不开"。
    pub(crate) journal_warning: Option<String>,
    /// 交易引擎：幂等索引 + 待重放队列（P1.4 会落盘）。
    pub(crate) tx: TxEngine,
    /// 设备集合：P1.2 的 mock 适配器，P3 换真机只换这一处构造。
    pub(crate) devices: Devices,
    /// 已加载的业务包（按依赖拓扑序）。前端按 route 找模块。
    pub(crate) packages: Vec<BizPackage>,
    /// 审计日志句柄。`None` 表示账本不可用（降级为内存模式）。
    pub(crate) audit: Option<Arc<SqliteJournal>>,
    /// 单实例锁。必须持有到进程结束；提前释放等于允许第二个进程进来。
    ///
    /// `None` 时 `startup_fatal` 一定非空（第二个实例被拒）。
    pub(crate) instance_lock: Option<bootstrap::InstanceLock>,
    /// 外设型号白名单。默认只允许 mock 型号——没登记就拒绝。
    pub(crate) policy: DevicePolicy,
    /// 会话锁屏状态。`None` 表示未登录。
    pub(crate) session: Option<SessionGuard>,
    /// 时钟端口。锁屏/退避/审计都靠它，注入才能在测试里控制时间。
    pub(crate) clock: StdArc<dyn ports::Clock>,
    /// 自动重放调度。挂起交易按退避节奏逐笔重试，不再一次全推。
    pub(crate) replay: ReplayScheduler,
    /// 冲正账本：记录"状态不明"的交易与冲正申请。
    pub(crate) reversal: ReversalBook,
    /// 菜单包 / 业务包的验签公钥。
    ///
    /// 生产由构建期 `TK_MENU_PUBKEY_HEX` 注入；调试构建没有注入时才回退演示公钥。
    pub(crate) verifying_key: VerifyingKey,
}

impl AppState {
    pub fn new(
        store: MenuStore,
        appdata_dir: String,
        tx: TxEngine,
        journal_warning: Option<String>,
        audit: Option<Arc<SqliteJournal>>,
        choice: ports::RemoteChoice,
        verifying_key: VerifyingKey,
    ) -> Self {
        let remote = Arc::clone(choice.remote());
        let mock = choice.mock().cloned();
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
            instance_lock: None,
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

/// 账本被占用 / 明确降级时拒绝业务命令；其它打不开的原因只告警。
/// 不看宽泛的"密文"字样——迁移事务遇到锁或 I/O 失败也会带这个词，那不是篡改。
fn journal_open_fatal(error: &str) -> Option<String> {
    if error.contains("账本已被另一个进程占用") {
        return Some(format!("账本已被另一个进程占用，已拒绝启动：{error}"));
    }
    if error.contains("拒绝启动以防密文降级") || error.contains("疑似密文被剥离") {
        return Some(format!("交易账本密文校验失败，已拒绝启动：{error}"));
    }
    None
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
        Ok(setup) => {
            let (choice, note) = setup.into_parts();
            (choice, note, StartupFatal::default())
        }
        Err(reason) => {
            // 进程还要起来：只有起来，才能把"为什么被拒绝"显示成人能读的页面。
            // 业务命令一律先过 `StartupFatal::check`，所以演示远端不会被真正使用。
            eprintln!("启动被拒绝：{reason}");
            let mock = Arc::new(ports::MockRemote::default());
            (
                ports::RemoteChoice::new(Arc::clone(&mock) as Arc<dyn RemoteBanking>, Some(mock)),
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
        // 会话校验统一走 replay_pending 里的 require_unlocked：
        // 未登录/锁定时它会拒绝并写审计，后台线程吞掉 Err 即可（skip 已留痕）。
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
    fn record_and_enqueue(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        self.0.record_and_enqueue(request, result)
    }
    fn settle(
        &self,
        request: &TxRequest,
        result: &TxResult,
        signature: Option<&str>,
        dequeue: bool,
    ) -> Result<(), String> {
        self.0.settle(request, result, signature, dequeue)
    }
    fn load_idempotent(&self) -> Result<Vec<(String, String, TxResult)>, String> {
        self.0.load_idempotent()
    }
    fn save_idempotent(&self, key: &str, signature: &str, result: &TxResult) -> Result<(), String> {
        self.0.save_idempotent(key, signature, result)
    }
    fn load_attempts(&self) -> Result<Vec<ReplayAttempt>, String> {
        self.0.load_attempts()
    }
    fn upsert_attempt(&self, attempt: &ReplayAttempt) -> Result<(), String> {
        self.0.upsert_attempt(attempt)
    }
    fn delete_attempt(&self, key: &str) -> Result<(), String> {
        self.0.delete_attempt(key)
    }
    fn load_exceptions(&self) -> Result<Vec<(String, ExceptionKind)>, String> {
        self.0.load_exceptions()
    }
    fn upsert_exception(&self, key: &str, kind: ExceptionKind) -> Result<(), String> {
        self.0.upsert_exception(key, kind)
    }
    fn load_reversals(&self) -> Result<Vec<ReversalRecord>, String> {
        self.0.load_reversals()
    }
    fn upsert_reversal(&self, record: &ReversalRecord) -> Result<(), String> {
        self.0.upsert_reversal(record)
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

            // 单实例：同一数据目录只允许一个柜面终端。没拿到锁也让进程起来，
            // 但原因写进 StartupFatal（所有命令都被拦），绝不降级成演示模式。
            let (instance_lock, instance_error) = match bootstrap::acquire_instance_lock(&appdata) {
                Ok(lock) => (Some(lock), None),
                Err(reason) => (None, Some(reason)),
            };

            // 交易账本本身也带 SQLite 排他锁：已有实例时连读都不读别人的数据。
            // 账本被占用或密文被剥离是启动拒绝；其它打不开的原因只告警。
            let (tx, journal_warning, audit, recovery_journal, ledger_fatal) =
                if instance_error.is_some() {
                    (
                        TxEngine::new(),
                        Some("未打开交易账本：本进程不是唯一实例".to_string()),
                        None,
                        None,
                        None,
                    )
                } else {
                    match SqliteJournal::open(appdata.join("tx.db")) {
                        Ok(journal) => {
                            let shared = Arc::new(journal);
                            let recovery: Arc<dyn TxJournal> =
                                Arc::new(ArcJournal(Arc::clone(&shared)));
                            let engine = TxEngine::new()
                                .with_journal(Box::new(ArcJournal(Arc::clone(&shared))));
                            (engine, None, Some(shared), Some(recovery), None)
                        }
                        Err(e) => {
                            let fatal = journal_open_fatal(&e);
                            (
                                TxEngine::new(),
                                Some(format!("交易账本不可用（{e}）：本会话不会落盘")),
                                None,
                                None,
                                fatal,
                            )
                        }
                    }
                };
            // 端点配置（changes/008）：release 缺配置/配置非法即拒绝启动
            let (remote, remote_note, mut startup_fatal) = build_remote(&appdata);
            // 单实例与账本排他的拒绝优先于端点配置：没有唯一实例谈安全没有意义。
            if let Some(reason) = instance_error {
                startup_fatal = StartupFatal(Some(reason));
            } else if let Some(reason) = ledger_fatal {
                startup_fatal = StartupFatal(Some(reason));
            }
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
            // 必须存进状态：局部变量一丢就等于放锁，第二个进程马上能进来。
            state.instance_lock = instance_lock;
            // 幂等索引由 TxEngine::with_journal 读回；这里补读异常/冲正与重放进度。
            // 读盘失败只告警不阻断启动，但不能静默当作"没有待办"。
            if let Some(journal) = recovery_journal {
                let replay =
                    ReplayScheduler::with_journal(ReplayPolicy::default(), Arc::clone(&journal));
                let reversal = ReversalBook::with_journal(journal);
                let mut notes = Vec::new();
                if let Some(e) = replay.journal_error() {
                    notes.push(e.to_string());
                }
                if let Some(e) = reversal.journal_error() {
                    notes.push(e.to_string());
                }
                if !notes.is_empty() {
                    let note = format!("恢复状态不完整：{}", notes.join("；"));
                    state.journal_warning = Some(match state.journal_warning.take() {
                        Some(old) => format!("{old}；{note}"),
                        None => note.clone(),
                    });
                    eprintln!("{note}");
                }
                state.replay = replay;
                state.reversal = reversal;
            }
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

    #[test]
    fn cipher_downgrade_is_fatal_but_other_journal_errors_only_warn() {
        assert!(journal_open_fatal("拒绝启动以防密文降级").is_some());
        assert!(journal_open_fatal("疑似密文被剥离").is_some());
        // 账本被占也必须 fail-closed，不能降级为"能开但提示异常"。
        let busy = journal_open_fatal("账本已被另一个进程占用：database is locked")
            .expect("被占用必须拒绝启动");
        assert!(busy.contains("另一个进程"), "{busy}");
        // 迁移遇到锁/I/O 失败也会提到"密文"，但那不是篡改，不能升级成启动拒绝。
        assert!(journal_open_fatal("密文迁移事务失败：database is locked").is_none());
        assert!(journal_open_fatal("打开账本失败：database is locked").is_none());
    }

    #[test]
    fn second_instance_failure_blocks_every_command() {
        let dir = tempfile::tempdir().unwrap();
        let first = bootstrap::acquire_instance_lock(dir.path()).expect("首个实例拿到锁");
        let reason = bootstrap::acquire_instance_lock(dir.path()).expect_err("第二个实例被拒");

        // 即便兜底的演示远端还在，启动拒绝也必须让所有命令止步。
        let (remote, _note, _unused) = build_remote(dir.path());
        let mut state = AppState::new(
            MenuStore::new(dir.path().join("menu"), demo_verifying_key()),
            dir.path().display().to_string(),
            TxEngine::new(),
            Some("未打开交易账本：本进程不是唯一实例".into()),
            None,
            remote,
            demo_verifying_key(),
        );
        state.startup_fatal = StartupFatal(Some(reason));
        let err = state.startup_fatal.check().unwrap_err();
        assert!(
            err.contains("启动被拒绝"),
            "前端错误页要看到明确拒绝：{err}"
        );
        assert!(err.contains("另一个柜面终端实例"), "要带可读原因：{err}");
        drop(first);
    }

    #[test]
    fn every_command_is_gated_by_startup_fatal() {
        let source = include_str!("commands.rs");
        assert!(
            source.contains("guard.startup_fatal.check()?"),
            "commands::lock 必须统一拦截启动失败"
        );
        for name in [
            "boot",
            "login",
            "logout",
            "current_menu",
            "check_update",
            "reload_from_disk",
            "dev_set_offline",
            "dev_set_tampered",
            "dev_bump_server_version",
            "dev_corrupt_local",
            "dev_reset_local",
            "submit_transaction",
            "print_voucher",
            "tx_pending_count",
            "tx_exceptions",
            "tx_reconciliation",
            "request_reversal",
            "replay_pending",
            "package_module",
            "audit_tail",
            "session_touch",
            "session_tick",
            "session_blur",
        ] {
            let marker = format!("pub fn {name}(");
            let start = source
                .find(&marker)
                .unwrap_or_else(|| panic!("找不到命令 {name}"));
            let rest = &source[start + marker.len()..];
            let end = rest.find("\npub fn ").unwrap_or(rest.len());
            assert!(
                rest[..end].contains("lock(&state)?"),
                "命令 {name} 必须经 commands::lock 做启动检查"
            );
        }
    }
}

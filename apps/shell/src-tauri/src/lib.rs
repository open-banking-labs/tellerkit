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

use std::sync::{Arc, Mutex};

use tellerkit_core::banking::RemoteBanking;
use tellerkit_core::device::{mock_devices, DevicePolicy, Devices};
use tellerkit_core::mock::demo_verifying_key;
use tellerkit_core::model::MenuTree;
use tellerkit_core::pack::pubkey_from_hex;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::pkg::BizPackage;
use tellerkit_core::recovery::{parse_exception_kind, RecoveryJournal};
use tellerkit_core::replay::{ReplayPolicy, ReplayScheduler};
use tellerkit_core::reversal::{ExceptionKind, ReversalBook};
use tellerkit_core::store::MenuStore;
use tellerkit_core::tx::TxEngine;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult};
use tellerkit_journal_sqlite::SqliteJournal;

use bootstrap::StartupFatal;
use ed25519_dalek::VerifyingKey;
use tauri::Manager;

/// 会话状态的共享句柄（独立锁，P0-10 第 2 步）。
///
/// `session_tick` 每秒一次，不能排在持锁 3s 的交易后面；所以会话从 `AppState`
/// 拆出来单独托管。锁序：`AppState` → `SessionSlot`，禁止反向。
pub type SharedSession = Arc<Mutex<app::SessionSlot>>;

/// 应用状态。所有命令都通过 `Arc<Mutex<AppState>>` 访问，避免 Rust/前端各持一份真相。
///
/// 用 `Arc` 而不是裸 `Mutex`：命令是 `async` 的，阻塞工作要搬进
/// `spawn_blocking`，闭包必须 `'static`，只能先克隆句柄再 move 进去。
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
    /// 会话锁屏状态（独立锁）。未登录/锁定状态在槽内表示，不在这里。
    ///
    /// 与 `clock`/`audit` 共享同一批 `Arc`，见 `AppState::new` 的装配。
    pub session: SharedSession,
    /// 时钟端口。锁屏/退避/审计都靠它，注入才能在测试里控制时间。
    pub clock: Arc<dyn ports::Clock>,
    /// 自动重放调度。挂起交易按退避节奏逐笔重试，不再一次全推。
    pub replay: ReplayScheduler,
    /// 冲正账本：记录"状态不明"的交易与冲正申请。
    pub reversal: ReversalBook,
    /// "正在检查菜单包更新"的在途标记。
    ///
    /// 更新检查（探测 + 下载 + 安装）现在跑在锁外，两次并发检查会同时写同一个
    /// 临时包文件——写入不是原子的，可能把包写坏。标记挡住第二次。
    pub updating: bool,
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
        let clock: Arc<dyn ports::Clock> = Arc::new(ports::SystemClock);
        // 会话槽与 AppState 必须共用同一份 clock/audit：否则 tick 用的时间、
        // 写的账本会和交易路径不是同一个，测试里注入的时钟也会失效。
        let session: SharedSession = Arc::new(Mutex::new(app::SessionSlot::new(
            Arc::clone(&clock),
            audit.as_ref().map(Arc::clone),
        )));
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
            session,
            clock,
            replay: ReplayScheduler::new(ReplayPolicy::default()),
            reversal: ReversalBook::new(),
            updating: false,
            verifying_key,
        }
    }

    /// 清空权限快照。登出、切换账号、收到 401 时都必须调用。
    pub fn clear_session(&mut self) {
        self.snapshot = None;
    }

    /// 替换时钟端口，**同时**替换会话槽里的那一份。
    ///
    /// 注入时钟必须两处一起换：只换 `AppState.clock` 会让锁屏判定继续用系统时钟
    /// （P0-6 的测试就是靠这一点精确控制时间）。只有测试会调用它。
    #[cfg(test)]
    pub fn set_clock(&mut self, clock: Arc<dyn ports::Clock>) {
        self.clock = Arc::clone(&clock);
        if let Ok(mut slot) = self.session.lock() {
            slot.set_clock(clock);
        }
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
        let shared = Arc::clone(handle.state::<Arc<Mutex<AppState>>>().inner());

        // 阶段一（锁内）：算出本轮到期清单，并把请求取出队列占位。
        let prepared = {
            let Ok(mut guard) = shared.lock() else {
                continue;
            };
            // 启动被拒绝时绝不重放：兜底的演示远端不能碰任何挂起交易。
            if guard.startup_fatal.check().is_err() {
                continue;
            }
            // 会话校验统一走 prepare_replay 里的 require_unlocked：
            // 未登录/锁定时它会拒绝并写审计，后台线程吞掉 Err 即可（skip 已留痕）。
            match app::ShellApp::new(&mut guard).prepare_replay() {
                Ok(prepared) => prepared,
                Err(_) => continue,
            }
        };

        // 阶段二（**不持锁**）：逐笔打主机。重放可能是"多笔 × 3s 超时"，
        // 整段持锁会让柜员操作、锁屏判定都排在后面。
        let outcomes = prepared.call_hosts();

        // 阶段三（锁内）：落回执、推进调度表。
        let Ok(mut guard) = shared.lock() else {
            continue;
        };
        let _ = app::ShellApp::new(&mut guard).complete_replay(prepared, outcomes);
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
            // 恢复跨重启状态：幂等索引已由 TxEngine::with_journal 读回；
            // 这里读异常/冲正清单与重放进度，失败只告警不阻断启动。
            let mut recovery_warnings: Vec<String> = Vec::new();
            if let Some(journal) = state.audit.as_deref() {
                match journal.load_exceptions() {
                    Ok(rows) => {
                        let mut exceptions = Vec::new();
                        for (key, raw) in rows {
                            match parse_exception_kind(&raw) {
                                Ok(kind) => exceptions.push((key, kind)),
                                Err(e) => recovery_warnings.push(e),
                            }
                        }
                        match journal.load_reversals() {
                            Ok(reversals) => state.reversal.restore(exceptions, reversals),
                            Err(e) => recovery_warnings.push(format!("读取冲正记录失败：{e}")),
                        }
                    }
                    Err(e) => recovery_warnings.push(format!("读取待冲正清单失败：{e}")),
                }
                match journal.load_attempts() {
                    Ok(attempts) => state.replay.restore(attempts),
                    Err(e) => recovery_warnings.push(format!("读取重放进度失败：{e}")),
                }
            }
            if !recovery_warnings.is_empty() {
                let note = format!("恢复状态不完整：{}", recovery_warnings.join("；"));
                state.journal_warning = Some(match state.journal_warning.take() {
                    Some(old) => format!("{old}；{note}"),
                    None => note,
                });
                eprintln!("{}", state.journal_warning.as_deref().unwrap_or(""));
            }
            // 写前意图已落盘但主机结论未记的交易，只能按"状态不明"处置：
            // 标记待冲正并提示查账，绝不自动重发（可能重复入账）。
            let recovered = state.tx.take_in_flight();
            for request in &recovered {
                state
                    .reversal
                    .mark(&request.idempotency_key, ExceptionKind::MarkedForReversal);
                if let Some(journal) = state.audit.as_deref() {
                    if let Err(e) = journal.save_exception(
                        &request.idempotency_key,
                        tellerkit_core::recovery::exception_kind_str(
                            ExceptionKind::MarkedForReversal,
                        ),
                    ) {
                        eprintln!("遗留意图标记落盘失败：{e}");
                    }
                }
            }
            if !recovered.is_empty() {
                eprintln!(
                    "发现 {} 笔在途交易（结论未记），已转入待冲正，请查账确认",
                    recovered.len()
                );
            }
            if let Some(note) = remote_note {
                eprintln!("{note}");
            }
            // 会话槽单独托管一份：`session_*` 命令按类型直接取，不经过 AppState 锁。
            // 托管的是同一个 Arc（不是副本），所以 require_unlocked 看到的仍是同一份状态。
            app.manage(Arc::clone(&state.session));
            app.manage(Arc::new(Mutex::new(state)));
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

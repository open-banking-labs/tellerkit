//! Tauri 命令层。
//!
//! 这一层刻意保持很薄：加锁、计时、把错误翻成中文、提供演示开关。
//! 所有决策逻辑都在 `tellerkit-core::sync` 与 `filter`，因此重要路径可以被单测覆盖。

use std::collections::BTreeMap;
use std::fs;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use tellerkit_core::device::{PrintReceipt, Voucher};
use tellerkit_core::filter::{filter_visible, MenuView};
use tellerkit_core::mock::DemoRole;
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::session::{SessionGuard, SessionPolicy};
use tellerkit_core::sync;
use tellerkit_core::tx::{TxHostMode, TxRequest, TxResult};
use tellerkit_core::LoadSource;
use tellerkit_journal_sqlite::AuditEvent;

use crate::AppState;

fn lock<'a>(
    state: &'a State<'_, Mutex<AppState>>,
) -> Result<std::sync::MutexGuard<'a, AppState>, String> {
    state
        .lock()
        .map_err(|_| "应用状态已损坏，请重启客户端".to_string())
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
    /// 全量树递归剪枝耗时（毫秒），P0 验收要求 < 1ms 量级。
    pub filter_ms: f64,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct UpdateReport {
    pub checked: bool,
    #[ts(type = "number | null")]
    pub local: Option<u64>,
    #[ts(type = "number | null")]
    pub latest: Option<u64>,
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
    #[ts(type = "number")]
    pub server_version: u64,
    #[ts(type = "Array<number>")]
    pub local_versions: Vec<u64>,
}

fn source_label(source: &LoadSource) -> String {
    match source {
        LoadSource::LocalFile { .. } => "local".to_string(),
        LoadSource::NetworkFallback { .. } => "network".to_string(),
    }
}

/// 记一条审计。写不进去不影响业务，但要把失败暴露出来（审计坏了不能静默）。
fn audit(st: &AppState, actor: &str, action: &str, target: &str, result: &str, detail: &str) {
    if let Some(journal) = &st.audit {
        let event = AuditEvent {
            actor: actor.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            detail: detail.to_string(),
            at: String::new(), // 由实现填时间戳
        };
        if let Err(e) = journal.audit(&event) {
            eprintln!("审计写入失败：{e}");
        }
    }
}

/// 审计日志尾部。返回已格式化的一行一条，避免为一屏日志再加一个 DTO。
#[tauri::command]
pub fn audit_tail(limit: usize, state: State<'_, Mutex<AppState>>) -> Result<Vec<String>, String> {
    let st = lock(&state)?;
    let Some(journal) = &st.audit else {
        return Ok(vec!["审计日志不可用：账本未能打开".into()]);
    };
    let events = journal.audit_tail(limit.min(500))?;
    Ok(events
        .into_iter()
        .map(|e| {
            format!(
                "{} · {} {} {} · {}{}",
                e.at,
                e.actor,
                e.action,
                e.target,
                e.result,
                if e.detail.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", e.detail)
                }
            )
        })
        .collect())
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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn session_state_of(session: &Option<SessionGuard>) -> SessionState {
    match session {
        None => SessionState {
            locked: false,
            reason: None,
            idle_secs: 0,
        },
        Some(guard) => SessionState {
            locked: guard.is_locked(),
            reason: guard.reason().map(|r| r.label().to_string()),
            idle_secs: guard.idle_secs(now_secs()),
        },
    }
}

/// 记录一次操作。节流由前端做，这里只认最后一次时间。
#[tauri::command]
pub fn session_touch(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut st = lock(&state)?;
    if let Some(guard) = st.session.as_mut() {
        guard.touch(now_secs());
    }
    Ok(session_state_of(&st.session))
}

/// 周期性评估。锁定了就顺手记一条审计——离岗锁定本身也是安全事件。
#[tauri::command]
pub fn session_tick(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut st = lock(&state)?;
    let actor = st
        .snapshot
        .as_ref()
        .map(|s| s.teller_no.clone())
        .unwrap_or_else(|| "unknown".into());

    let locked_reason = st
        .session
        .as_mut()
        .and_then(|guard| guard.evaluate(now_secs()));

    if let Some(reason) = locked_reason {
        audit(&st, &actor, "session_lock", "", "ok", reason.label());
    }
    Ok(session_state_of(&st.session))
}

/// 窗口失焦。是否锁定由策略决定（`lock_on_blur`）。
#[tauri::command]
pub fn session_blur(state: State<'_, Mutex<AppState>>) -> Result<SessionState, String> {
    let mut st = lock(&state)?;
    let actor = st
        .snapshot
        .as_ref()
        .map(|s| s.teller_no.clone())
        .unwrap_or_else(|| "unknown".into());
    let locked_reason = st
        .session
        .as_mut()
        .and_then(|guard| guard.on_blur(now_secs()));
    if let Some(reason) = locked_reason {
        audit(&st, &actor, "session_lock", "", "ok", reason.label());
    }
    Ok(session_state_of(&st.session))
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
    // 业务包：扫描 → 验签 → 依赖拓扑 → 合并菜单
    let mut tree = outcome.loaded.tree;
    let pkg_dir = std::path::Path::new(&st.appdata_dir).join("packages");
    let shell = tellerkit_core::pkg::Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(
        tellerkit_core::pkg::Version {
            major: 0,
            minor: 0,
            patch: 0,
        },
    );
    let (plan, failures) =
        tellerkit_core::pkg::scan_dir(&pkg_dir, &tellerkit_core::mock::demo_verifying_key(), shell);
    let conflicts = tellerkit_core::pkg::merge_packages(&mut tree, &plan.ordered);
    st.packages = plan.ordered;

    st.menu = Some(tree);
    st.last_load = Some(report.clone());
    let mut notes = outcome.notes;
    for package in &st.packages {
        notes.push(format!(
            "已加载业务包 {} v{}",
            package.manifest.id, package.manifest.version
        ));
    }
    for failure in &failures {
        notes.push(format!("跳过业务包 {}：{}", failure.id, failure.reason));
    }
    for conflict in &conflicts {
        notes.push(format!("业务包菜单冲突，保留主壳版本：{conflict}"));
    }
    if let Some(w) = &st.journal_warning {
        notes.push(w.clone());
    }

    Ok(BootInfo {
        shell_version: env!("CARGO_PKG_VERSION").to_string(),
        pubkey_hex: tellerkit_core::mock::demo_pubkey_hex(),
        appdata_dir: st.appdata_dir.clone(),
        offline: st.host.offline,
        local_versions: st.store.list_versions(),
        load: report,
        notes,
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
    st.session = Some(SessionGuard::new(SessionPolicy::default(), now_secs()));
    audit(
        &st,
        &snapshot.teller_no,
        "login",
        &snapshot.outlet_code,
        "ok",
        &format!("角色 {}", snapshot.role_label),
    );
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
    if let Some(snapshot) = &st.snapshot {
        audit(&st, &snapshot.teller_no, "logout", "", "ok", "");
    }
    st.session = None;
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

/// 提交交易。幂等键由前端生成——断网时客户端拿不到服务端 ID。
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
    // 先把不需要引擎的东西取出来，再结束可变借用——否则 audit(&*guard) 借不到
    let actor = guard
        .snapshot
        .as_ref()
        .map(|s| s.teller_no.clone())
        .unwrap_or_else(|| "unknown".into());

    let request = TxRequest {
        tx_code,
        idempotency_key,
        amount_cents,
        elements,
        offline_capable,
    };
    let code = request.tx_code.clone();

    let result = {
        // 拆分借用：引擎要 &mut，主机要 &，两者是同一结构体的不同字段
        let AppState { tx, tx_host, .. } = &mut *guard;
        tx.submit(tx_host, request)
    };

    let detail = format!("status={:?} amount_cents={amount_cents}", result.status);
    audit(
        &guard,
        &actor,
        "submit_tx",
        &code,
        if result.status == tellerkit_core::tx::TxStatus::Committed {
            "ok"
        } else {
            "failed"
        },
        &detail,
    );
    Ok(result)
}

/// 打印凭证。模板与要素分离：套打格式随网点配置变，要素由交易产生。
#[tauri::command]
pub fn print_voucher(
    template: String,
    tx_code: String,
    reference: Option<String>,
    fields: BTreeMap<String, String>,
    state: State<'_, Mutex<AppState>>,
) -> Result<PrintReceipt, String> {
    let st = lock(&state)?;
    // 白名单先于动作：未登记的型号连纸都不该动
    st.devices
        .ensure_allowed(&st.policy)
        .map_err(|e| e.to_string())?;
    let mut voucher = Voucher::new(&template, &tx_code);
    voucher.reference = reference;
    voucher.fields = fields;
    let outcome = st
        .devices
        .printer
        .print(&voucher)
        .map_err(|e| e.to_string());
    let actor = st
        .snapshot
        .as_ref()
        .map(|s| s.teller_no.clone())
        .unwrap_or_else(|| "unknown".into());
    audit(
        &st,
        &actor,
        "print",
        &tx_code,
        if outcome.is_ok() { "ok" } else { "failed" },
        &template,
    );
    outcome
}

/// 返回业务包的模块源码，前端用 Blob URL 加载后调用 `mount(container, shell)`。
///
/// 为什么不返回文件路径：包在运行时是外部文件，WebView 读不到任意本地路径，
/// 而且让前端自己拼路径等于把文件系统访问权交出去。
#[tauri::command]
pub fn package_module(route: String, state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let st = lock(&state)?;
    for package in &st.packages {
        if let Some(module) = package.routes.get(&route) {
            return package
                .assets
                .get(module)
                .cloned()
                .ok_or_else(|| format!("包 {} 的模块 {module} 未随包提供", package.manifest.id));
        }
    }
    Err("该路由不是业务包提供的".to_string())
}

/// 重放挂起交易：跑一轮自动重放调度。
///
/// 不是"一次全推"：每笔按自己的退避节奏到期才重试，失败累加次数，
/// 达到上限交给人工。返回人可读汇总。
#[tauri::command]
pub fn replay_pending(state: State<'_, Mutex<AppState>>) -> Result<String, String> {
    let mut guard = lock(&state)?;
    let actor = guard
        .snapshot
        .as_ref()
        .map(|s| s.teller_no.clone())
        .unwrap_or_else(|| "system".into());

    let (run, journal_error) = {
        let AppState {
            tx,
            tx_host,
            replay,
            ..
        } = &mut *guard;
        let run = tellerkit_core::replay::run_once(tx, replay, tx_host, now_secs());
        (run, tx.take_journal_error())
    };

    if let Some(err) = journal_error {
        audit(&guard, &actor, "replay", "tx_queue", "failed", &err);
    }

    if run.attempted == 0 && run.still_pending == 0 {
        return Ok("没有待重放的交易".to_string());
    }
    if run.attempted == 0 {
        return Ok(format!("还有 {} 笔挂起，均未到重试时间", run.still_pending));
    }

    let detail = format!(
        "attempted={} committed={} pending={} gave_up={}",
        run.attempted,
        run.committed,
        run.still_pending,
        run.gave_up.len()
    );
    audit(&guard, &actor, "replay", "tx_queue", "ok", &detail);

    Ok(format!(
        "重放 {} 笔：成功 {} 笔，仍挂起 {} 笔{}",
        run.attempted,
        run.committed,
        run.still_pending,
        if run.gave_up.is_empty() {
            String::new()
        } else {
            format!("，{} 笔已放弃自动重试，需人工处理", run.gave_up.len())
        }
    ))
}

/// 待重放笔数：界面用它提示"还有 N 笔挂起"。
#[tauri::command]
pub fn tx_pending_count(state: State<'_, Mutex<AppState>>) -> Result<usize, String> {
    let st = lock(&state)?;
    Ok(st.tx.pending_requests().len())
}

#[tauri::command]
pub fn dev_set_offline(
    offline: bool,
    state: State<'_, Mutex<AppState>>,
) -> Result<DevState, String> {
    let mut st = lock(&state)?;
    st.host.offline = offline;
    // 一个开关同时控制菜单探测与交易主机，演示时不用来回切两处
    st.tx_host.set_mode(if offline {
        TxHostMode::Offline
    } else {
        TxHostMode::Online
    });
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

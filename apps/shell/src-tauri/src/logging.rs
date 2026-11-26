//! 客户端结构化日志：JSON 行、按大小轮转、基本脱敏。
//!
//! 为什么自己写而不是引 `tracing` / `tracing-subscriber`：构建与 CI 全部 `--locked`，
//! 新增依赖要走供应链审查；现场排障需要的只是「时间、级别、模块、消息、request_id」，
//! 用已有依赖 `serde_json` 拼一行 JSON 就够，也少一个会把敏感字段顺手带进日志的轮子。
//!
//! 这是**跨层横切件**：不属于任何一层的业务逻辑，L2 命令层与 L3 用例层都可以用。
//!
//! 日志量控制：默认只记 `Info` 以上。命令层**每次调用都写一条**会淹没现场信息
//! （`session_tick` 每秒一次），所以命令只在「失败」或「慢于阈值」时写 Warn，
//! 成功且正常耗时落 Debug（默认不输出）。
//!
//! 脱敏：消息里连续 12 位以上数字（卡号/账号）会被替换成 `***`。金额单位是分，
//! 12 位 = 一百亿元，正常业务不会出现，所以不会误伤金额。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 一条命令超过这个耗时就算"慢"，值得留一条 Warn（默认 3s 是主机超时量级）。
const SLOW_COMMAND_MS: f64 = 1_000.0;

/// 日志级别。顺序即严重度，比较大小用来做阈值过滤。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn as_str(self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "debug" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "warn" | "warning" => Some(Level::Warn),
            "error" => Some(Level::Error),
            _ => None,
        }
    }
}

struct Logger {
    file: File,
    path: PathBuf,
    max_bytes: u64,
    keep: usize,
    min_level: Level,
}

static LOGGER: OnceLock<Mutex<Logger>> = OnceLock::new();

thread_local! {
    /// 当前命令的 request_id。命令层在阻塞池线程里进入作用域，深层日志自动带上。
    static CURRENT_REQUEST: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// request_id 作用域。进作用域时设置线程内的 id，退出时清空（阻塞池线程会复用）。
pub struct RequestScope;

impl RequestScope {
    pub fn enter(request_id: &str) -> Self {
        CURRENT_REQUEST.with(|slot| *slot.borrow_mut() = request_id.to_string());
        Self
    }
}

impl Drop for RequestScope {
    fn drop(&mut self) {
        CURRENT_REQUEST.with(|slot| slot.borrow_mut().clear());
    }
}

fn current_request_id() -> String {
    CURRENT_REQUEST.with(|slot| slot.borrow().clone())
}

/// 初始化日志。写到 `<appdata>/logs/client.log`；返回落盘路径便于启动摘要展示。
///
/// 环境变量：`TK_LOG`（级别，默认 info）、`TK_LOG_MAX_BYTES`（默认 2MiB）、
/// `TK_LOG_KEEP`（保留份数，默认 5）。
pub fn init(appdata: &Path) -> Result<PathBuf, String> {
    let dir = appdata.join("logs");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("创建日志目录失败（{}）：{e}", dir.display()))?;
    let path = dir.join("client.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开日志文件失败（{}）：{e}", path.display()))?;
    let max_bytes = env_u64("TK_LOG_MAX_BYTES", 2 * 1024 * 1024);
    let keep = env_u64("TK_LOG_KEEP", 5).max(1) as usize;
    let min_level = std::env::var("TK_LOG")
        .ok()
        .and_then(|raw| Level::parse(&raw))
        .unwrap_or(Level::Info);
    let logger = Logger {
        file,
        path: path.clone(),
        max_bytes,
        keep,
        min_level,
    };
    // 重复初始化（例如测试里）不算错误：保留第一份即可。
    let _ = LOGGER.set(Mutex::new(logger));
    install_panic_hook();
    Ok(path)
}

fn env_u64(key: &str, fallback: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .unwrap_or(fallback)
}

/// 崩溃也要留下痕迹：panic 发生在命令线程里时，不挂 hook 就只剩前端白屏。
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(Level::Error, "panic", &format!("客户端 panic：{info}"));
        previous(info);
    }));
}

/// 写一条日志，`request_id` 取当前线程的作用域（没有就留空）。
pub fn log(level: Level, target: &str, message: &str) {
    let request_id = current_request_id();
    log_for(level, target, &request_id, message);
}

/// 写一条日志，显式给 `request_id`（命令层的进出/耗时统计用这个）。
pub fn log_for(level: Level, target: &str, request_id: &str, message: &str) {
    let Some(logger) = LOGGER.get() else {
        // 还没初始化（极早期错误）：退回 stderr，别把消息吞掉。
        eprintln!("[{}] {}: {}", level.as_str(), target, message);
        return;
    };
    let Ok(mut logger) = logger.lock() else {
        return;
    };
    if level < logger.min_level {
        return;
    }
    let line = format_line(level, target, request_id, message, now_millis());
    if let Err(e) = logger.write_line(&line) {
        // 日志写不进去不能影响业务：退回 stderr 后继续。
        eprintln!("日志写入失败（{e}）：{line}");
    }
}

#[allow(dead_code)] // 对外日志 API：当前调用点只用 info/warn/error，保留 debug 供后续接入
pub fn debug(target: &str, message: &str) {
    log(Level::Debug, target, message);
}

pub fn info(target: &str, message: &str) {
    log(Level::Info, target, message);
}

pub fn warn(target: &str, message: &str) {
    log(Level::Warn, target, message);
}

pub fn error(target: &str, message: &str) {
    log(Level::Error, target, message);
}

/// 生成一条命令的 `request_id`：毫秒时间 + 进程内单调计数。
///
/// 现场按 id 一串就能把同一次操作的所有日志捞出来，不需要跨进程唯一。
pub fn new_request_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let serial = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) & 0xffff;
    format!("{:x}-{serial:04x}", now_millis())
}

/// 一条命令的收尾记录：失败一律 Warn；成功但慢也 Warn；其余 Debug。
pub fn command_finished(
    command: &str,
    request_id: &str,
    elapsed_ms: f64,
    result: &Result<(), String>,
) {
    match result {
        Err(message) => log_for(
            Level::Warn,
            command,
            request_id,
            &format!("命令失败（{elapsed_ms:.1}ms）：{message}"),
        ),
        Ok(()) if elapsed_ms >= SLOW_COMMAND_MS => log_for(
            Level::Info,
            command,
            request_id,
            &format!("命令偏慢（{elapsed_ms:.1}ms）"),
        ),
        Ok(()) => log_for(
            Level::Debug,
            command,
            request_id,
            &format!("命令完成（{elapsed_ms:.1}ms）"),
        ),
    }
}

impl Logger {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        let incoming = line.len() as u64 + 1;
        let current = self.file.metadata().map(|m| m.len()).unwrap_or(0);
        if should_rotate(current, incoming, self.max_bytes) {
            self.rotate()?;
        }
        writeln!(self.file, "{line}")?;
        self.file.flush()
    }

    /// `client.log` → `client.log.1` → …；最老的被覆盖。
    fn rotate(&mut self) -> std::io::Result<()> {
        for index in (1..self.keep).rev() {
            let from = self.rotated(index);
            if from.exists() {
                let _ = std::fs::rename(&from, self.rotated(index + 1));
            }
        }
        if self.path.exists() {
            let _ = std::fs::rename(&self.path, self.rotated(1));
        }
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        Ok(())
    }

    fn rotated(&self, index: usize) -> PathBuf {
        let name = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "client.log".to_string());
        self.path.with_file_name(format!("{name}.{index}"))
    }
}

/// 一行是否会让文件超过上限。纯函数，便于测试边界。
fn should_rotate(current: u64, incoming: u64, max_bytes: u64) -> bool {
    max_bytes > 0 && current.saturating_add(incoming) > max_bytes
}

/// 脱敏：连续 12 位以上数字（卡号/账号）替换成 `***`；短数字（金额、笔数）保留。
fn redact(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut run = String::new();
    for ch in message.chars() {
        if ch.is_ascii_digit() {
            run.push(ch);
            continue;
        }
        flush_digit_run(&mut out, &mut run);
        out.push(ch);
    }
    flush_digit_run(&mut out, &mut run);
    out
}

fn flush_digit_run(out: &mut String, run: &mut String) {
    if run.len() >= 12 {
        out.push_str("***");
    } else {
        out.push_str(run);
    }
    run.clear();
}

/// 毫秒时间戳（UTC），用于日志行与诊断包排序。
fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn format_line(level: Level, target: &str, request_id: &str, message: &str, ts_ms: u64) -> String {
    let event = serde_json::json!({
        "ts": iso_utc(ts_ms),
        "ts_ms": ts_ms,
        "level": level.as_str(),
        "target": target,
        "request_id": request_id,
        "msg": redact(message),
    });
    serde_json::to_string(&event).unwrap_or_else(|_| {
        format!(
            "{{\"ts_ms\":{ts_ms},\"level\":\"{}\",\"target\":\"{target}\",\"msg\":\"日志序列化失败\"}}",
            level.as_str()
        )
    })
}

/// 毫秒时间戳 → `YYYY-MM-DDTHH:MM:SS.mmmZ`。现场看日志不想先做时间戳换算。
fn iso_utc(ts_ms: u64) -> String {
    let secs = ts_ms / 1_000;
    let millis = ts_ms % 1_000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// 1970-01-01 起的天数 → (年, 月, 日)。Howard Hinnant 的 civil_from_days 公式。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_formats_as_iso_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00.000Z");
        // 2026-11-25T09:00:00Z 的毫秒值（现场排查最常见的形态）
        assert_eq!(iso_utc(1_795_597_200_000), "2026-11-25T09:00:00.000Z");
    }

    #[test]
    fn rotation_triggers_only_when_line_would_exceed_limit() {
        assert!(!should_rotate(100, 50, 150), "正好等于上限不该轮转");
        assert!(should_rotate(100, 51, 150), "超过上限必须轮转");
        assert!(!should_rotate(0, 10_000, 0), "0 表示不轮转");
    }

    #[test]
    fn redact_masks_long_digit_runs_only() {
        let masked = redact("读取账号 6222021234567890123 失败，金额 10000");
        assert!(masked.contains("***"), "卡号必须被盖掉：{masked}");
        assert!(!masked.contains("6222021234567890123"));
        assert!(masked.contains("10000"), "金额不该误伤：{masked}");
    }

    #[test]
    fn formatted_line_is_valid_json_with_fields() {
        let line = format_line(Level::Warn, "submit_tx", "req-1", "慢", 0);
        let value: serde_json::Value = serde_json::from_str(&line).expect("日志行必须是合法 JSON");
        assert_eq!(value["level"], "warn");
        assert_eq!(value["target"], "submit_tx");
        assert_eq!(value["request_id"], "req-1");
        assert_eq!(value["ts"], "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn request_scope_tags_deeper_logs() {
        {
            let _scope = RequestScope::enter("req-scope");
            assert_eq!(current_request_id(), "req-scope");
        }
        assert!(
            current_request_id().is_empty(),
            "退出作用域必须清空，阻塞池线程会复用"
        );
    }

    #[test]
    fn init_writes_a_readable_file() {
        let dir = tempfile::tempdir().expect("临时目录");
        // 用独立的 logger 实例验证轮转，避免依赖全局单例（单例只能初始化一次）。
        let path = dir.path().join("client.log");
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("建文件");
        let mut logger = Logger {
            file,
            path: path.clone(),
            max_bytes: 120,
            keep: 2,
            min_level: Level::Debug,
        };
        for _ in 0..20 {
            logger
                .write_line("0123456789012345678901234567890123456789")
                .expect("写日志");
        }
        assert!(path.exists(), "当前日志文件必须还在");
        assert!(path.with_file_name("client.log.1").exists(), "应发生轮转");
    }
}

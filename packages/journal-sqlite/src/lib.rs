//! 交易审计与待重放队列的 SQLite 适配器。
//!
//! 为什么单独成包：`tellerkit-core` 必须能脱离磁盘做纯逻辑测试（宪法第 2 条）。
//! 数据库依赖放在这里，core 只留 `TxJournal` 端口。

use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult, TxStatus};

/// 一条管理审计事件：谁、在什么时候、对什么、做了什么、结果如何。
///
/// `at` 由 [`SqliteJournal::audit`] 写入，调用方传空字符串即可；
/// 即便传了值也会被实现覆盖，防止业务层伪造时间线。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    pub actor: String,
    pub action: String,
    pub target: String,
    pub result: String,
    pub detail: String,
    pub at: String,
}

/// 本地账本。一个进程一个连接，用 Mutex 串行化（柜面终端是单柜员场景）。
pub struct SqliteJournal {
    conn: Mutex<Connection>,
}

impl SqliteJournal {
    /// 打开（或创建）账本并建表。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("打开账本失败：{e}"))?;
        Self::init(conn)
    }

    /// 内存账本，测试用。
    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("打开内存账本失败：{e}"))?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, String> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            CREATE TABLE IF NOT EXISTS tx_log (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                idempotency_key  TEXT    NOT NULL,
                tx_code          TEXT    NOT NULL,
                amount_cents     INTEGER NOT NULL,
                status           TEXT    NOT NULL,
                reference        TEXT,
                message          TEXT    NOT NULL,
                created_at       TEXT    NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tx_queue (
                idempotency_key  TEXT PRIMARY KEY,
                tx_code          TEXT    NOT NULL,
                amount_cents     INTEGER NOT NULL,
                elements         TEXT    NOT NULL,
                offline_capable  INTEGER NOT NULL,
                created_at       TEXT    NOT NULL
            );
            -- 管理审计：只增表。本类型刻意不提供 update/delete/clear，
            -- 能删的审计等于没有审计（见 docs/security-boundary.md §4）。
            CREATE TABLE IF NOT EXISTS audit_log (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                actor   TEXT    NOT NULL,
                action  TEXT    NOT NULL,
                target  TEXT    NOT NULL,
                result  TEXT    NOT NULL,
                detail  TEXT    NOT NULL,
                at      TEXT    NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("建表失败：{e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 审计日志条数（测试与运维用）。
    pub fn log_count(&self) -> Result<i64, String> {
        self.lock()
            .query_row("SELECT COUNT(*) FROM tx_log", [], |r| r.get(0))
            .map_err(|e| e.to_string())
    }

    /// 某笔交易的结论序列（同一幂等键可能先挂起后成功）。
    pub fn history(&self, idempotency_key: &str) -> Result<Vec<String>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT status FROM tx_log WHERE idempotency_key = ?1 ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map(params![idempotency_key], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    /// 追加一条审计事件。`at` 由这里写入，调用方传什么都会被覆盖。
    ///
    /// 只有追加，没有修改与删除——这是审计日志的意义所在。
    pub fn audit(&self, event: &AuditEvent) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT INTO audit_log (actor, action, target, result, detail, at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    event.actor,
                    event.action,
                    event.target,
                    event.result,
                    event.detail,
                    now(),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("追加审计日志失败：{e}"))
    }

    /// 按时间倒序取最近 `limit` 条。同一秒内按自增 id 倒序，顺序稳定。
    pub fn audit_tail(&self, limit: usize) -> Result<Vec<AuditEvent>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT actor, action, target, result, detail, at
                 FROM audit_log ORDER BY at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| format!("读取审计日志失败：{e}"))?;
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(AuditEvent {
                    actor: r.get(0)?,
                    action: r.get(1)?,
                    target: r.get(2)?,
                    result: r.get(3)?,
                    detail: r.get(4)?,
                    at: r.get(5)?,
                })
            })
            .map_err(|e| format!("读取审计日志失败：{e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取审计日志失败：{e}"))
    }

    /// 审计事件总数（测试与运维用）。
    pub fn audit_count(&self) -> Result<i64, String> {
        self.lock()
            .query_row("SELECT COUNT(*) FROM audit_log", [], |r| r.get(0))
            .map_err(|e| format!("统计审计日志失败：{e}"))
    }
}

fn status_str(status: TxStatus) -> &'static str {
    match status {
        TxStatus::Committed => "committed",
        TxStatus::Pending => "pending",
        TxStatus::Rejected => "rejected",
    }
}

impl TxJournal for SqliteJournal {
    fn record(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT INTO tx_log
                 (idempotency_key, tx_code, amount_cents, status, reference, message, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    request.idempotency_key,
                    request.tx_code,
                    request.amount_cents,
                    status_str(result.status),
                    result.reference,
                    result.message,
                    now(),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("写审计日志失败：{e}"))
    }

    fn enqueue(&self, request: &TxRequest) -> Result<(), String> {
        let elements = serde_json::to_string(&request.elements)
            .map_err(|e| format!("序列化交易要素失败：{e}"))?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO tx_queue
                 (idempotency_key, tx_code, amount_cents, elements, offline_capable, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    request.idempotency_key,
                    request.tx_code,
                    request.amount_cents,
                    elements,
                    request.offline_capable as i32,
                    now(),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("挂起请求入队失败：{e}"))
    }

    fn dequeue(&self, idempotency_key: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "DELETE FROM tx_queue WHERE idempotency_key = ?1",
                params![idempotency_key],
            )
            .map(|_| ())
            .map_err(|e| format!("挂起请求出队失败：{e}"))
    }

    fn pending(&self) -> Result<Vec<TxRequest>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, tx_code, amount_cents, elements, offline_capable
                 FROM tx_queue ORDER BY created_at, rowid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                let elements: String = r.get(3)?;
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    elements,
                    r.get::<_, i32>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;

        let mut out = Vec::new();
        for row in rows {
            let (key, tx_code, amount_cents, elements, offline) = row.map_err(|e| e.to_string())?;
            out.push(TxRequest {
                idempotency_key: key,
                tx_code,
                amount_cents,
                elements: serde_json::from_str(&elements).unwrap_or_default(),
                offline_capable: offline != 0,
            });
        }
        Ok(out)
    }
}

/// 时间戳只用于排序与审计，不参与业务判断。
fn now() -> String {
    // 不引入 chrono：秒级精度够用，格式固定便于按字符串排序
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs:020}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tellerkit_core::tx::{HostError, TxHost};

    fn req(key: &str) -> TxRequest {
        TxRequest::new("account.deposit", key).amount(10_000)
    }

    fn committed() -> TxResult {
        TxResult {
            status: TxStatus::Committed,
            reference: Some("TX-1".into()),
            message: "交易成功".into(),
            replayable: false,
        }
    }

    fn pending() -> TxResult {
        TxResult {
            status: TxStatus::Pending,
            reference: None,
            message: "已挂起".into(),
            replayable: true,
        }
    }

    #[test]
    fn record_appends_to_log() {
        let j = SqliteJournal::in_memory().unwrap();
        j.record(&req("k1"), &committed()).unwrap();
        j.record(&req("k1"), &committed()).unwrap();
        assert_eq!(j.log_count().unwrap(), 2);
    }

    #[test]
    fn enqueue_and_pending_roundtrip() {
        let j = SqliteJournal::in_memory().unwrap();
        let mut r = req("k2").offline(true);
        r.elements.insert("to".into(), "A".into());
        j.enqueue(&r).unwrap();

        let list = j.pending().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].idempotency_key, "k2");
        assert_eq!(list[0].elements.get("to").map(String::as_str), Some("A"));
        assert!(list[0].offline_capable);
    }

    #[test]
    fn dequeue_removes_only_that_key() {
        let j = SqliteJournal::in_memory().unwrap();
        j.enqueue(&req("a")).unwrap();
        j.enqueue(&req("b")).unwrap();
        j.dequeue("a").unwrap();
        let keys: Vec<String> = j
            .pending()
            .unwrap()
            .into_iter()
            .map(|r| r.idempotency_key)
            .collect();
        assert_eq!(keys, vec!["b".to_string()]);
    }

    #[test]
    fn pending_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            j.enqueue(&req("survive")).unwrap();
        }
        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.pending().unwrap().len(), 1, "重启后挂起队列必须还在");
    }

    #[test]
    fn history_keeps_every_transition() {
        let j = SqliteJournal::in_memory().unwrap();
        j.record(&req("k3"), &pending()).unwrap();
        j.record(&req("k3"), &committed()).unwrap();
        assert_eq!(j.history("k3").unwrap(), vec!["pending", "committed"]);
    }

    /// 端到端：引擎挂上 SQLite 后，挂起的交易在"重启"后能被重放。
    #[test]
    fn engine_replays_persisted_queue_after_restart() {
        struct Online;
        impl TxHost for Online {
            fn submit(&self, _r: &TxRequest) -> Result<String, HostError> {
                Ok("TX-REPLAY".into())
            }
        }
        struct Offline;
        impl TxHost for Offline {
            fn submit(&self, _r: &TxRequest) -> Result<String, HostError> {
                Err(HostError::Unavailable)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");

        {
            let journal = SqliteJournal::open(&path).unwrap();
            let mut engine = tellerkit_core::tx::TxEngine::new().with_journal(Box::new(journal));
            let result = engine.submit(&Offline, req("persist-1").offline(true));
            assert_eq!(result.status, TxStatus::Pending);
        }

        // 模拟进程重启：重新打开账本并重建引擎
        let journal = SqliteJournal::open(&path).unwrap();
        let mut engine = tellerkit_core::tx::TxEngine::new().with_journal(Box::new(journal));
        assert_eq!(engine.pending_requests().len(), 1, "重启后应读回挂起请求");

        let outcomes = engine.replay_pending(&Online);
        assert_eq!(outcomes[0].1.status, TxStatus::Committed);
        assert!(engine.pending_requests().is_empty());
        assert!(engine.take_journal_error().is_none(), "记账不应报错");
    }

    fn audit_event(action: &str, target: &str) -> AuditEvent {
        AuditEvent {
            actor: "T001".into(),
            action: action.into(),
            target: target.into(),
            result: "ok".into(),
            detail: String::new(),
            at: String::new(),
        }
    }

    #[test]
    fn audit_appends_and_tail_returns_latest() {
        let j = SqliteJournal::in_memory().unwrap();
        j.audit(&audit_event("login", "T001")).unwrap();
        j.audit(&audit_event("menu_access", "account.open"))
            .unwrap();
        assert_eq!(j.audit_count().unwrap(), 2);

        let tail = j.audit_tail(1).unwrap();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].target, "account.open", "tail(1) 必须返回最新一条");
    }

    #[test]
    fn audit_tail_is_newest_first() {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        let targets: Vec<String> = j
            .audit_tail(10)
            .unwrap()
            .into_iter()
            .map(|e| e.target)
            .collect();
        assert_eq!(targets, ["m3", "m2", "m1"], "审计尾部必须按时间倒序");
        assert_eq!(j.audit_count().unwrap(), 3, "只增：取尾部不改变条数");
    }

    #[test]
    fn audit_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            j.audit(&audit_event("login", "T001")).unwrap();
        }
        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.audit_count().unwrap(), 1, "重启后审计日志必须还在");
        let tail = j.audit_tail(1).unwrap();
        assert_eq!(tail[0].action, "login");
        assert_eq!(tail[0].actor, "T001");
    }

    #[test]
    fn audit_fills_at_itself() {
        let j = SqliteJournal::in_memory().unwrap();
        let mut event = audit_event("submit_tx", "account.deposit");
        event.at = "caller-supplied".into();
        j.audit(&event).unwrap();

        let tail = j.audit_tail(1).unwrap();
        assert!(!tail[0].at.is_empty(), "at 必须由实现写入且非空");
        assert_ne!(tail[0].at, "caller-supplied", "调用方传的 at 必须被覆盖");
        assert!(
            tail[0].at.chars().all(|c| c.is_ascii_digit()),
            "at 是零填充的 epoch 秒，可直接按字符串排序"
        );
    }

    #[test]
    fn audit_keeps_empty_detail() {
        let j = SqliteJournal::in_memory().unwrap();
        let event = audit_event("print", "voucher");
        assert!(event.detail.is_empty());
        j.audit(&event).unwrap();

        let tail = j.audit_tail(1).unwrap();
        assert_eq!(tail[0].detail, "", "空 detail 必须原样存取");
        assert_eq!(tail[0].target, "voucher");
    }

    #[test]
    fn audit_event_serializes_roundtrip() {
        let j = SqliteJournal::in_memory().unwrap();
        j.audit(&audit_event("package_load", "biz.demo")).unwrap();
        let stored = j.audit_tail(1).unwrap().remove(0);

        let json = serde_json::to_string(&stored).unwrap();
        let back: AuditEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back, stored);
    }
}

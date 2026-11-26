//! 交易审计与待重放队列的 SQLite 适配器。
//!
//! 为什么单独成包：`tellerkit-core` 必须能脱离磁盘做纯逻辑测试（宪法第 2 条）。
//! 数据库依赖放在这里，core 只留 `TxJournal` 端口。

mod cipher;

use rusqlite::{params, Connection};

use cipher::audit_chain_hash;
pub use cipher::LedgerCipher;
use std::path::Path;
use std::sync::Mutex;
use tellerkit_core::replay::ReplayAttempt;
use tellerkit_core::reversal::{parse_exception_kind, ExceptionKind, ReversalRecord};
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult, TxStatus};

/// 一条管理审计事件：谁、在什么时候、对什么、做了什么、结果如何。
///
/// `at` 由 [`SqliteJournal::audit`] 写入，调用方传空字符串即可；
/// 即便传了值也会被实现覆盖，防止业务层伪造时间线。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditEvent {
    actor: String,
    action: String,
    target: String,
    result: String,
    detail: String,
    at: String,
}

impl AuditEvent {
    /// `at` 由账本写入时覆盖，调用方不需要（也不应该）传时间。
    pub fn new(actor: &str, action: &str, target: &str, result: &str, detail: &str) -> Self {
        Self {
            actor: actor.to_string(),
            action: action.to_string(),
            target: target.to_string(),
            result: result.to_string(),
            detail: detail.to_string(),
            at: String::new(),
        }
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn result(&self) -> &str {
        &self.result
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    pub fn at(&self) -> &str {
        &self.at
    }
}

/// 审计链头：序号与最新一行的哈希，启动时对外报告用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditChainHead {
    seq: i64,
    hash: String,
}

impl AuditChainHead {
    pub fn seq(&self) -> i64 {
        self.seq
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }
}

/// 本地账本。一个进程一个连接，用 Mutex 串行化（柜面终端是单柜员场景）。
///
/// 交易要素（`tx_queue.elements`）落盘前加密：账号不该以明文躺在磁盘上。
/// 加密的边界与局限见 [`cipher`] 模块文档——**它防的是单文件泄露，不是本地攻击者**。
pub struct SqliteJournal {
    conn: Mutex<Connection>,
    cipher: LedgerCipher,
}

impl SqliteJournal {
    /// 打开（或创建）账本并建表。
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let conn = Connection::open(path).map_err(|e| format!("打开账本失败：{e}"))?;
        let key_path = path.with_file_name("ledger.key");
        let cipher = LedgerCipher::open_or_create(&key_path)?;
        Self::init(conn, cipher)
    }

    /// 内存账本，测试用。密钥不落盘。
    pub fn in_memory() -> Result<Self, String> {
        let conn = Connection::open_in_memory().map_err(|e| format!("打开内存账本失败：{e}"))?;
        Self::init(conn, LedgerCipher::ephemeral()?)
    }

    fn init(conn: Connection, cipher: LedgerCipher) -> Result<Self, String> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            CREATE TABLE IF NOT EXISTS ledger_meta (
                key     TEXT PRIMARY KEY,
                value   TEXT NOT NULL
            );
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
            -- 幂等索引：重启后同键必须命中首次结论，不能重新打主机。
            CREATE TABLE IF NOT EXISTS tx_idempotent (
                idempotency_key  TEXT PRIMARY KEY,
                signature        TEXT    NOT NULL,
                status           TEXT    NOT NULL,
                reference        TEXT,
                message          TEXT    NOT NULL,
                replayable       INTEGER NOT NULL
            );
            -- 待处置异常与冲正：状态不明跨重启必须还在待办清单里。
            CREATE TABLE IF NOT EXISTS tx_exception (
                idempotency_key  TEXT PRIMARY KEY,
                kind             TEXT    NOT NULL
            );
            CREATE TABLE IF NOT EXISTS tx_reversal (
                idempotency_key  TEXT PRIMARY KEY,
                initiator        TEXT    NOT NULL,
                approver         TEXT    NOT NULL,
                reason           TEXT    NOT NULL,
                host_reference   TEXT
            );
            -- 重放进度：次数、下次时刻、是否放弃都跨重启保留。
            CREATE TABLE IF NOT EXISTS replay_attempt (
                idempotency_key  TEXT PRIMARY KEY,
                attempts         INTEGER NOT NULL,
                next_at_secs     INTEGER NOT NULL,
                gave_up          INTEGER NOT NULL
            );
            -- 管理审计：只增表。本类型刻意不提供 update/delete/clear，
            -- 能删的审计等于没有审计（见 docs/security-boundary.md §4）。
            CREATE TABLE IF NOT EXISTS audit_log (
                id      INTEGER PRIMARY KEY AUTOINCREMENT,
                seq     INTEGER NOT NULL DEFAULT 0,
                prev_hash TEXT   NOT NULL DEFAULT '',
                hash    TEXT    NOT NULL DEFAULT '',
                actor   TEXT    NOT NULL,
                action  TEXT    NOT NULL,
                target  TEXT    NOT NULL,
                result  TEXT    NOT NULL,
                detail  TEXT    NOT NULL,
                at      TEXT    NOT NULL
            );
            -- 链头单独存一行：删掉尾部一行也能被发现（末尾不留 seq 空洞）。
            CREATE TABLE IF NOT EXISTS audit_chain_state (
                id        INTEGER PRIMARY KEY CHECK (id = 1),
                last_seq  INTEGER NOT NULL,
                last_hash TEXT    NOT NULL
            );
            INSERT OR IGNORE INTO audit_chain_state (id, last_seq, last_hash)
            VALUES (1, 0, '');
            "#,
        )
        .map_err(|e| format!("建表失败：{e}"))?;
        let mut conn = conn;
        Self::migrate_cipher_format(&mut conn, &cipher)?;
        Self::migrate_audit_chain(&mut conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            cipher,
        })
    }

    /// 旧库一次性把无前缀的交易要素读后重写为密文。
    ///
    /// `ledger_meta` 是迁移开关：新库第一次打开就打标；老库迁移成功后打标。
    /// 打标之后再出现无前缀数据，只可能是被剥离/改写，**直接拒绝启动**，
    /// 否则攻击者只要把前缀去掉就能把篡改伪装成"旧明文"混过迁移。
    fn migrate_cipher_format(conn: &mut Connection, cipher: &LedgerCipher) -> Result<(), String> {
        let migrated = match conn.query_row(
            "SELECT value FROM ledger_meta WHERE key = 'cipher_format'",
            [],
            |r| r.get::<_, String>(0),
        ) {
            Ok(value) => value == "v1",
            Err(rusqlite::Error::QueryReturnedNoRows) => false,
            Err(e) => return Err(format!("读取密文格式标记失败：{e}")),
        };

        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, elements FROM tx_queue
                 WHERE substr(elements, 1, 3) <> 'v1:' ORDER BY rowid",
            )
            .map_err(|e| format!("准备密文迁移查询失败：{e}"))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .map_err(|e| format!("扫描旧版交易要素失败：{e}"))?;
        drop(stmt);

        let tx = conn
            .transaction()
            .map_err(|e| format!("密文迁移事务失败：{e}"))?;
        if rows.is_empty() {
            if !migrated {
                tx.execute(
                    "INSERT OR REPLACE INTO ledger_meta (key, value) VALUES ('cipher_format', 'v1')",
                    [],
                )
                .map_err(|e| format!("写入密文格式标记失败：{e}"))?;
            }
        } else if migrated {
            return Err(format!(
                "账本已启用 v1: 密文格式，却发现 {} 条无前缀交易要素（首个幂等键 {}）；拒绝启动以防密文降级",
                rows.len(),
                rows[0].0
            ));
        } else {
            for (key, plaintext) in &rows {
                let sealed = cipher
                    .migrate_legacy(plaintext)
                    .map_err(|e| format!("迁移交易要素（{key}）失败：{e}"))?;
                tx.execute(
                    "UPDATE tx_queue SET elements = ?1 WHERE idempotency_key = ?2",
                    params![sealed, key.as_str()],
                )
                .map_err(|e| format!("重写交易要素（{key}）失败：{e}"))?;
            }
            tx.execute(
                "INSERT OR REPLACE INTO ledger_meta (key, value) VALUES ('cipher_format', 'v1')",
                [],
            )
            .map_err(|e| format!("写入密文格式标记失败：{e}"))?;
        }
        tx.commit().map_err(|e| format!("密文迁移提交失败：{e}"))
    }

    /// 旧库补链：给升级前写入的审计行按 id 顺序补上 seq/prev_hash/hash。
    ///
    /// 迁移与链头初始化在同一事务里；链头单独存表，删掉尾部行也藏不住。
    /// 链头丢失但日志仍有入链行时直接拒绝启动，不能"重新编号"把删改洗白。
    fn migrate_audit_chain(conn: &mut Connection) -> Result<(), String> {
        let tx = conn
            .transaction()
            .map_err(|e| format!("审计链迁移事务失败：{e}"))?;

        let columns: Vec<String> = {
            let mut stmt = tx
                .prepare("PRAGMA table_info(audit_log)")
                .map_err(|e| format!("读取审计表结构失败：{e}"))?;
            let rows = stmt
                .query_map([], |r| r.get::<_, String>(1))
                .map_err(|e| format!("读取审计表结构失败：{e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取审计表结构失败：{e}"))?
        };
        for (name, declaration) in [
            ("seq", "INTEGER NOT NULL DEFAULT 0"),
            ("prev_hash", "TEXT NOT NULL DEFAULT ''"),
            ("hash", "TEXT NOT NULL DEFAULT ''"),
        ] {
            if !columns.iter().any(|c| c == name) {
                tx.execute(
                    &format!("ALTER TABLE audit_log ADD COLUMN {name} {declaration}"),
                    [],
                )
                .map_err(|e| format!("补齐审计链字段（{name}）失败：{e}"))?;
            }
        }

        let (mut seq, mut prev_hash): (i64, String) = tx
            .query_row(
                "SELECT last_seq, last_hash FROM audit_chain_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("读取审计链头失败：{e}"))?;
        let chained: i64 = tx
            .query_row("SELECT COUNT(*) FROM audit_log WHERE seq > 0", [], |r| {
                r.get(0)
            })
            .map_err(|e| format!("统计已入链审计行失败：{e}"))?;
        if (seq > 0) != (chained > 0) {
            return Err(format!(
                "审计链头与审计日志不一致（链头 seq={seq}，入链行 {chained}）；拒绝启动等待人工核查"
            ));
        }

        let legacy: Vec<(i64, String, String, String, String, String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, actor, action, target, result, detail, at
                     FROM audit_log WHERE seq = 0 ORDER BY id",
                )
                .map_err(|e| format!("扫描旧审计行失败：{e}"))?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                    ))
                })
                .map_err(|e| format!("扫描旧审计行失败：{e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("扫描旧审计行失败：{e}"))?
        };
        for (id, actor, action, target, result, detail, at) in legacy {
            seq = seq
                .checked_add(1)
                .ok_or_else(|| "审计序号溢出，拒绝迁移".to_string())?;
            let parent = prev_hash.clone();
            let hash = audit_chain_hash(
                seq, &parent, &actor, &action, &target, &result, &detail, &at,
            );
            tx.execute(
                "UPDATE audit_log SET seq = ?1, prev_hash = ?2, hash = ?3 WHERE id = ?4",
                params![seq, parent, hash, id],
            )
            .map_err(|e| format!("补链审计行（id={id}）失败：{e}"))?;
            prev_hash = hash;
        }
        tx.execute(
            "UPDATE audit_chain_state SET last_seq = ?1, last_hash = ?2 WHERE id = 1",
            params![seq, prev_hash],
        )
        .map_err(|e| format!("更新审计链头失败：{e}"))?;
        tx.commit().map_err(|e| format!("审计链迁移提交失败：{e}"))
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

    /// 追加一条审计事件。链头读取、哈希计算、插入、链头推进在同一事务内完成。
    ///
    /// 只有追加，没有修改与删除——审计事件自带序号与前序哈希，
    /// 中间删改或末尾删除都会被 [`SqliteJournal::verify_chain`] 发现。
    pub fn audit(&self, event: &AuditEvent) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("审计写入事务失败：{e}"))?;
        let (last_seq, prev_hash): (i64, String) = tx
            .query_row(
                "SELECT last_seq, last_hash FROM audit_chain_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("读取审计链头失败：{e}"))?;
        let seq = last_seq
            .checked_add(1)
            .ok_or_else(|| "审计序号溢出，拒绝写入".to_string())?;
        let at = now();
        let hash = audit_chain_hash(
            seq,
            &prev_hash,
            event.actor(),
            event.action(),
            event.target(),
            event.result(),
            event.detail(),
            &at,
        );
        tx.execute(
            "INSERT INTO audit_log
             (seq, prev_hash, hash, actor, action, target, result, detail, at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                seq,
                prev_hash,
                hash,
                event.actor(),
                event.action(),
                event.target(),
                event.result(),
                event.detail(),
                at,
            ],
        )
        .map_err(|e| format!("追加审计日志失败：{e}"))?;
        tx.execute(
            "UPDATE audit_chain_state SET last_seq = ?1, last_hash = ?2 WHERE id = 1",
            params![seq, hash],
        )
        .map_err(|e| format!("推进审计链头失败：{e}"))?;
        tx.commit().map_err(|e| format!("审计写入提交失败：{e}"))
    }

    /// 全表校验：按 id 顺序逐行核对序号、前序哈希与本行哈希，
    /// 最后和独立存放的链头对账。篡改、删行、乱序、删尾行都会报错。
    pub fn verify_chain(&self) -> Result<AuditChainHead, String> {
        let conn = self.lock();
        let (head_seq, head_hash): (i64, String) = conn
            .query_row(
                "SELECT last_seq, last_hash FROM audit_chain_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("读取审计链头失败：{e}"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, seq, prev_hash, hash, actor, action, target, result, detail, at
                 FROM audit_log ORDER BY id",
            )
            .map_err(|e| format!("校验审计链失败：{e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, String>(8)?,
                    r.get::<_, String>(9)?,
                ))
            })
            .map_err(|e| format!("校验审计链失败：{e}"))?;
        let mut expected_seq = 0_i64;
        let mut expected_prev = String::new();
        for row in rows {
            let (id, seq, prev_hash, hash, actor, action, target, result, detail, at) =
                row.map_err(|e| format!("校验审计链失败：{e}"))?;
            let next = expected_seq
                .checked_add(1)
                .ok_or_else(|| "审计序号溢出，无法校验".to_string())?;
            if seq != next {
                return Err(format!(
                    "审计链序号断裂：id={id} seq={seq}，期望 {next}（删行或乱序）"
                ));
            }
            if prev_hash != expected_prev {
                return Err(format!(
                    "审计链前序哈希断裂：id={id} seq={seq}（删行或重排）"
                ));
            }
            let recomputed = audit_chain_hash(
                seq, &prev_hash, &actor, &action, &target, &result, &detail, &at,
            );
            if hash != recomputed {
                return Err(format!("审计链哈希不匹配：id={id} seq={seq}，疑似篡改"));
            }
            expected_seq = seq;
            expected_prev = hash;
        }
        if expected_seq != head_seq || expected_prev != head_hash {
            return Err(format!(
                "审计链头不一致：表内 seq={expected_seq} hash={}；链头 seq={head_seq} hash={head_hash}（末尾删行或链头被改）",
                shorten(&expected_prev)
            ));
        }
        Ok(AuditChainHead {
            seq: head_seq,
            hash: head_hash,
        })
    }

    /// 不扫全表的链头读取，供审计尾部展示与运维核对。
    pub fn audit_chain_head(&self) -> Result<AuditChainHead, String> {
        let conn = self.lock();
        let (seq, hash): (i64, String) = conn
            .query_row(
                "SELECT last_seq, last_hash FROM audit_chain_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("读取审计链头失败：{e}"))?;
        Ok(AuditChainHead { seq, hash })
    }

    /// 按链上序号倒序取最近 `limit` 条；时钟回拨也不会漏最新事件。
    pub fn audit_tail(&self, limit: usize) -> Result<Vec<AuditEvent>, String> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT actor, action, target, result, detail, at
                 FROM audit_log ORDER BY seq DESC, id DESC LIMIT ?1",
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
        // 状态不明：账本里必须与"明确拒绝"区分开，否则对账时看不出这笔需要查证
        TxStatus::MarkedForReversal => "marked_for_reversal",
    }
}

/// 未知状态报错，不能当成已成功——那会把状态不明的交易误报为已入账。
fn parse_status(raw: &str) -> Result<TxStatus, String> {
    match raw {
        "committed" => Ok(TxStatus::Committed),
        "pending" => Ok(TxStatus::Pending),
        "rejected" => Ok(TxStatus::Rejected),
        "marked_for_reversal" => Ok(TxStatus::MarkedForReversal),
        other => Err(format!("账本中出现未知交易状态：{other}")),
    }
}

fn insert_log(
    tx: &rusqlite::Transaction<'_>,
    request: &TxRequest,
    result: &TxResult,
) -> Result<(), String> {
    tx.execute(
        "INSERT INTO tx_log
         (idempotency_key, tx_code, amount_cents, status, reference, message, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            request.idempotency_key(),
            request.tx_code(),
            request.amount_cents(),
            status_str(result.status()),
            result.reference(),
            result.message(),
            now(),
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("写审计日志失败：{e}"))
}

fn seal_elements(request: &TxRequest, cipher: &LedgerCipher) -> Result<String, String> {
    let plain = serde_json::to_string(request.elements())
        .map_err(|e| format!("序列化交易要素失败：{e}"))?;
    cipher.seal(&plain)
}

fn insert_queue(
    tx: &rusqlite::Transaction<'_>,
    request: &TxRequest,
    elements: &str,
) -> Result<(), String> {
    tx.execute(
        "INSERT OR REPLACE INTO tx_queue
         (idempotency_key, tx_code, amount_cents, elements, offline_capable, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            request.idempotency_key(),
            request.tx_code(),
            request.amount_cents(),
            elements,
            request.offline_capable() as i32,
            now(),
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("挂起请求入队失败：{e}"))
}

fn upsert_idempotent(
    tx: &rusqlite::Transaction<'_>,
    key: &str,
    signature: &str,
    result: &TxResult,
) -> Result<(), String> {
    tx.execute(
        "INSERT OR REPLACE INTO tx_idempotent
         (idempotency_key, signature, status, reference, message, replayable)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            key,
            signature,
            status_str(result.status()),
            result.reference(),
            result.message(),
            result.replayable() as i32,
        ],
    )
    .map(|_| ())
    .map_err(|e| format!("幂等索引落盘失败：{e}"))
}

/// 一条按状态聚合的对账行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSummary {
    status: String,
    count: i64,
    amount_cents: i64,
}

impl StatusSummary {
    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn count(&self) -> i64 {
        self.count
    }

    pub fn amount_cents(&self) -> i64 {
        self.amount_cents
    }
}

impl SqliteJournal {
    /// 按状态聚合交易（日终对账用）。
    ///
    /// `since` 是零填充的秒级时间戳字符串（与 `created_at` 同格式），
    /// 传空串表示不限时间。**只读**：不改任何状态。
    pub fn summary(&self, since: &str) -> Result<Vec<StatusSummary>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT status, COUNT(*), COALESCE(SUM(amount_cents), 0)
                 FROM tx_log WHERE created_at >= ?1
                 GROUP BY status ORDER BY status",
            )
            .map_err(|e| format!("准备对账查询失败：{e}"))?;
        let rows = stmt
            .query_map(params![since], |r| {
                Ok(StatusSummary {
                    status: r.get(0)?,
                    count: r.get(1)?,
                    amount_cents: r.get(2)?,
                })
            })
            .map_err(|e| format!("执行对账查询失败：{e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取对账结果失败：{e}"))?);
        }
        Ok(out)
    }

    /// 最近 N 条交易记录（对账时要能点开看具体是哪几笔）。
    pub fn recent(&self, limit: usize) -> Result<Vec<RecentTx>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, tx_code, amount_cents, status, reference, created_at
                 FROM tx_log ORDER BY id DESC LIMIT ?1",
            )
            .map_err(|e| format!("准备明细查询失败：{e}"))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = stmt
            .query_map(params![limit], |r| {
                Ok(RecentTx {
                    idempotency_key: r.get(0)?,
                    tx_code: r.get(1)?,
                    amount_cents: r.get(2)?,
                    status: r.get(3)?,
                    reference: r.get(4)?,
                    at: r.get(5)?,
                })
            })
            .map_err(|e| format!("执行明细查询失败：{e}"))?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|e| format!("读取交易明细失败：{e}"))?);
        }
        Ok(out)
    }
}

/// 一笔交易的简报（对账明细）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentTx {
    idempotency_key: String,
    tx_code: String,
    amount_cents: i64,
    status: String,
    reference: Option<String>,
    at: String,
}

impl RecentTx {
    pub fn idempotency_key(&self) -> &str {
        &self.idempotency_key
    }

    pub fn tx_code(&self) -> &str {
        &self.tx_code
    }

    pub fn amount_cents(&self) -> i64 {
        self.amount_cents
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn reference(&self) -> &Option<String> {
        &self.reference
    }

    pub fn at(&self) -> &str {
        &self.at
    }
}

impl TxJournal for SqliteJournal {
    fn record(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("结论事务失败：{e}"))?;
        insert_log(&tx, request, result)?;
        tx.commit().map_err(|e| format!("结论提交失败：{e}"))
    }

    /// 结论与挂起入队同一事务：提交前崩溃则两样都不留，绝不产生"有结论没队列"。
    fn record_and_enqueue(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        let elements = seal_elements(request, &self.cipher)?;
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("挂起事务失败：{e}"))?;
        insert_log(&tx, request, result)?;
        if result.status() == TxStatus::Pending {
            insert_queue(&tx, request, &elements)?;
        }
        tx.commit().map_err(|e| format!("挂起提交失败：{e}"))
    }

    /// 终态收口：结论、出队、幂等索引同一事务。
    fn settle(
        &self,
        request: &TxRequest,
        result: &TxResult,
        signature: Option<&str>,
        dequeue: bool,
    ) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("收口事务失败：{e}"))?;
        insert_log(&tx, request, result)?;
        if dequeue {
            tx.execute(
                "DELETE FROM tx_queue WHERE idempotency_key = ?1",
                params![request.idempotency_key()],
            )
            .map_err(|e| format!("挂起请求出队失败：{e}"))?;
        }
        if let Some(signature) = signature {
            upsert_idempotent(&tx, request.idempotency_key(), signature, result)?;
        }
        tx.commit().map_err(|e| format!("收口提交失败：{e}"))
    }

    fn load_idempotent(&self) -> Result<Vec<(String, String, TxResult)>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, signature, status, reference, message, replayable
                 FROM tx_idempotent ORDER BY rowid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i32>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (key, signature, status, reference, message, replayable) =
                row.map_err(|e| e.to_string())?;
            out.push((
                key,
                signature,
                TxResult::restore(parse_status(&status)?, reference, message, replayable != 0),
            ));
        }
        Ok(out)
    }

    fn save_idempotent(&self, key: &str, signature: &str, result: &TxResult) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("幂等事务失败：{e}"))?;
        upsert_idempotent(&tx, key, signature, result)?;
        tx.commit().map_err(|e| format!("幂等提交失败：{e}"))
    }

    fn enqueue(&self, request: &TxRequest) -> Result<(), String> {
        let elements = seal_elements(request, &self.cipher)?;
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("挂起事务失败：{e}"))?;
        insert_queue(&tx, request, &elements)?;
        tx.commit().map_err(|e| format!("挂起提交失败：{e}"))
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
            let (key, tx_code, amount_cents, stored, offline) = row.map_err(|e| e.to_string())?;
            // 解密失败必须报错，不能默认成空要素——那样会把交易变成另一笔交易
            let plain = self.cipher.open(&stored)?;
            let elements = serde_json::from_str(&plain)
                .map_err(|e| format!("交易要素解析失败（{key}）：{e}"))?;
            out.push(
                TxRequest::new(&tx_code, &key)
                    .amount(amount_cents)
                    .with_elements(elements)
                    .offline(offline != 0),
            );
        }
        Ok(out)
    }

    fn load_attempts(&self) -> Result<Vec<ReplayAttempt>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT idempotency_key, attempts, next_at_secs, gave_up FROM replay_attempt")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i32>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (key, attempts, next_at, gave_up) = row.map_err(|e| e.to_string())?;
            out.push(
                ReplayAttempt::new(&key, attempts as u32, next_at as u64)
                    .with_gave_up(gave_up != 0),
            );
        }
        Ok(out)
    }

    fn upsert_attempt(&self, attempt: &ReplayAttempt) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO replay_attempt
                 (idempotency_key, attempts, next_at_secs, gave_up) VALUES (?1, ?2, ?3, ?4)",
                params![
                    attempt.key(),
                    attempt.attempts() as i64,
                    attempt.next_at_secs() as i64,
                    attempt.gave_up() as i32,
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("重放进度落盘失败：{e}"))
    }

    fn delete_attempt(&self, key: &str) -> Result<(), String> {
        self.lock()
            .execute(
                "DELETE FROM replay_attempt WHERE idempotency_key = ?1",
                params![key],
            )
            .map(|_| ())
            .map_err(|e| format!("重放进度删除失败：{e}"))
    }

    fn load_exceptions(&self) -> Result<Vec<(String, ExceptionKind)>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT idempotency_key, kind FROM tx_exception")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (key, kind) = row.map_err(|e| e.to_string())?;
            out.push((key, parse_exception_kind(&kind)?));
        }
        Ok(out)
    }

    fn upsert_exception(&self, key: &str, kind: ExceptionKind) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO tx_exception (idempotency_key, kind) VALUES (?1, ?2)",
                params![key, tellerkit_core::reversal::exception_kind_str(kind)],
            )
            .map(|_| ())
            .map_err(|e| format!("待冲正标记落盘失败：{e}"))
    }

    fn load_reversals(&self) -> Result<Vec<ReversalRecord>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, initiator, approver, reason, host_reference
                 FROM tx_reversal",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for row in rows {
            let (key, initiator, approver, reason, host_reference) =
                row.map_err(|e| e.to_string())?;
            out.push(ReversalRecord::new(
                &key,
                &initiator,
                &approver,
                &reason,
                host_reference,
            ));
        }
        Ok(out)
    }

    fn upsert_reversal(&self, record: &ReversalRecord) -> Result<(), String> {
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO tx_reversal
                 (idempotency_key, initiator, approver, reason, host_reference)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    record.idempotency_key(),
                    record.initiator(),
                    record.approver(),
                    record.reason(),
                    record.host_reference(),
                ],
            )
            .map(|_| ())
            .map_err(|e| format!("冲正记录落盘失败：{e}"))
    }
}

/// 时间戳只用于排序与审计，不参与业务判断。
fn shorten(raw: &str) -> &str {
    if raw.len() > 16 {
        &raw[..16]
    } else {
        raw
    }
}

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
        TxResult::committed("TX-1".into())
    }

    fn pending() -> TxResult {
        TxResult::pending("已挂起")
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
        r.elements_mut().insert("to".into(), "A".into());
        j.enqueue(&r).unwrap();

        let list = j.pending().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].idempotency_key(), "k2");
        assert_eq!(list[0].elements().get("to").map(String::as_str), Some("A"));
        assert!(list[0].offline_capable());
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
            .map(|r| r.idempotency_key().to_string())
            .collect();
        assert_eq!(keys, vec!["b".to_string()]);
    }

    #[test]
    fn summary_aggregates_by_status_and_amount() {
        let dir = tempfile::tempdir().unwrap();
        let j = SqliteJournal::open(dir.path().join("tx.db")).unwrap();

        j.record(&req("a"), &committed()).unwrap();
        j.record(&req("b"), &committed()).unwrap();
        j.record(&req("c"), &TxResult::rejected("余额不足"))
            .unwrap();

        let rows = j.summary("").unwrap();
        let committed = rows.iter().find(|r| r.status == "committed").unwrap();
        assert_eq!(committed.count, 2);
        assert_eq!(committed.amount_cents, 2 * req("a").amount_cents());

        let rejected = rows.iter().find(|r| r.status == "rejected").unwrap();
        assert_eq!(rejected.count, 1);
    }

    #[test]
    fn summary_respects_since_and_recent_is_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let j = SqliteJournal::open(dir.path().join("tx.db")).unwrap();
        j.record(&req("old"), &committed()).unwrap();
        j.record(&req("new"), &committed()).unwrap();

        // 未来时间点：什么都统计不到（证明 since 生效）
        let future = format!("{:020}", u64::MAX / 2);
        assert!(j.summary(future.as_str()).unwrap().is_empty());

        let recent = j.recent(10).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].idempotency_key, "new", "最近的在前");
        assert_eq!(recent[0].reference.as_deref(), Some("TX-1"));
    }

    #[test]
    fn elements_are_encrypted_at_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        let account = "6222021234567890123";
        {
            let j = SqliteJournal::open(&path).unwrap();
            let mut r = req("k-enc").offline(true);
            r.elements_mut().insert("account".into(), account.into());
            j.enqueue(&r).unwrap();
        }

        // 直接扫磁盘字节（含 WAL）：卡号不得以明文出现
        let needle = account.as_bytes();
        for entry in std::fs::read_dir(dir.path()).unwrap() {
            let p = entry.unwrap().path();
            let raw = std::fs::read(&p).unwrap_or_default();
            assert!(
                !raw.windows(needle.len()).any(|w| w == needle),
                "{} 里出现了明文卡号",
                p.display()
            );
        }

        // 但重新打开必须能解出来（加密不能变成丢数据）
        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.pending().unwrap()[0].elements()["account"], account);
    }

    #[test]
    fn corrupt_ciphertext_fails_loudly_instead_of_dropping_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            let mut r = req("k-bad").offline(true);
            r.elements_mut().insert("account".into(), "6222".into());
            j.enqueue(&r).unwrap();
        }
        // 把密文改成不合法的格式
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE tx_queue SET elements = 'v1:zz:zz' WHERE idempotency_key = 'k-bad'",
                [],
            )
            .unwrap();
        }

        let j = SqliteJournal::open(&path).unwrap();
        let err = j.pending().unwrap_err();
        assert!(err.contains("nonce") || err.contains("密文"), "实际：{err}");
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
            assert_eq!(result.status(), TxStatus::Pending);
        }

        // 模拟进程重启：重新打开账本并重建引擎
        let journal = SqliteJournal::open(&path).unwrap();
        let mut engine = tellerkit_core::tx::TxEngine::new().with_journal(Box::new(journal));
        assert_eq!(engine.pending_count(), 1, "重启后应读回挂起请求");

        let outcomes = engine.replay_pending(&Online);
        assert_eq!(outcomes[0].1.status(), TxStatus::Committed);
        assert!(
            !outcomes[0].1.message().contains("记账失败"),
            "记账不应报错"
        );
        assert_eq!(engine.pending_count(), 0);
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

    #[test]
    fn audit_chain_detects_tampered_middle_row() {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        assert_eq!(j.verify_chain().unwrap().seq(), 3);

        {
            let conn = j.lock();
            conn.execute("UPDATE audit_log SET detail = '篡改' WHERE seq = 2", [])
                .unwrap();
        }
        let err = j.verify_chain().unwrap_err();
        assert!(err.contains("哈希不匹配"), "实际：{err}");
    }

    #[test]
    fn audit_chain_detects_deleted_rows_and_tail_deletion() {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        {
            let conn = j.lock();
            conn.execute("DELETE FROM audit_log WHERE seq = 2", [])
                .unwrap();
        }
        let err = j.verify_chain().unwrap_err();
        assert!(err.contains("序号断裂"), "实际：{err}");

        // 重开一份干净链，只删最后一条：没有 seq 空洞，靠独立链头发现。
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        {
            let conn = j.lock();
            conn.execute("DELETE FROM audit_log WHERE seq = 3", [])
                .unwrap();
        }
        let err = j.verify_chain().unwrap_err();
        assert!(err.contains("链头不一致"), "实际：{err}");
    }

    #[test]
    fn audit_chain_rejects_reordered_rows() {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        {
            let conn = j.lock();
            conn.execute(
                "UPDATE audit_log SET seq = CASE seq WHEN 1 THEN 2 WHEN 2 THEN 1 END",
                [],
            )
            .unwrap();
        }
        let err = j.verify_chain().unwrap_err();
        assert!(err.contains("序号断裂"), "实际：{err}");
    }

    #[test]
    fn audit_tail_orders_by_seq_not_by_clock() {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        {
            let conn = j.lock();
            // 模拟时钟回拨：最新事件的时间戳反而最小。
            conn.execute(
                "UPDATE audit_log SET at = '00000000000000000001' WHERE seq = 3",
                [],
            )
            .unwrap();
        }
        assert_eq!(j.audit_tail(1).unwrap()[0].target, "m3");
    }

    #[test]
    fn audit_write_failure_propagates_without_touching_chain() {
        let j = SqliteJournal::in_memory().unwrap();
        {
            let conn = j.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_audit BEFORE INSERT ON audit_log
                 WHEN NEW.actor = 'blocked'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        }
        let mut event = audit_event("login", "T001");
        event.actor = "blocked".into();
        let err = j.audit(&event).unwrap_err();
        assert!(err.contains("追加审计日志失败"), "实际：{err}");
        assert_eq!(j.audit_count().unwrap(), 0, "失败写入不得留下半条链");
        assert_eq!(j.verify_chain().unwrap().seq(), 0, "链头不得被推进");
    }

    #[test]
    fn legacy_audit_rows_are_chained_on_open_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE audit_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    actor TEXT NOT NULL, action TEXT NOT NULL, target TEXT NOT NULL,
                    result TEXT NOT NULL, detail TEXT NOT NULL, at TEXT NOT NULL
                 );
                 INSERT INTO audit_log (actor, action, target, result, detail, at)
                 VALUES ('T001','login','old1','ok','','1'),
                        ('T001','menu_access','old2','ok','','2');",
            )
            .unwrap();
        }
        {
            let j = SqliteJournal::open(&path).unwrap();
            assert_eq!(j.verify_chain().unwrap().seq(), 2, "旧行按 id 补链");
            assert_eq!(j.audit_tail(1).unwrap()[0].target, "old2");
            j.audit(&audit_event("login", "T002")).unwrap();
            assert_eq!(j.verify_chain().unwrap().seq(), 3);
        }
        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.verify_chain().unwrap().seq(), 3, "重开不得二次补链");
        // 迁移前的数据被删改也要能被发现。
        {
            let conn = j.lock();
            conn.execute("UPDATE audit_log SET target = 'forged' WHERE seq = 1", [])
                .unwrap();
        }
        assert!(j.verify_chain().is_err());
    }

    #[test]
    fn audit_chain_hash_binds_all_fields() {
        let base = audit_chain_hash(1, "", "a", "b", "c", "ok", "d", "t");
        assert_ne!(base, audit_chain_hash(2, "", "a", "b", "c", "ok", "d", "t"));
        assert_ne!(
            base,
            audit_chain_hash(1, "", "a", "b", "c", "denied", "d", "t")
        );
        assert_ne!(
            base,
            audit_chain_hash(1, "", "ab", "", "c", "ok", "d", "t"),
            "长度前缀必须消除字段拼接歧义"
        );
    }

    #[test]
    fn settle_is_atomic_when_idempotent_write_fails() {
        let j = SqliteJournal::in_memory().unwrap();
        j.enqueue(&req("k-fail")).unwrap();
        {
            let conn = j.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_idem BEFORE INSERT ON tx_idempotent
                 WHEN NEW.idempotency_key = 'k-fail'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        }

        assert!(j
            .settle(&req("k-fail"), &committed(), Some("sig"), true)
            .is_err());
        assert_eq!(j.log_count().unwrap(), 0, "事务失败不得留下半条结论");
        assert_eq!(j.pending().unwrap().len(), 1, "出队必须一起回滚");
        assert!(j.load_idempotent().unwrap().is_empty());
    }

    #[test]
    fn record_and_enqueue_is_atomic_when_queue_write_fails() {
        let j = SqliteJournal::in_memory().unwrap();
        {
            let conn = j.lock();
            conn.execute_batch(
                "CREATE TRIGGER fail_queue BEFORE INSERT ON tx_queue
                 WHEN NEW.idempotency_key = 'k-fail2'
                 BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            )
            .unwrap();
        }

        assert!(j.record_and_enqueue(&req("k-fail2"), &pending()).is_err());
        assert_eq!(j.log_count().unwrap(), 0, "入队失败不得留下无队列的结论");
        assert!(j.pending().unwrap().is_empty());
    }

    #[test]
    fn settle_records_conclusion_dequeue_and_idempotent_together() {
        let j = SqliteJournal::in_memory().unwrap();
        j.enqueue(&req("k-settle")).unwrap();
        j.settle(&req("k-settle"), &committed(), Some("sig"), true)
            .unwrap();

        assert_eq!(j.log_count().unwrap(), 1);
        assert!(j.pending().unwrap().is_empty());
        let rows = j.load_idempotent().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].1, "sig");
        assert_eq!(rows[0].2, committed());
    }

    #[test]
    fn recovery_state_survives_reopen() {
        use tellerkit_core::reversal::ExceptionKind;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            j.save_idempotent("k-1", "sig-1", &committed()).unwrap();
            j.upsert_exception("k-1", ExceptionKind::MarkedForReversal)
                .unwrap();
            j.upsert_reversal(&ReversalRecord::new(
                "k-1",
                "teller",
                "chief",
                "查账后冲正",
                None,
            ))
            .unwrap();
            j.upsert_attempt(&ReplayAttempt::new("k-2", 3, 999).with_gave_up(true))
                .unwrap();
        }

        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.load_idempotent().unwrap()[0].1, "sig-1");
        assert_eq!(
            j.load_exceptions().unwrap(),
            vec![("k-1".to_string(), ExceptionKind::MarkedForReversal)]
        );
        let reversals = j.load_reversals().unwrap();
        assert_eq!(reversals.len(), 1);
        assert_eq!(reversals[0].approver(), "chief");
        let attempts = j.load_attempts().unwrap();
        assert_eq!(attempts[0].attempts(), 3, "失败预算跨重启不重置");
        assert_eq!(attempts[0].next_at_secs(), 999);
        assert!(attempts[0].gave_up(), "放弃状态跨重启保留");
        j.delete_attempt("k-2").unwrap();
        assert!(j.load_attempts().unwrap().is_empty());
    }

    #[test]
    fn prefix_stripping_is_rejected_after_migration_marker_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            j.enqueue(&req("k-downgrade")).unwrap();
        }
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE tx_queue SET elements = '{\"amount\":999}' WHERE idempotency_key = 'k-downgrade'",
                [],
            )
            .unwrap();
        }

        let err = SqliteJournal::open(&path)
            .err()
            .expect("第二次打开应被拒绝");
        assert!(err.contains("降级"), "实际：{err}");
    }

    #[test]
    fn legacy_plaintext_is_migrated_once_on_open() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            j.enqueue(&req("k-legacy")).unwrap();
        }
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute(
                "UPDATE tx_queue SET elements = ?1 WHERE idempotency_key = 'k-legacy'",
                params![r#"{"legacy":"yes"}"#],
            )
            .unwrap();
            // 抹掉迁移标记，模拟升级前的旧库。
            conn.execute("DELETE FROM ledger_meta", []).unwrap();
        }

        {
            let j = SqliteJournal::open(&path).unwrap();
            assert_eq!(j.pending().unwrap()[0].elements()["legacy"], "yes");
            let conn = j.lock();
            let stored: String = conn
                .query_row(
                    "SELECT elements FROM tx_queue WHERE idempotency_key = 'k-legacy'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(stored.starts_with("v1:"), "旧明文必须已被重写为密文");
        }
        // 第二次打开不得再迁移，也不能因已加密的数据报错。
        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.pending().unwrap().len(), 1);
    }

    struct Rejecting;
    impl TxHost for Rejecting {
        fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
            Err(HostError::Rejected("余额不足".into()))
        }
    }

    struct Offline;
    impl TxHost for Offline {
        fn submit(&self, _request: &TxRequest) -> Result<String, HostError> {
            Err(HostError::Unavailable)
        }
    }

    #[test]
    fn rejected_replay_one_is_recorded_and_dequeued() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let journal = SqliteJournal::open(&path).unwrap();
            let mut engine = tellerkit_core::tx::TxEngine::new().with_journal(Box::new(journal));
            assert_eq!(
                engine
                    .submit(&Offline, req("k-rej-one").offline(true))
                    .status(),
                TxStatus::Pending
            );
            let (_, result) = engine.replay_one(&Rejecting, "k-rej-one").unwrap();
            assert_eq!(result.status(), TxStatus::Rejected);
        }

        let journal = SqliteJournal::open(&path).unwrap();
        assert!(journal.pending().unwrap().is_empty(), "终态不得留在队列");
        assert_eq!(
            journal.history("k-rej-one").unwrap(),
            vec!["pending".to_string(), "rejected".to_string()]
        );
        let rows = journal.load_idempotent().unwrap();
        assert_eq!(rows[0].2.status(), TxStatus::Rejected);
    }

    #[test]
    fn rejected_replay_pending_is_recorded_and_dequeued() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let journal = SqliteJournal::open(&path).unwrap();
            let mut engine = tellerkit_core::tx::TxEngine::new().with_journal(Box::new(journal));
            engine.submit(&Offline, req("k-rej-all").offline(true));
            let outcomes = engine.replay_pending(&Rejecting);
            assert_eq!(outcomes[0].1.status(), TxStatus::Rejected);
            assert_eq!(engine.pending_count(), 0);
        }

        let journal = SqliteJournal::open(&path).unwrap();
        assert!(journal.pending().unwrap().is_empty());
        assert_eq!(journal.history("k-rej-all").unwrap().len(), 2);
        assert_eq!(
            journal.load_idempotent().unwrap()[0].2.status(),
            TxStatus::Rejected
        );
    }
}

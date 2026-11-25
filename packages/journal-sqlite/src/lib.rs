//! 交易审计与待重放队列的 SQLite 适配器。
//!
//! 为什么单独成包：`tellerkit-core` 必须能脱离磁盘做纯逻辑测试（宪法第 2 条）。
//! 数据库依赖放在这里，core 只留 `TxJournal` 端口。
//!
//! 审计表带哈希链：`seq` 单调、`prev_hash` 指向上一条的 `hash`，
//! [`SqliteJournal::audit_verify`] 全表扫描可发现改字段/删行/重排。
//! 链头单独存在 `audit_chain_state`，以便发现**尾部删行**。

mod cipher;

use rusqlite::{params, Connection};

pub use cipher::LedgerCipher;
use std::path::Path;
use std::sync::Mutex;
use tellerkit_core::tx::{TxJournal, TxRequest, TxResult, TxStatus};

/// 哈希域分隔符：防止域外数据被拼进本链的哈希输入。
const CHAIN_DOMAIN: &[u8] = b"tellerkit-audit-chain-v1\0";

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

/// 全表校验时发现的第一处问题（按 `audit_log.id` 升序）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditChainIssue {
    /// 出问题的行号；链头不一致时为 0。
    pub id: i64,
    /// 该行记录的序号；链头不一致时为链头期待的序号。
    pub seq: i64,
    /// 机器可读的问题类型：`sequence_gap` / `broken_prev_hash` / `hash_mismatch` /
    /// `legacy_in_chain` / `head_mismatch`。
    pub kind: String,
    /// 给人看的中文说明。
    pub message: String,
}

/// 审计链全表校验结果。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditVerifyReport {
    /// 带哈希的链条完整且链头一致。旧库迁移前的行不计入（见 `legacy`）。
    pub ok: bool,
    /// 本次真正校验过的带哈希事件数。
    pub checked: usize,
    /// 迁移前没有哈希、因而无法证明不可篡改的行数。
    pub legacy: usize,
    /// 第一处篡改/断链位置；`ok` 为真时为空。
    pub first_issue: Option<AuditChainIssue>,
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

    fn init(mut conn: Connection, cipher: LedgerCipher) -> Result<Self, String> {
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
            .map_err(|e| format!("设置账本模式失败：{e}"))?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("升级账本失败：{e}"))?;
        tx.execute_batch(
            r#"
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
            -- 写前意图：调用主机之前先落盘。只有 settle() 才会删除，
            -- 崩溃遗留的行即"主机可能已受理但结论未记"的交易，启动时转异常。
            CREATE TABLE IF NOT EXISTS tx_intent (
                idempotency_key  TEXT PRIMARY KEY,
                tx_code          TEXT    NOT NULL,
                amount_cents     INTEGER NOT NULL,
                elements         TEXT    NOT NULL,
                offline_capable  INTEGER NOT NULL,
                created_at       TEXT    NOT NULL
            );
            -- 管理审计：只增表。本类型刻意不提供 update/delete/clear，
            -- 能删的审计等于没有审计（见 docs/security-boundary.md §4）。
            -- seq/prev_hash/hash 默认空：旧库升级后原有行仍是"无哈希旧行"。
            CREATE TABLE IF NOT EXISTS audit_log (
                id        INTEGER PRIMARY KEY AUTOINCREMENT,
                actor     TEXT    NOT NULL,
                action    TEXT    NOT NULL,
                target    TEXT    NOT NULL,
                result    TEXT    NOT NULL,
                detail    TEXT    NOT NULL,
                at        TEXT    NOT NULL,
                seq       INTEGER NOT NULL DEFAULT 0,
                prev_hash TEXT    NOT NULL DEFAULT '',
                hash      TEXT    NOT NULL DEFAULT ''
            );
            -- 链头放在审计表之外：尾部删行也能被发现；它只是元数据，不是事件。
            CREATE TABLE IF NOT EXISTS audit_chain_state (
                id        INTEGER PRIMARY KEY CHECK (id = 1),
                last_seq  INTEGER NOT NULL,
                last_hash TEXT    NOT NULL
            );
            INSERT OR IGNORE INTO audit_chain_state (id, last_seq, last_hash) VALUES (1, 0, '');
            "#,
        )
        .map_err(|e| format!("建表失败：{e}"))?;
        ensure_audit_chain_columns(&tx)?;
        tx.commit().map_err(|e| format!("升级账本失败：{e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
            cipher,
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
    /// 写事件与推进链头在同一事务里，避免中途失败留下断链。
    pub fn audit(&self, event: &AuditEvent) -> Result<(), String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启审计事务失败：{e}"))?;
        let (last_seq, last_hash): (i64, String) = tx
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
        let hash = chain_hash(seq, &last_hash, event, &at);

        tx.execute(
            "INSERT INTO audit_log (actor, action, target, result, detail, at, seq, prev_hash, hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                event.actor,
                event.action,
                event.target,
                event.result,
                event.detail,
                at,
                seq,
                last_hash,
                hash,
            ],
        )
        .map_err(|e| format!("追加审计日志失败：{e}"))?;
        tx.execute(
            "UPDATE audit_chain_state SET last_seq = ?1, last_hash = ?2 WHERE id = 1",
            params![seq, hash],
        )
        .map_err(|e| format!("推进审计链头失败：{e}"))?;
        tx.commit().map_err(|e| format!("提交审计日志失败：{e}"))
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

    /// 扫描全表校验哈希链，报告第一处篡改/断链位置。只读，不修链、不删行。
    ///
    /// 旧库迁移前的行没有哈希，计入 `legacy` 且不算篡改；但它们无法证明不可抵赖，
    /// 调用方若要求全量可验，应同时要求 `legacy == 0`。
    pub fn audit_verify(&self) -> Result<AuditVerifyReport, String> {
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("开启审计校验失败：{e}"))?;
        let (head_seq, head_hash): (i64, String) = tx
            .query_row(
                "SELECT last_seq, last_hash FROM audit_chain_state WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| format!("读取审计链头失败：{e}"))?;

        let mut stmt = tx
            .prepare(
                "SELECT id, seq, prev_hash, hash, actor, action, target, result, detail, at
                 FROM audit_log ORDER BY id",
            )
            .map_err(|e| format!("读取审计链失败：{e}"))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    AuditEvent {
                        actor: r.get(4)?,
                        action: r.get(5)?,
                        target: r.get(6)?,
                        result: r.get(7)?,
                        detail: r.get(8)?,
                        at: r.get(9)?,
                    },
                ))
            })
            .map_err(|e| format!("读取审计链失败：{e}"))?;

        let mut report = AuditVerifyReport {
            ok: true,
            checked: 0,
            legacy: 0,
            first_issue: None,
        };
        let mut expected_seq = 0i64;
        let mut expected_prev = String::new();
        let mut seen_chained = false;

        for row in rows {
            let (id, seq, prev_hash, hash, event) =
                row.map_err(|e| format!("读取审计链失败：{e}"))?;
            if hash.is_empty() {
                report.legacy += 1;
                if seen_chained && report.first_issue.is_none() {
                    report.first_issue = Some(AuditChainIssue {
                        id,
                        seq,
                        kind: "legacy_in_chain".into(),
                        message: format!("审计链中间混入无哈希的旧行（id={id}）"),
                    });
                }
                continue;
            }
            seen_chained = true;
            let issue = if seq != expected_seq + 1 {
                Some(AuditChainIssue {
                    id,
                    seq,
                    kind: "sequence_gap".into(),
                    message: format!("序号断档：期待 {}，实际 {seq}", expected_seq + 1),
                })
            } else if prev_hash != expected_prev {
                Some(AuditChainIssue {
                    id,
                    seq,
                    kind: "broken_prev_hash".into(),
                    message: format!("前序哈希不衔接（id={id}）：疑似删行或重排"),
                })
            } else if chain_hash(seq, &prev_hash, &event, &event.at) != hash {
                Some(AuditChainIssue {
                    id,
                    seq,
                    kind: "hash_mismatch".into(),
                    message: format!("本条内容被改（id={id}）：重算哈希与存储不符"),
                })
            } else {
                None
            };
            match issue {
                Some(issue) if report.first_issue.is_none() => report.first_issue = Some(issue),
                None => {
                    expected_seq = seq;
                    expected_prev = hash;
                    report.checked += 1;
                }
                _ => {}
            }
        }

        // 链头比对放在最后：只删尾部时，其余各行自身都合法。
        if report.first_issue.is_none() && (expected_seq != head_seq || expected_prev != head_hash)
        {
            report.first_issue = Some(AuditChainIssue {
                id: 0,
                seq: head_seq,
                kind: "head_mismatch".into(),
                message: format!(
                    "链头与末条不符（链头 seq={head_seq}，末条 seq={expected_seq}）：疑似尾部删行"
                ),
            });
        }
        report.ok = report.first_issue.is_none();
        Ok(report)
    }
}

/// 旧库的 `audit_log` 没有链字段；逐列补齐，默认值空表示"迁移前的行"。
fn ensure_audit_chain_columns(conn: &Connection) -> Result<(), String> {
    let mut stmt = conn
        .prepare("PRAGMA table_info(audit_log)")
        .map_err(|e| format!("读取审计表结构失败：{e}"))?;
    let cols = stmt
        .query_map([], |r| r.get::<_, String>(1))
        .map_err(|e| format!("读取审计表结构失败：{e}"))?;
    let mut names = Vec::new();
    for col in cols {
        names.push(col.map_err(|e| format!("读取审计表结构失败：{e}"))?);
    }
    for (name, ddl) in [
        (
            "seq",
            "ALTER TABLE audit_log ADD COLUMN seq INTEGER NOT NULL DEFAULT 0",
        ),
        (
            "prev_hash",
            "ALTER TABLE audit_log ADD COLUMN prev_hash TEXT NOT NULL DEFAULT ''",
        ),
        (
            "hash",
            "ALTER TABLE audit_log ADD COLUMN hash TEXT NOT NULL DEFAULT ''",
        ),
    ] {
        if !names.iter().any(|n| n == name) {
            conn.execute(ddl, [])
                .map_err(|e| format!("升级审计字段 {name} 失败：{e}"))?;
        }
    }
    Ok(())
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

/// 一条按状态聚合的对账行。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSummary {
    pub status: String,
    pub count: i64,
    pub amount_cents: i64,
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
    pub idempotency_key: String,
    pub tx_code: String,
    pub amount_cents: i64,
    pub status: String,
    pub reference: Option<String>,
    pub at: String,
}

impl SqliteJournal {
    /// 交易要素落盘前加密：账号不该以明文躺在磁盘上。
    fn seal_elements(&self, request: &TxRequest) -> Result<String, String> {
        self.cipher.seal(
            &serde_json::to_string(&request.elements)
                .map_err(|e| format!("序列化交易要素失败：{e}"))?,
        )
    }

    /// 把落盘的密文还原为请求要素；已有 `enqueue`/`pending` 共用同一格式。
    fn unseal_elements(
        &self,
        sealed: &str,
    ) -> Result<std::collections::BTreeMap<String, String>, String> {
        let plain = self.cipher.open(sealed)?;
        serde_json::from_str(&plain).map_err(|e| format!("反序列化交易要素失败：{e}"))
    }
}

impl TxJournal for SqliteJournal {
    fn record_intent(&self, request: &TxRequest) -> Result<(), String> {
        let elements = self.seal_elements(request)?;
        self.lock()
            .execute(
                "INSERT OR REPLACE INTO tx_intent
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
            .map_err(|e| format!("写前意图落盘失败：{e}"))
    }

    fn settle(&self, request: &TxRequest, result: &TxResult) -> Result<(), String> {
        let elements = if result.status == TxStatus::Pending {
            Some(self.seal_elements(request)?)
        } else {
            None
        };
        let mut conn = self.lock();
        let tx = conn
            .transaction()
            .map_err(|e| format!("结论落盘事务失败：{e}"))?;
        tx.execute(
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
        .map_err(|e| format!("写审计日志失败：{e}"))?;
        tx.execute(
            "DELETE FROM tx_intent WHERE idempotency_key = ?1",
            params![request.idempotency_key],
        )
        .map_err(|e| format!("清除写前意图失败：{e}"))?;
        if let Some(elements) = elements {
            tx.execute(
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
            .map_err(|e| format!("挂起请求入队失败：{e}"))?;
        } else {
            tx.execute(
                "DELETE FROM tx_queue WHERE idempotency_key = ?1",
                params![request.idempotency_key],
            )
            .map_err(|e| format!("挂起请求出队失败：{e}"))?;
        }
        tx.commit()
            .map_err(|e| format!("提交结论落盘事务失败：{e}"))
    }

    fn in_flight_intents(&self) -> Result<Vec<TxRequest>, String> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT idempotency_key, tx_code, amount_cents, elements, offline_capable
                 FROM tx_intent ORDER BY created_at, rowid",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, String>(3)?,
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
                elements: self.unseal_elements(&elements)?,
                offline_capable: offline != 0,
            });
        }
        Ok(out)
    }

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
        let elements = self.seal_elements(request)?;
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
            let (key, tx_code, amount_cents, stored, offline) = row.map_err(|e| e.to_string())?;
            // 解密失败必须报错，不能默认成空要素——那样会把交易变成另一笔交易
            let plain = self.cipher.open(&stored)?;
            let elements = serde_json::from_str(&plain)
                .map_err(|e| format!("交易要素解析失败（{key}）：{e}"))?;
            out.push(TxRequest {
                idempotency_key: key,
                tx_code,
                amount_cents,
                elements,
                offline_capable: offline != 0,
            });
        }
        Ok(out)
    }
}

/// 本条哈希 = SHA256(域分隔 || seq || prev_hash || 六个事件字段)。
/// 每字段带 8 字节长度前缀，避免 `("ab","c")` 与 `("a","bc")` 拼出同一串。
fn chain_hash(seq: i64, prev_hash: &str, event: &AuditEvent, at: &str) -> String {
    let seq_bytes = seq.to_be_bytes();
    let mut buf = Vec::new();
    buf.extend_from_slice(CHAIN_DOMAIN);
    for field in [
        &seq_bytes[..],
        prev_hash.as_bytes(),
        event.actor.as_bytes(),
        event.action.as_bytes(),
        event.target.as_bytes(),
        event.result.as_bytes(),
        event.detail.as_bytes(),
        at.as_bytes(),
    ] {
        buf.extend_from_slice(&(field.len() as u64).to_be_bytes());
        buf.extend_from_slice(field);
    }
    hex::encode(ring::digest::digest(&ring::digest::SHA256, &buf).as_ref())
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

    // S-08：崩溃窗口内只有意图，没有结论；重启必须能读出它并转异常。
    #[test]
    fn write_ahead_intent_survives_until_settled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        let j = SqliteJournal::open(&path).unwrap();
        let r = req("k-crash").offline(true);

        j.record_intent(&r).unwrap();
        drop(j);
        let j = SqliteJournal::open(&path).unwrap();
        let in_flight = j.in_flight_intents().unwrap();
        assert_eq!(in_flight.len(), 1);
        assert_eq!(in_flight[0].idempotency_key, "k-crash");
        assert_eq!(in_flight[0].amount_cents, 10_000);

        j.settle(&r, &committed()).unwrap();
        assert!(j.in_flight_intents().unwrap().is_empty());
        assert_eq!(j.log_count().unwrap(), 1, "结论只写一条，不与 intents 重复");
    }

    // Pending 时 settle 在同一事务里写结论并入队，意图不再遗留。
    #[test]
    fn settle_pending_moves_intent_to_queue_in_one_transaction() {
        let j = SqliteJournal::in_memory().unwrap();
        let r = req("k-pending").offline(true);
        j.record_intent(&r).unwrap();

        j.settle(&r, &pending()).unwrap();

        assert!(j.in_flight_intents().unwrap().is_empty());
        let keys: Vec<String> = j
            .pending()
            .unwrap()
            .into_iter()
            .map(|r| r.idempotency_key)
            .collect();
        assert_eq!(keys, vec!["k-pending".to_string()]);
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
    fn summary_aggregates_by_status_and_amount() {
        let dir = tempfile::tempdir().unwrap();
        let j = SqliteJournal::open(dir.path().join("tx.db")).unwrap();

        j.record(&req("a"), &committed()).unwrap();
        j.record(&req("b"), &committed()).unwrap();
        j.record(
            &req("c"),
            &TxResult {
                status: TxStatus::Rejected,
                reference: None,
                message: "余额不足".into(),
                replayable: false,
            },
        )
        .unwrap();

        let rows = j.summary("").unwrap();
        let committed = rows.iter().find(|r| r.status == "committed").unwrap();
        assert_eq!(committed.count, 2);
        assert_eq!(committed.amount_cents, 2 * req("a").amount_cents);

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
            r.elements.insert("account".into(), account.into());
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
        assert_eq!(j.pending().unwrap()[0].elements["account"], account);
    }

    #[test]
    fn corrupt_ciphertext_fails_loudly_instead_of_dropping_elements() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tx.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            let mut r = req("k-bad").offline(true);
            r.elements.insert("account".into(), "6222".into());
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

    fn chain_rows(j: &SqliteJournal) -> Vec<(i64, String, String)> {
        let conn = j.conn.lock().unwrap_or_else(|e| e.into_inner());
        let mut stmt = conn
            .prepare("SELECT seq, prev_hash, hash FROM audit_log ORDER BY id")
            .unwrap();
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    }

    fn with_three_events() -> SqliteJournal {
        let j = SqliteJournal::in_memory().unwrap();
        for target in ["m1", "m2", "m3"] {
            j.audit(&audit_event("menu_access", target)).unwrap();
        }
        j
    }

    #[test]
    fn audit_chain_links_and_survives_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("chain.db");
        {
            let j = SqliteJournal::open(&path).unwrap();
            for target in ["m1", "m2"] {
                j.audit(&audit_event("menu_access", target)).unwrap();
            }
        }
        let j = SqliteJournal::open(&path).unwrap();
        j.audit(&audit_event("submit_tx", "t3")).unwrap();

        let rows = chain_rows(&j);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].0, rows[1].0, rows[2].0),
            (1, 2, 3),
            "序号必须单调连续"
        );
        assert_eq!(rows[0].1, "", "创世行的 prev_hash 为空");
        assert_eq!(rows[1].1, rows[0].2, "第二条指向第一条的哈希");
        assert_eq!(rows[2].1, rows[1].2, "重启后仍接在原链尾");
        assert_ne!(rows[0].2, rows[1].2, "内容不同则哈希不同");

        let report = j.audit_verify().unwrap();
        assert!(report.ok, "{:?}", report.first_issue);
        assert_eq!(report.checked, 3);
        assert_eq!(report.legacy, 0);
    }

    #[test]
    fn audit_verify_detects_field_tampering() {
        let j = with_three_events();
        j.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("UPDATE audit_log SET detail = 'tampered' WHERE id = 2", [])
            .unwrap();

        let report = j.audit_verify().unwrap();
        assert!(!report.ok, "改字段必须校验失败");
        let issue = report.first_issue.unwrap();
        assert_eq!(issue.id, 2, "必须报出被改的行号");
        assert_eq!(issue.kind, "hash_mismatch");
        assert!(issue.message.contains("内容被改"), "{}", issue.message);
    }

    #[test]
    fn audit_verify_detects_deletions_and_reorder() {
        // 中间删行：序号断档
        let j = with_three_events();
        j.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DELETE FROM audit_log WHERE id = 2", [])
            .unwrap();
        let issue = j.audit_verify().unwrap().first_issue.unwrap();
        assert_eq!(issue.kind, "sequence_gap");

        // 尾部删行：剩下各行自身合法，只能靠链头发现
        let j = with_three_events();
        j.conn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .execute("DELETE FROM audit_log WHERE id = 3", [])
            .unwrap();
        let issue = j.audit_verify().unwrap().first_issue.unwrap();
        assert_eq!(issue.kind, "head_mismatch");
        assert_eq!(issue.seq, 3, "链头仍指向被删的第三行");

        // 重排：交换前两行的 id，按 id 扫描时序号倒退
        let j = with_three_events();
        let conn = j.conn.lock().unwrap_or_else(|e| e.into_inner());
        conn.execute("UPDATE audit_log SET id = 99 WHERE id = 1", [])
            .unwrap();
        conn.execute("UPDATE audit_log SET id = 1 WHERE id = 2", [])
            .unwrap();
        conn.execute("UPDATE audit_log SET id = 2 WHERE id = 99", [])
            .unwrap();
        drop(conn);
        let issue = j.audit_verify().unwrap().first_issue.unwrap();
        assert_eq!(issue.kind, "sequence_gap");
    }

    #[test]
    fn old_audit_db_migrates_and_new_rows_are_chained() {
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
                 INSERT INTO audit_log (actor, action, target, result, detail, at) VALUES
                    ('u1', 'login', 'u1', 'ok', '', '00000000000000000001'),
                    ('u1', 'menu_access', 'm1', 'ok', '', '00000000000000000002');",
            )
            .unwrap();
        }

        let j = SqliteJournal::open(&path).unwrap();
        let report = j.audit_verify().unwrap();
        assert!(report.ok);
        assert_eq!(report.legacy, 2, "旧行无哈希，必须如实计入 legacy");
        assert_eq!(report.checked, 0);
        j.audit(&audit_event("submit_tx", "t1")).unwrap();

        let j = SqliteJournal::open(&path).unwrap();
        assert_eq!(j.audit_count().unwrap(), 3, "迁移不能丢旧行");
        let rows = chain_rows(&j);
        assert_eq!(rows[0].0, 0, "旧行仍是 seq=0");
        assert_eq!(rows[2].0, 1, "新链从 1 开始");
        let report = j.audit_verify().unwrap();
        assert!(report.ok, "{:?}", report.first_issue);
        assert_eq!(report.legacy, 2);
        assert_eq!(report.checked, 1, "只有新行参与哈希校验");
        assert_eq!(j.audit_tail(1).unwrap()[0].target, "t1", "旧接口保持可用");
    }
}

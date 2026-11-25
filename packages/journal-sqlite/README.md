# packages/journal-sqlite · 交易账本

`TxJournal` 端口的 SQLite 实现：待重放队列 + 交易审计日志。

## 为什么不放 core

`tellerkit-core` 必须能脱离磁盘做纯逻辑测试（宪法第 2 条）。
数据库依赖留在这一层，core 只定义端口。

## 表

| 表 | 用途 | 保留 |
|---|---|---|
| `tx_queue` | 挂起待重放的请求，主键是幂等键 | 出队即删 |
| `tx_log` | 每次状态迁移追加一条 | 只增，P2 接上报后再定归档 |
| `audit_log` | 登录/菜单/交易/打印/重放等操作留痕 | 只增，**不提供删除接口** |

## 约束

- 只有重放**成功**才出队；不可达保留（不丢单）
- 记账失败不改变交易结论，但会通过 `take_journal_error()` 上报
- 账本打不开时主壳降级为内存模式并告警，不阻断启动

## 测试

```bash
cargo test -p tellerkit-journal-sqlite   # 或 scripts/remote-build.sh 同样命令
```

## 加密

`tx_queue.elements`（账号、户名、摘要）用 AES-256-GCM 加密后入库，格式
`v1:<hex nonce>:<hex ciphertext>`。密钥在 `<appdata>/ledger.key`（Unix 0600）。

**局限**：密钥与库同目录，能执行代码者可一并拿走。它防的是"只拷走数据库"这类泄露，
不是"账本安全了"。详见 `changes/010-ledger-encryption.md`。

旧库无需迁移：没有 `v1:` 前缀的记录按明文读取。

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

## 约束

- 只有重放**成功**才出队；不可达保留（不丢单）
- 记账失败不改变交易结论，但会通过 `take_journal_error()` 上报
- 账本打不开时主壳降级为内存模式并告警，不阻断启动

## 测试

```bash
cargo test -p tellerkit-journal-sqlite   # 或 scripts/remote-build.sh 同样命令
```

# packages/journal-sqlite/AGENTS.md

## 职责

`TxJournal` 端口的 SQLite 适配器。只做存取，不做业务判断。

## 允许依赖

`tellerkit-core`（端口定义）· `rusqlite`（bundled）· `serde_json`

## 禁止

- 在 SQL 层做状态机判断（结论由 `TxEngine` 给）
- 直接删除未完成的 `tx_queue` 记录（会丢单）
- 让 `tellerkit-core` 反向依赖本包
- 存储完整卡号、PIN 块或客户敏感明文（交易要素由业务层决定是否含敏感字段，
  当前 `elements` 只放账号掩码与备注）

## 契约

`TxJournal` trait 定义在 `packages/core/src/tx.rs`；改动它等于改接口，需同步核心与主壳。

## 验收

```bash
cargo test -p tellerkit-journal-sqlite
```

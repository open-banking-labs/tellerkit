# 交易落盘与重放 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 定义 TxJournal 端口并接入引擎 | S-01, S-03, S-05 | `core` | `packages/core/src/tx.rs` | `cargo test -p tellerkit-core tx::` |
| T-02 | 实现 SQLite 账本 | S-01, S-02, S-05 | `core` | `packages/journal-sqlite/**` | `cargo test -p tellerkit-journal-sqlite` |
| T-03 | 主壳装配与降级告警 | S-02, S-04 | `shell` | `apps/shell/src-tauri/**` | `cargo build -p tellerkit` |
| T-04 | 界面重放入口 | S-02 | `ui` | `apps/shell/src/**` | `npm run build` |

## 完成定义

- 四个任务的验收命令 PASS
- `npm run gate` 绿
- 端到端用例覆盖"重启后重放"（不是只测单个方法）

# 自动重放接线 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 单笔重放与一轮调度 | S-01, S-02, S-03, S-04 | `core` | `packages/core/src/{tx.rs,replay.rs}` | `cargo test -p tellerkit-core replay::` |
| T-02 | 命令层接入调度器 | S-01 | `shell` | `apps/shell/src-tauri/**` | `cargo build -p tellerkit` |

## 完成定义

- 两个任务的验收命令 PASS
- `npm run gate` 绿

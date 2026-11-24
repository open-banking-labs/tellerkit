# 自动重放 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现重放调度与退避 | S-01, S-02, S-03, S-04, S-05, S-06 | `core` | `packages/core/src/replay.rs` | `cargo test -p tellerkit-core replay::` |

## 完成定义

- 验收命令 PASS
- seam 签名不得改动

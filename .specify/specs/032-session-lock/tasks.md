# 离岗锁屏 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现会话锁定状态机 | S-01, S-02, S-03, S-04, S-05, S-06 | `core` | `packages/core/src/session.rs` | `cargo test -p tellerkit-core session::` |

## 完成定义

- 验收命令 PASS
- seam 签名不得改动

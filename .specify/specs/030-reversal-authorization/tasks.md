# 冲正与授权 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现异常标记与冲正申请 | S-01, S-02, S-03, S-04, S-05, S-06 | `core` | `packages/core/src/reversal.rs` | `cargo test -p tellerkit-core reversal::` |

## 完成定义

- 验收命令 PASS
- seam 里的签名与文档注释不得改动（改了要重新集成）
- 至少 6 个用例，覆盖六个场景

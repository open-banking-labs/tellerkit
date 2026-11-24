# 更新策略 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现更新决策与灰度散列 | S-01, S-02, S-03, S-04, S-05, S-06 | `core` | `packages/core/src/update.rs` | `cargo test -p tellerkit-core update::` |

## 完成定义

- 验收命令 PASS
- seam 签名不得改动
- `decide` 必须是纯函数：不读时钟、不读环境变量

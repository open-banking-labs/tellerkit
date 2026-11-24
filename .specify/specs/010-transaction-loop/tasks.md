# 交易闭环 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现交易模型与状态机 | S-01, S-03, S-04, S-05 | `core` | `packages/core/src/tx.rs`, `packages/core/src/lib.rs` | `cargo test -p tellerkit-core tx::` |
| T-02 | 补充幂等去重的对抗用例 | S-02 | `core` | `packages/core/src/tx.rs` | `cargo test -p tellerkit-core tx::` |
| T-03 | 修正瞬时失败缓存与幂等键复用 | S-03, S-06, S-07 | `core` | `packages/core/src/tx.rs` | `cargo test -p tellerkit-core tx::` |

## 完成定义

- 两个任务的验收命令在干净树上 PASS
- `npm run gate` 绿
- 幂等用例断言主机调用次数，而不只是返回值

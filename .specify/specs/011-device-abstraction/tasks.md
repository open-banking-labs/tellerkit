# 设备抽象 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现四个设备 trait 与错误模型 | S-01, S-02 | `core` | `packages/core/src/device.rs`, `packages/core/src/lib.rs` | `cargo test -p tellerkit-core device::` |
| T-02 | 实现设备就绪校验与 mock 适配器 | S-03 | `core` | `packages/core/src/device.rs` | `cargo test -p tellerkit-core device::` |
| T-03 | 敏感数据脱敏与换实现用例 | S-04, S-05 | `core` | `packages/core/src/device.rs` | `cargo test -p tellerkit-core device::` |
| T-04 | 补充设备错误的对抗用例 | S-02, S-03 | `core` | `packages/core/src/device.rs` | `cargo test -p tellerkit-core device::` |

## 完成定义

- 四个任务的验收命令在干净树上 PASS
- `npm run gate` 绿
- 脱敏用例断言 Debug 输出字符串，而不是只检查字段

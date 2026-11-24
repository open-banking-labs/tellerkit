# 业务包加载 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现签名容器与清单校验 | S-01, S-02, S-05 | `core` | `packages/core/src/pkg.rs`, `packages/core/src/lib.rs` | `cargo test -p tellerkit-core pkg::` |
| T-02 | 实现版本兼容与依赖拓扑 | S-03, S-04 | `core` | `packages/core/src/pkg.rs` | `cargo test -p tellerkit-core pkg::` |
| T-03 | 菜单合并与权限闭环用例 | S-05, S-06 | `core` | `packages/core/src/pkg.rs` | `cargo test -p tellerkit-core pkg::` |
| T-04 | 打包工具与示例包对齐 | S-01 | `core` | `tools/mtree-tool/src/**`, `packages/biz-demo/**` | `cargo run -q -p mtree-tool -- pkg-verify /tmp/biz-demo.bizpkg` |

## 完成定义

- 四个任务的验收命令 PASS
- `npm run gate` 绿
- `packages/biz-demo` 能被真实打包并验签通过

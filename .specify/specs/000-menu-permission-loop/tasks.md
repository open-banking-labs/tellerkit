# 菜单权限闭环 · 任务

> 机器可读格式。结构由 `tools/check-tasks.mjs` 校验，验收由 `tools/run-task.mjs` 执行。
> 本能力已实现，此表是补记：给后续能力立格式，同时作为执行器的回归样本。

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 定义菜单契约与结构校验 | S-02 | `core` | `packages/core/src/model.rs` | `cargo test -p tellerkit-core model::` |
| T-02 | 实现权限递归剪枝 | S-02, S-03 | `core` | `packages/core/src/filter.rs` | `cargo test -p tellerkit-core filter::` |
| T-03 | 实现 mtree 编解码与验签 | S-04 | `core` | `packages/core/src/pack.rs` | `cargo test -p tellerkit-core pack::` |
| T-04 | 实现本地包仓库与原子替换 | S-01, S-04 | `core` | `packages/core/src/store.rs` | `cargo test -p tellerkit-core store::` |
| T-05 | 实现启动回退与版本探测 | S-01, S-06 | `core` | `packages/core/src/sync.rs` | `cargo test -p tellerkit-core sync::` |
| T-06 | 装配 AppState 与命令层 | S-01, S-05 | `shell` | `apps/shell/src-tauri/src/**` | `cargo build -p tellerkit` |
| T-07 | 登录登出与快照生命周期 | S-05 | `shell` | `apps/shell/src-tauri/src/commands.rs` | `cargo build -p tellerkit` |
| T-08 | 菜单渲染与键盘漫游 | S-02, S-03 | `ui` | `apps/shell/src/**` | `npm run build` |
| T-09 | 造包 CLI 与 fixtures | S-04 | `core` | `tools/mtree-tool/src/**` | `bash scripts/verify-p0.sh` |
| T-10 | 安全边界文档与验收脚本 | S-04 | `docs` | `docs/**`, `scripts/**`, `README.md` | `npm run gate` |

## 完成定义

- 每个场景至少有 `spec.md` 中声明的验收路径（测试或人工步骤）
- `npm run gate` 绿
- 改动未越出「允许改动」范围

## 备注

T-10 的验收命令是 `npm run gate`：它同时验证了文档表述、门禁脚本与契约一致性，
是本仓库唯一"用自己的门禁验收自己文档"的任务。

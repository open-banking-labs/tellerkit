# 菜单权限闭环 · 任务

> 已完成。补记用于给后续能力立格式：每项都引用场景与层次。

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 定义 MenuNode/MenuTree 与结构校验 | S-02 | `core` | `crates/tellerkit-core/src/model.rs` | `cargo test -p tellerkit-core` |
| T-02 | 实现 filter_visible 递归剪枝 | S-02, S-03 | `core` | `crates/tellerkit-core/src/filter.rs` | `cargo test -p tellerkit-core` |
| T-03 | `.mtree` 编解码 + Ed25519 签名 | S-04 | `core` | `crates/tellerkit-core/src/pack.rs` | `cargo test -p tellerkit-core` |
| T-04 | 本地包仓库：原子替换、保留 N-1、坏包拒绝 | S-01, S-04 | `core` | `crates/tellerkit-core/src/store.rs` | `cargo test -p tellerkit-core` |
| T-05 | 启动回退与版本探测编排 | S-01, S-06 | `core` | `crates/tellerkit-core/src/sync.rs` | `cargo test -p tellerkit-core` |
| T-06 | AppState 与命令层（薄封装） | S-01, S-05 | `shell` | `src-tauri/src/*` | `cargo build -p tellerkit` |
| T-07 | 登录/登出与快照生命周期 | S-05 | `shell` | `src-tauri/src/commands.rs` | 人工验收第 5 条 |
| T-08 | 菜单渲染 + 键盘漫游 + 日志 | S-02, S-03 | `ui` | `src/**` | `npm run build` |
| T-09 | 造包 CLI 与 fixtures | S-04 | `core` | `crates/mtree-tool/**` | `bash scripts/verify-p0.sh` |
| T-10 | 安全边界文档与验收脚本 | 全部 | `docs` | `docs/**`, `scripts/**` | 人工复核 |

## 完成定义

- 六个场景在 `crates/tellerkit-core` 有对应用例，或有人工验收步骤
- `npm run gate` 绿
- 未触碰 `contracts/`（补记阶段建立契约时，另行走提案）

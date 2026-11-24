# 业务包页面加载 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 容器嵌入前端资源与打包 | S-04 | `core` | `packages/core/src/pkg.rs`, `tools/mtree-tool/src/**` | `cargo test -p tellerkit-core pkg::` |
| T-02 | 启动扫描、合并与冲突提示 | S-01, S-02, S-03 | `shell` | `apps/shell/src-tauri/**` | `cargo build -p tellerkit` |
| T-03 | 模块下发命令与前端挂载 | S-04 | `ui` | `apps/shell/src/**` | `npm run build` |
| T-04 | 安全边界与能力说明更新 | S-05 | `docs` | `docs/**` | `npm run gate` |

## 完成定义

- 四个任务的验收命令 PASS
- `npm run gate` 绿
- 安全边界文档明确写出"包模块没有进程级沙箱"

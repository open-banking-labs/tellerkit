# P2 收尾 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 审计表与只增接口 | S-01, S-02 | `core` | `packages/journal-sqlite/src/lib.rs`, `packages/journal-sqlite/Cargo.toml` | `cargo test -p tellerkit-journal-sqlite` |
| T-02 | 关键行为埋点与审计命令 | S-01 | `shell` | `apps/shell/src-tauri/**` | `cargo build -p tellerkit` |
| T-03 | 敏感字段剪贴板管控 | S-03 | `ui` | `apps/shell/src/**` | `npm run build` |
| T-04 | 外设型号白名单 | S-04, S-05 | `core` | `packages/core/src/device.rs` | `cargo test -p tellerkit-core device::` |
| T-05 | 业务包脚手架 | S-06, S-07 | `core` | `tools/mtree-tool/src/**` | `cargo run -q -p mtree-tool -- pkg-init /tmp/tk-init` |
| T-06 | 轨迹门禁 | S-08 | `tools` | `tools/check-trajectory.mjs`, `package.json` | `node tools/check-trajectory.mjs` |

## 完成定义

- 六个任务的验收命令 PASS
- `npm run gate` 绿（含新增的轨迹门禁）
- 审计埋点覆盖：登录、登出、菜单访问、交易提交、凭证打印、重放、业务包加载

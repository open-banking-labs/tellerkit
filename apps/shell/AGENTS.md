# apps/shell/AGENTS.md

## 职责

柜面客户端主壳的装配层。把 `packages/core` 的纯逻辑和 `packages/biz-*` 的业务包
组装成一个可运行终端；持有 `AppState`（唯一真相源）。

## 允许依赖

`packages/core`（Rust crate `tellerkit-core`）· `tauri` · `vue` · `@tauri-apps/api`

## 禁止

- 在 `src-tauri/src/commands.rs` 里写业务决策（决策属于 `tellerkit-core::sync` / `filter`）
- 在 `src/` 里判断权限（前端只渲染 Rust 给的结果）
- 让 `packages/*` 反向依赖本目录（依赖方向只能 apps → packages）
- 在 `apps/` 下新增第二个应用（`check-boundaries` 会拦）

## 子目录

| 目录 | 规则 |
|---|---|
| `src/` | 见 `src/AGENTS.md`，只渲染 |
| `src-tauri/` | 见 `src-tauri/AGENTS.md`，只装配 |

## 验收

```bash
npm run gate                       # 三道门禁
npm run build                      # vue-tsc + vite（apps/shell 内）
cargo build -p tellerkit --features custom-protocol
```

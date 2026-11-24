# apps/shell · 柜面客户端主壳

一次构建产物：`TellerKit.app`（macOS）/ `TellerKit.exe`（Windows）。

```
apps/shell/
├── src/            Vue 3 前端，只渲染
├── src-tauri/      Rust 主壳：AppState、命令、外设、审计（P1+）
├── index.html
└── dist/           构建产物（gitignored）
```

## 职责

把 `packages/*` 的能力组装成一个可运行的柜面终端：

- 持有 `AppState`（唯一真相源）
- 提供 IPC 命令（薄封装）
- 加载业务包、合并菜单、按权限渲染

## 不负责

- 业务逻辑（在 `packages/core`）
- 具体业务功能（在 `packages/biz-*`）
- 契约定义（在 `contracts/`）

## 命令

```bash
npm run dev --workspace @tellerkit/shell    # 浏览器预览
npm run tauri:dev --workspace @tellerkit/shell
```

在仓库根直接跑 `npm run tauri:dev` 等价。

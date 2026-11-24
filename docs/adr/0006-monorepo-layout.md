# ADR-0006：一步到位改为 monorepo 布局

- 状态：已接受
- 日期：2026-11-24
- 决策者：项目负责人

## 背景

P0 阶段仓库是单应用布局：`src/`、`src-tauri/`、`crates/`。

R12–R14 确立的目标形态是 `apps/shell` + `packages/*` + `contracts/` + `.specify/` + `tools/`，
并且明确"团队以 AI 为主力"。继续用单应用布局会带来两个具体问题：

1. **门禁作用域写死。** `check-boundaries` 直接把 `src/`、`src-tauri/` 写进代码，
   新增一个业务包或子应用时门禁不会自动覆盖，绿色的可信度会静默下降。
2. **业务包没有位置。** `packages/` 不存在，`.bizpkg` 的形状只能靠文档描述，
   生成器和门禁没有可对齐的实物。

## 决策

现在就迁到目标布局，**不等第一个业务包出现**：

```
apps/shell/       柜面客户端主壳（前端 + src-tauri）
packages/core/    纯 Rust 核心引擎（原 crates/tellerkit-core）
packages/biz-*/   业务包（biz-demo 为骨架）
tools/            门禁 + mtree-tool
contracts/ .specify/ changes/ docs/ .agents/
```

同时把门禁改为**按目录发现**（`tools/paths.mjs`），不再写死路径。

## 理由

- 迁移成本现在最低：3 个 git mv + 4 处 path 依赖，一小时量级。
- 等 P2 再迁，业务包和加载器会和旧布局缠绕，改动面翻倍。
- 门禁自动化后，新增 `/apps` 或 `/packages` 单位会**自动**进入扫描范围，
  "绿"才有意义。

## 代价

| 代价 | 缓解 |
|---|---|
| Tauri/Vite/Cargo 三处路径同步改 | 一次性完成并跑通构建 + 门禁 |
| npm workspaces 引入，`npm install` 层级变化 | 根目录单 lockfile，脚本统一用 `-w` |
| 只有 1 个 app 时 `apps/` 像"目录表演" | 接受：这是为门禁作用域和业务包位置付的学费，不是给外人看的 |

## 被否决的方案

- **维持单应用，等 P2 再迁**：门禁作用域无法自动扩展，业务包无处安放。
- **现在就拆 `packages/protocol`（生成的 TS 类型）**：W2 才需要，现在建是空目录。
- **上 pnpm/Turborepo**：当前只有 1 个 app、0 个依赖 workspace 的包，
  引入工具链的复杂度大于收益。触发条件：workspace 依赖 ≥3 个或构建时间 >2 分钟。

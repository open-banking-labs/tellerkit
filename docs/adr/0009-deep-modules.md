# ADR-0009：把「深模块 / 窄接口」列为高阶要求并全量收口

- 状态：已接受
- 日期：2026-11-26
- 决策者：项目负责人（批准）、AI（主导落地）

## 背景

按 [deslop-please](https://github.com/deterministic-machines/deslop-please)（《A Philosophy of
Software Design》的 "deep modules, narrow interfaces"）对 `docs/p1-11-progress` 做只读审计：

- **module-cycle：0 条**。生产依赖图是严格分层 DAG。
- **field-leak 普遍**：`MenuNode` / `MenuTree` / `AppState` / `MenuPack` / `LoadedMenu` /
  `MenuVersionProbe` / `Version` 等字段全 `pub`，被其他模块直接读写；`apps/shell` 里
  `app.rs` 有 44 处 `self.state.*`，`commands.rs` 直接摸 `st.mock/st.store`。
- **shallow-module 明显**：`ShellApp` 一个 struct 19 个 `pub fn` 承担 11 类用例；`commands.rs`
  23 个命令薄转发；`client.ts` 19 + 5 个方法，`App.vue` 自己拼多步流程。
- **零内部可见性边界**：core 与 shell 全树 `pub(crate)` / 私有 `mod` 出现 0 次。

现有四层（L1 界面 / L2 IPC / L3 用例 / L4 领域）只约束 import 方向，不约束接口宽度；
`check-boundaries` 等门禁全是文本/正则规则，无法表达"模块藏了多少"。

## 决策

1. `docs/constitution.md` 新增第 11 条「深模块 / 窄接口」，判定以 deslop-please 四类反模式为准。
2. 结构体字段默认私有（含 DTO）：跨模块走构造函数/访问器；跨语言形状由 ts-rs 与
   `contracts/fields.json` 继续兜底，不因私有化改变。
3. `packages/core` 收敛为四个能力域门面：`MenuCatalog` / `TxService` / `RemoteGateway` /
   `PackageLoader`；`session` / `device` 保持独立。**不新增 crate、不新增层**。
4. 门禁：本机 `npm run gate` 增加第 7 道 `tools/check-module-depth.mjs`（先观测后阻断）；
   构建机安装并运行 `deslop-please`；`check-contracts.mjs` 的字段提取放宽到私有字段。
5. 修复纪律：从第一性原理整体修，禁止 `--update-allow` 批量静音；豁免逐条人工批准。

## 理由

- 项目以「人 + AI 共同维护」为前提。宽接口会让 AI 倾向于机械拼接、复制字段访问；
  窄接口是让产出收敛的最低成本手段。
- 现有门禁的"绿"只证明 import 方向和个别 token 合规，不证明模块化；需要一条能表达
  接口宽度的检查。
- 当前规模仍可整体收口（core 约 1.9 万行），越晚迁移面越大。

## 代价

| 代价 | 缓解 |
|---|---|
| 字段私有化波及 core 与主壳大部分文件 | 按能力域拆任务，每域单独跑 gate + 构建机 `cargo test` |
| `check-contracts.mjs` 必须同步改字段提取 | 先改门禁并验证基线，再动代码 |
| ts-rs 对私有字段的导出行为需构建机确认 | 若不可行，回退为「DTO 保留字段 + allow 逐条登记」并在本 ADR 记录 |
| 与 `docs/p1-11-progress` 并行 | 统一在 `refactor/deep-modules` 分支，PR 前 rebase |

## 被否决的方案

- **只观测不改**：门禁会长期红，等于没有规则；且 field-leak 会继续扩散。
- **保留 DTO 的 `pub` 字段、只收口领域状态**：项目负责人明确选择"专业做法、忽略风险"，
  DTO 也纳入收口；跨语言形状由契约门禁保证，私有化不改变契约。
- **每个命令一个 use case struct**：只把函数换个形状，无第二个实现，违反减法优先（architecture.md §6）。
- **拆成新的 `packages/protocol` 等 crate**：ADR-0006 已否决，当前没有第二个消费者。
- **本机安装 deslop-please 跑 cargo**：违反编译纪律；改为构建机运行。

## 豁免记录

- `shallow-module tellerkit_core::menu`：`menu/` 是菜单域聚合门面，对外 re-export 的 13 个类型/入口是命令层与启动路径实际要用的；进一步收窄需要把 dev 操作（corrupt/reset）移进 `MenuStore`，属后续重构。已登记 `.deslop-please.allow`，新增问题仍会失败。

## 验收

- `npm run gate` 绿（含第 7 道 `check-module-depth`）。
- 构建机 `cargo test --workspace` + `deslop-please` 通过或仅剩逐条批准的豁免。
- `packages/core/src` 与 `apps/shell/src-tauri/src` 的 `pub(crate)` / 私有 `mod` 计数 > 0；
  `lib.rs` 导出符号数显著下降；`AppState` 字段不再被 `app.rs` 直接读写。
- 前端 `generated/` 与 `contracts/fields.json` 三方一致（`npm run bindings` 后 gate 绿）。

# 017 把「深模块 / 窄接口」列为项目高阶要求

- 状态：**已批准**（项目负责人 2026-11-26 口头批准并授权 AI 主导落地）
- 关联：[deslop-please](https://github.com/deterministic-machines/deslop-please)、[ADR-0009](../docs/adr/0009-deep-modules.md)、[docs/architecture.md](../docs/architecture.md)

## 现状

按 [deslop-please](https://github.com/deterministic-machines/deslop-please) 的四类反模式（`module-cycle` / `tangle` / `field-leak` / `shallow-module`）对当前分支做只读审计，结论是：**有技术分层，没有信息隐藏意义上的模块化**。

硬证据（当前分支 `docs/p1-11-progress`，2026-11-26）：

| 现象 | 证据 |
|---|---|
| 零内部可见性边界 | `packages/core/src` 与 `apps/shell/src-tauri/src` 全树 `pub(crate)` / `pub(super)` / 私有 `mod` 出现 **0 次**；`packages/core/src/lib.rs:6-21` 16 个 `pub mod` 全平铺，导出 81 个符号 |
| field-leak | `model.rs:53` `MenuNode` 11 个字段全 `pub`；`filter.rs:65-136` 跨模块遍历 `tree.nodes` 并读 `node.code/parent_code/status/sort/actions`；`pkg.rs:402` 跨模块写 `node.source`；`mock.rs:41,112` 写 `actions/status`；`store.rs:149` 读 `pack.menu_version`；`sync.rs:58-62` 跨模块构造 `LoadedMenu`；`update.rs:125` 读 `Version.major` |
| 主壳 field-leak | `apps/shell/src-tauri/src/lib.rs:41-83` `AppState` 18 个 `pub` 字段；`app.rs` 44 处 `self.state.*` 直接读写；`commands.rs:216-223` 直读 `st.mock/st.store`，`dev_corrupt_local` 在命令层直接 `std::fs::read/write` |
| shallow-module | `ShellApp` 一个 struct 承担 11 类用例、19 个 `pub fn`；`commands.rs` 23 个命令一一薄转发；`client.ts` `ShellClient` 19 方法 + `DevSwitches` 5 方法，`App.vue` 自己拼 6 步登录 / 3 步重置 |
| module-cycle | **0 条**（生产依赖图是严格分层 DAG），不夸大 |
| tangle | 无 import 级双向环，但全 `pub` 字段让各层互相直接读写内部；`ports.rs` 一个文件混放端口 trait、`MockRemote` 适配器（173 行）、`SystemClock` 与自由函数 |

已做对、可作样板的点：`ReplayScheduler`（`replay.rs:44-47`）、`MenuStore`（`store.rs:57`）字段私有；`filter.rs:60-61` 走 `snapshot.menu_set()/action_set()` 而非读 `PermSnapshot.menu_codes`。

## 为什么现在做

1. 项目以「人 + AI 共同维护」为前提，AI 最容易被宽接口引导去做机械拼接、复制字段访问；窄接口是让 AI 产出收敛的最低成本手段。
2. 现有门禁（`tools/*.mjs`）全是文本/正则规则，只约束 import 方向与个别 token，**没有一条检查接口宽度或字段可见性**——「绿色」不代表模块化。
3. 当前代码仍处于可整体收口的规模（core 约 1.9 万行），越晚做迁移面越大。

## 建议

### 1. 宪法新增第 11 条

```markdown
## 第 11 条 · 深模块 / 窄接口

模块化不是按文件或技术层切分，而是按信息隐藏切分：内部状态私有，对外只暴露少数接口，
接口面必须显著窄于实现面。判定以 deslop-please 的四类反模式为准：module-cycle、tangle、
field-leak、shallow-module。

- 跨模块不得直接读写对方结构体字段；DTO 同样字段私有，用构造函数/访问器，
  跨语言序列化形状由契约门禁（ts-rs + fields.json）保证，不因私有化而改变。
- 每个 crate 只 `pub` 能力域门面；内部模块用 `pub(crate)`，`lib.rs` 不得平铺导出全部内部实现。
- 单个对象对外 `pub fn` 超过 12 个必须评审：拆成多个深模块，或把调用方的组合逻辑收进模块。
- 修复必须从第一性原理整体修；禁止用 `--update-allow` 批量静音，豁免需逐条人工批准并登记理由。
- 检查：`tools/check-module-depth.mjs`（本机 `npm run gate` 第 7 道）+ 构建机 `deslop-please`。
```

### 2. AGENTS.md

全局禁令加第 10 条（不得跨模块直接读写字段、不得把内部模块全部 `pub`）；命令区加 `npm run depth` 与构建机 `deslop-please`；入口导航加「模块化 / 深模块」。

### 3. rust-style.md §3.3 修订

把「Tauri DTO 不加 `#[non_exhaustive]`」的理由改为「字段私有 + 构造函数；`#[non_exhaustive]` 仍不加，因为构造由 `new()` 承担，不再依赖结构体字面量」。字段可见性规则改为：**所有结构体字段默认私有，只有序列化契约需要时用构造函数暴露，不直接 `pub` 字段**。

### 4. 门禁

- 新增 `tools/check-module-depth.mjs`：统计 `pub mod` / `pub(crate)` / 跨模块字段访问 / 单文件 `pub fn` / 单 struct `pub` 字段，先观测、后阻断（阈值分两阶段）。
- `tools/check-contracts.mjs` 的字段提取从 `^pub\s+(\w+)\s*:` 放宽到私有字段（否则私有化会误报「缺少字段」）。
- 构建机（`scripts/remote-build.sh`）安装并运行 `deslop-please`；`.deslop-please.allow` 只允许登记经批准的单条豁免。

### 5. 四个能力域全量收口

| 门面 | 吸收 | 对外接口（目标） |
|---|---|---|
| `MenuCatalog` | `model` + `filter` + `pack` + `store` + `sync` 的菜单部分 | `load()` / `visible(snapshot)` / `install(bytes)` / `sync()` |
| `TxService` | `tx` + `replay` + `reversal` | `submit()` / `replay_pending()` / `reverse()` / `exceptions()` |
| `RemoteGateway` | `remote` + `banking` | `router.call()` + 端口 trait |
| `PackageLoader` | `pkg` + `update` | `load_all()` / `decide_update()` |

`session` / `device` 保持独立（本就是深模块样板）。**不新增 crate、不新增层**（符合 ADR-0006 与宪法第 7 条）；按能力域收口，不是「一个命令一个 struct」（不与 architecture.md §6 冲突）。

## 影响

- Rust 内部可见性与构造方式大改；**契约字段名与形状不变**（不改 `contracts/`，不违反宪法第 1 条）。
- `check-contracts.mjs` 必须同步放宽字段提取；`check-boundaries.mjs` 的 token 规则可能需要随模块路径调整。
- `commands.rs` 结构体字面量构造改为 `new()`；前端 `generated/` 不变。
- 构建机验证链路必须参与每次提交（本机不跑 cargo）。

## 风险

- 私有字段 + serde/ts-rs 派生需要构建机验证；ts-rs 对私有字段的导出行为以构建机结果为准，若不可行则回退为「DTO 保留字段 + allow 逐条登记」。
- 一次性改动面大，必须按域拆任务、每域跑 `npm run gate` + 构建机 `cargo test`，禁止「一次改完再验」。
- 与进行中的 `docs/p1-11-progress` 并行，统一在 `refactor/deep-modules` 分支完成，PR 前 rebase。

## 批准记录

- 2026-11-26：项目负责人批准立项，选择「全量（C）+ DTO 不豁免 + 构建机 linter + 本机 gate 第 7 道」，并授权 AI 主导落地、杂活优先委派本机 Claude。

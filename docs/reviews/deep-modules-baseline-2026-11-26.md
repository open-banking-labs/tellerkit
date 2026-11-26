# 深模块基线报告（2026-11-26）
> 由 Claude 生成，待 lead 复核。

## 统计范围与口径

- 范围：`packages/core/src/**/*.rs`（17 个文件）与 `apps/shell/src-tauri/src/**/*.rs`（7 个文件），共 24 个 Rust 文件、10,509 行；排除 `target/`、`node_modules/`、`.agents/`。只读源码，未运行 cargo/npm/git。
- 表 A 只计文件级 `mod name;` 声明；`#[cfg(test)] mod tests { … }` 内联测试块不算作模块切分。
- 表 B 只列具名字段的 `pub struct`；元组结构体（如 `apps/shell/src-tauri/src/bootstrap.rs:28` 的 `StartupFatal(pub Option<String>)`）不在表内。
- 表 C 计生产代码中的 `pub fn` / `pub async fn`；排除 `#[cfg(test)]` 函数与测试模块内的函数，`pub(crate) fn` / `pub(super) fn` 不计（例如 `packages/core/src/model.rs:216`、`:220`）。
- 与 `tools/check-module-depth.mjs` 的口径差异：该脚本在首个 `#[cfg(test)]` 处整文件截断，会漏掉其后仍属生产的代码。`device.rs:342` 是单个方法的属性、`replay.rs:162` 的测试块在 `:336` 已闭合，因此本报告把 `device.rs` 记为 26 个 `pub fn`（脚本口径 20）、`replay.rs` 记为 11 个（脚本口径 10），并把 `MockCardReader` / `MockPinPad` / `MockScanner` / `ReplayRun` 计入表 B。

## 表 A · 各 crate 模块可见性

| crate | 源码目录 | 文件数 | `pub mod` | `pub(crate)` / `pub(super)` / 私有 `mod` |
|---|---|---:|---:|---:|
| `tellerkit-core` | `packages/core/src` | 17 | 16 | 0 |
| `tellerkit`（主壳） | `apps/shell/src-tauri/src` | 7 | 6 | 0 |
| **合计** | — | **24** | **22** | **0** |

16 个 core 模块全部在 `packages/core/src/lib.rs:6-21` 平铺；6 个主壳模块全部在 `apps/shell/src-tauri/src/lib.rs:9-14` 平铺。

## 表 B · 含 `pub` 字段的结构体（按 pub 字段数降序）

| # | 结构体 | 所在文件 | pub 字段 / 总字段 |
|---:|---|---|---:|
| 1 | AppState | `apps/shell/src-tauri/src/lib.rs` | 18/18 |
| 2 | MenuView | `packages/core/src/filter.rs` | 11/11 |
| 3 | BootSummary | `apps/shell/src-tauri/src/app.rs` | 11/11 |
| 4 | PermSnapshot | `packages/core/src/perm.rs` | 8/8 |
| 5 | EndpointSpec | `apps/shell/src-tauri/src/endpoint_config.rs` | 8/8 |
| 6 | BizManifest | `packages/core/src/pkg.rs` | 7/7 |
| 7 | BootInfo | `apps/shell/src-tauri/src/commands.rs` | 7/7 |
| 8 | BizPackage | `packages/core/src/pkg.rs` | 6/6 |
| 9 | Endpoint | `packages/core/src/remote.rs` | 6/6 |
| 10 | ReloadReport | `apps/shell/src-tauri/src/commands.rs` | 6/6 |
| 11 | MenuPack | `packages/core/src/pack.rs` | 5/5 |
| 12 | ReversalRequest | `packages/core/src/reversal.rs` | 5/5 |
| 13 | ReversalRecord | `packages/core/src/reversal.rs` | 5/5 |
| 14 | UpdateOutcome | `packages/core/src/sync.rs` | 5/5 |
| 15 | TxRequest | `packages/core/src/tx.rs` | 5/5 |
| 16 | ReloadOutcome | `apps/shell/src-tauri/src/app.rs` | 5/5 |
| 17 | RecentTxView | `apps/shell/src-tauri/src/app.rs` | 5/5 |
| 18 | LoadReport | `apps/shell/src-tauri/src/commands.rs` | 5/5 |
| 19 | UpdateReport | `apps/shell/src-tauri/src/commands.rs` | 5/5 |
| 20 | RecentTx | `apps/shell/src-tauri/src/commands.rs` | 5/5 |
| 21 | ServiceNames | `packages/core/src/banking.rs` | 4/4 |
| 22 | Voucher | `packages/core/src/device.rs` | 4/4 |
| 23 | Devices | `packages/core/src/device.rs` | 4/4 |
| 24 | TxMeta | `packages/core/src/pkg.rs` | 4/4 |
| 25 | RouteError | `packages/core/src/remote.rs` | 4/4 |
| 26 | ReplayRun | `packages/core/src/replay.rs` | 4/4 |
| 27 | TxResult | `packages/core/src/tx.rs` | 4/4 |
| 28 | DevState | `apps/shell/src-tauri/src/commands.rs` | 4/4 |
| 29 | MenuVersionProbe | `packages/core/src/banking.rs` | 3/3 |
| 30 | MockHost | `packages/core/src/mock.rs` | 3/3 |
| 31 | Version | `packages/core/src/pkg.rs` | 3/3 |
| 32 | PackageDecision | `packages/core/src/pkg.rs` | 3/3 |
| 33 | Request | `packages/core/src/remote.rs` | 3/3 |
| 34 | FieldSpec | `packages/core/src/remote.rs` | 3/3 |
| 35 | ReplayPolicy | `packages/core/src/replay.rs` | 3/3 |
| 36 | ReplayAttempt | `packages/core/src/replay.rs` | 3/3 |
| 37 | Authorization | `packages/core/src/reversal.rs` | 3/3 |
| 38 | LoadedMenu | `packages/core/src/store.rs` | 3/3 |
| 39 | ReloadOutcome | `packages/core/src/sync.rs` | 3/3 |
| 40 | UpdateCandidate | `packages/core/src/update.rs` | 3/3 |
| 41 | LoginOutcome | `apps/shell/src-tauri/src/app.rs` | 3/3 |
| 42 | SessionView | `apps/shell/src-tauri/src/app.rs` | 3/3 |
| 43 | Reconciliation | `apps/shell/src-tauri/src/app.rs` | 3/3 |
| 44 | StatusRow | `apps/shell/src-tauri/src/app.rs` | 3/3 |
| 45 | LoginResult | `apps/shell/src-tauri/src/commands.rs` | 3/3 |
| 46 | SessionState | `apps/shell/src-tauri/src/commands.rs` | 3/3 |
| 47 | StatusRow | `apps/shell/src-tauri/src/commands.rs` | 3/3 |
| 48 | Reconciliation | `apps/shell/src-tauri/src/commands.rs` | 3/3 |
| 49 | FieldLayoutSpec | `apps/shell/src-tauri/src/endpoint_config.rs` | 3/3 |
| 50 | LoadPolicy | `apps/shell/src-tauri/src/endpoint_config.rs` | 3/3 |
| 51 | PrintReceipt | `packages/core/src/device.rs` | 2/2 |
| 52 | CardData | `packages/core/src/device.rs` | 2/2 |
| 53 | CapturedImage | `packages/core/src/device.rs` | 2/2 |
| 54 | MockCardReader | `packages/core/src/device.rs` | 2/2 |
| 55 | ShellRange | `packages/core/src/pkg.rs` | 2/2 |
| 56 | LoadPlan | `packages/core/src/pkg.rs` | 2/2 |
| 57 | WireMessage | `packages/core/src/remote.rs` | 2/2 |
| 58 | SessionPolicy | `packages/core/src/session.rs` | 2/2 |
| 59 | BootOutcome | `packages/core/src/sync.rs` | 2/2 |
| 60 | UpdateDecision | `packages/core/src/update.rs` | 2/2 |
| 61 | TxExceptionView | `apps/shell/src-tauri/src/app.rs` | 2/2 |
| 62 | RemoteSetup | `apps/shell/src-tauri/src/bootstrap.rs` | 2/2 |
| 63 | TxException | `apps/shell/src-tauri/src/commands.rs` | 2/2 |
| 64 | EndpointConfig | `apps/shell/src-tauri/src/endpoint_config.rs` | 2/2 |
| 65 | LoadedEndpoints | `apps/shell/src-tauri/src/endpoint_config.rs` | 2/2 |
| 66 | MockPrinter | `packages/core/src/device.rs` | 1/3 |
| 67 | Permissions | `packages/core/src/pkg.rs` | 1/1 |
| 68 | FixedWidthCodec | `packages/core/src/remote.rs` | 1/1 |
| 69 | MockPinPad | `packages/core/src/device.rs` | 1/1 |
| 70 | MockScanner | `packages/core/src/device.rs` | 1/1 |

合计 70 个结构体、271 个 `pub` 字段（同名结构体在不同文件中各列一行，例如 `StatusRow`、`Reconciliation`、`ReloadOutcome`）。`MenuNode`（0/11）与 `MenuTree`（0/2）已是私有字段 + 访问器，未列入本表。

## 表 C · 每文件 `pub fn` 数量（排除测试代码，降序）

| # | 文件 | pub fn |
|---:|---|---:|
| 1 | `packages/core/src/model.rs` | 29 |
| 2 | `packages/core/src/device.rs` | 26 |
| 3 | `apps/shell/src-tauri/src/commands.rs` | 23 |
| 4 | `apps/shell/src-tauri/src/app.rs` | 19 |
| 5 | `packages/core/src/mock.rs` | 15 |
| 6 | `packages/core/src/tx.rs` | 14 |
| 7 | `packages/core/src/remote.rs` | 13 |
| 8 | `packages/core/src/session.rs` | 13 |
| 9 | `packages/core/src/store.rs` | 13 |
| 10 | `packages/core/src/pkg.rs` | 11 |
| 11 | `packages/core/src/replay.rs` | 11 |
| 12 | `packages/core/src/pack.rs` | 9 |
| 13 | `packages/core/src/reversal.rs` | 8 |
| 14 | `apps/shell/src-tauri/src/endpoint_config.rs` | 6 |
| 15 | `packages/core/src/perm.rs` | 4 |
| 16 | `apps/shell/src-tauri/src/bootstrap.rs` | 4 |
| 17 | `apps/shell/src-tauri/src/config_sig.rs` | 4 |
| 18 | `packages/core/src/banking.rs` | 3 |
| 19 | `packages/core/src/sync.rs` | 3 |
| 20 | `apps/shell/src-tauri/src/lib.rs` | 3 |
| 21 | `packages/core/src/filter.rs` | 2 |
| 22 | `packages/core/src/update.rs` | 2 |
| 23 | `packages/core/src/lib.rs` | 0 |
| 24 | `apps/shell/src-tauri/src/main.rs` | 0 |

合计 235 个生产 `pub fn`；前 3 个文件占 78 个。

## 跨模块直接读写字段的典型例子

| # | 位置 | 字段定义处 | 代码摘录 |
|---:|---|---|---|
| 1 | `apps/shell/src-tauri/src/commands.rs:351` | `Voucher.reference`（`packages/core/src/device.rs:78`） | `    voucher.reference = reference;` |
| 2 | `apps/shell/src-tauri/src/app.rs:194` | `AppState.packages`（`apps/shell/src-tauri/src/lib.rs:66`） | `        self.state.packages = plan.ordered;` |
| 3 | `apps/shell/src-tauri/src/bootstrap.rs:82` | `LoadPolicy.require_signed/pubkey`（`apps/shell/src-tauri/src/endpoint_config.rs:87,89`） | `    if release && policy.require_signed && policy.pubkey.is_none() {` |
| 4 | `packages/core/src/sync.rs:118` | `MenuVersionProbe.latest`（`packages/core/src/banking.rs:19`） | `    if local == Some(probe.latest) {` |
| 5 | `packages/core/src/banking.rs:147` | `RouteError.retryable`（`packages/core/src/remote.rs:162`） | `    if e.retryable {` |

均为生产代码，访问文件与字段定义文件不同；其中 1、2 为跨 crate / 跨模块写入，3–5 为直接读取。

## 结论

- 判定：**按文件/技术层切分 + 数据袋**，不是按信息隐藏划分的深模块。
- 依据：22 个文件级 `pub mod`、0 个 `pub(crate)`/私有 `mod`；70 个结构体敞开 271 个 `pub` 字段，`AppState` 达 18/18。
- 宽度：24 个文件共 235 个生产 `pub fn`，`model.rs`（29）、`device.rs`（26）、`commands.rs`（23）三处就占 78 个。
- 局部例外：`MenuNode` / `MenuTree` 已私有字段并提供访问器，说明有深化起点，但整体接口仍以字段形态外露。

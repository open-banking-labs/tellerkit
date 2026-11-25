# Rust 代码评审（对照 Google Rust Style Guide）

- 评审日期：2026-11-25
- 范围：`packages/**`、`apps/shell/src-tauri/**`、`tools/mtree-tool/**`，共 21 个 `.rs`、8385 行（生产 4420 / 测试 3965）
- 方法：机械检查（脚本）+ `cargo clippy --workspace --all-targets` + 人工通读
- 规范：[docs/rust-style.md](rust-style.md)

---

## 结论

代码整体质量好于预期：**clippy 只有 5 条警告，无 async / unsafe / 自定义宏 / 线程**，
错误处理统一走 `thiserror`，测试密度高（测试行占 47%）。

问题集中在三类：

| 级别 | 数量 | 性质 |
|---|---|---|
| P0 正确性 | 3 | 毒锁恢复、重复属性、可直接改写为模式匹配的 expect |
| P1 规范违规 | 5 类 | 导入方式、`#[must_use]`、`#[non_exhaustive]`、断言消息、clippy |
| P2 结构/设计 | 4 | 文件主项过多、错误类型字符串化、重复类型名、clone 密度 |

---

## A. P0：正确性问题

### A-1 毒锁恢复（规范明文禁止）

> “Never reach inside a poisoned mutex to get at its insides, no matter how tempting the use case may seem.”

4 处违反：

| 位置 | 代码 |
|---|---|
| `packages/journal-sqlite/src/lib.rs:86` | `self.conn.lock().unwrap_or_else(\|e\| e.into_inner())` |
| `packages/core/src/device.rs:335` | `self.printed.lock().unwrap_or_else(...)` |
| `packages/core/src/tx.rs:176` | `*self.mode.lock().unwrap_or_else(...) = mode` |
| `packages/core/src/tx.rs:180` | `self.mode.lock().unwrap_or_else(...).clone()` |

**为什么这不是小事**：中毒意味着**上一个持锁线程 panic 了**，锁内数据可能处于半写状态。
`into_inner()` 把这种"状态可疑"当成正常，等于把一次崩溃静默降级成"可能读到了坏数据"。

**建议**：
- `SqliteJournal`：`lock()` 改为返回 `Result<MutexGuard, String>`，中毒时返回
  `"账本锁已中毒：请重启客户端"`。审计与交易都不能在坏状态上继续写。约 10 个调用点。
- `device.rs` / `DemoTxHost`（演示与测试用实现）：用 `.expect("打印记录锁中毒")` 明确暴露。

**注意**：这会改变 `TxHost` 的调用形态（`mode()` 返回 Result），建议单独 PR，
并连带补一个"中毒后拒绝写入"的测试。

### A-2 `DevState.server_version` 重复 `#[ts(type = "number")]`（✅ 本次已修）

`apps/shell/src-tauri/src/commands.rs:99-100` 属性写了两遍（clippy: duplicated attribute）。

无害，但是**契约字段被手改过的痕迹**——`fields.json` 三方比对查不出重复属性。
建议删除，并在 `check-contracts` 加一条"属性不得重复"的检查（成本很低）。

### A-3 `record_failure` 里的 `expect`（规范：prefer pattern matching）（✅ 本次已修）

`packages/core/src/replay.rs:111-114`：

```rust
self.attempts
    .get_mut(key)
    .expect("record_failure 刚插入的条目必然存在")
    .next_at_secs = next_at_secs;
```

规范要求"优先用模式匹配证明正确性，而不是靠运行时断言"。这里可以直接：

```rust
if let Some(entry) = self.attempts.get_mut(key) {
    entry.next_at_secs = next_at_secs;
}
```

语义等价，且不需要解释"为什么必然存在"。

---

## B. P1：规范违规（机械可修）

### B-1 顶层函数未用模块名限定

规范：“Qualify calls to top-level functions with a module name.”

| 位置 | 现状 | 应为 |
|---|---|---|
| `apps/shell/src-tauri/src/commands.rs:16` | `use tellerkit_core::filter::{filter_visible, MenuView}` 后直接 `filter_visible(..)`（3 处调用：326/364/397） | `use tellerkit_core::filter;` + `filter::filter_visible(..)` |
| `apps/shell/src-tauri/src/lib.rs:78` | `mock_devices()` 直接调用 | `device::mock_devices()` |

类型导入（`MenuView`、`Device`）保持不带前缀，符合规范。

### B-2 `#[must_use]` 全仓库 0 处

规范要求：纯函数、结果不可忽略的函数加 `#[must_use]`（`Result` 已隐含，不必重复）。

37 个 `pub fn` 返回 `bool`/`Option`/`Vec`/数值，其中忽略返回值会导致**静默错误**的至少有：

| 函数 | 忽略的后果 |
|---|---|
| `Devices::ensure` / `ensure_allowed` | 设备未就绪/未授权却继续办业务 |
| `DevicePolicy::permits` | 白名单判定被丢弃 |
| `SessionGuard::is_locked` / `evaluate` | 该锁不锁 |
| `ReplayScheduler::due` / `exhausted` | 重放节奏失效 |
| `MenuStore::current_version` | 版本比对失效 |

建议先加这 6 处（`ensure*` 返回 `Result` 已隐含 must_use，重点在 `permits`、`is_locked`、
`due`、`exhausted`、`current_version`、`tracked`）。

### B-3 `#[non_exhaustive]` 全仓库 0 处

规范：公开枚举应加；两变体可布尔化的、描述既有本体的可豁免。

判定：

| 分类 | 类型 | 建议 |
|---|---|---|
| 库错误枚举（应加） | `PackError`、`StoreError`、`DeviceError`、`ReversalError`、`PkgError`、`BootError`、`TreeError` | 加，调用方只匹配关心的变体 |
| 两变体（豁免） | `LoadSource`、`Channel`、`ExceptionKind`（2 种异常） | 不加 |
| 既有本体（豁免） | `NodeType`、`NodeStatus`、`DeviceKind`、`TxStatus`（三态，业务闭环） | 不加 |
| Tauri DTO | `BootInfo` 等 8 个 | **不加**，已在偏离表 §3.3 登记 |

注意：`ReversalError::NotReversible` 等无数据变体加上 `non_exhaustive` 后，
`assert_eq!` 仍可用（`PartialEq` 不受影响）。

### B-4 断言缺上下文消息 409 / 546

规范：“Use context strings to clarify assertions.”

现有 137 条带消息（多在我手写的边界用例里），409 条裸断言。
`assert_eq!(a, b)` 失败时只打印值，不打印"这是哪个场景"。

**建议不必全补**，优先这三类（失败时最难定位）：
1. 状态机用例（`tx.rs`、`reversal.rs`、`session.rs`）
2. 权限/安全用例（`filter.rs`、`device.rs` 脱敏）
3. 序列化/契约用例（`pack.rs`、`pkg.rs`）

### B-5 clippy 5 条（✅ 本次已修，CI 已接 `-D warnings`）

| 位置 | 警告 |
|---|---|
| `model.rs:172` | collapsible_if |
| `tx.rs:212` | derivable_impls（`Default for TxEngine` 可 derive） |
| `commands.rs:100` | duplicated attribute（同 A-2） |
| `pkg.rs:582` | `== false` 应写成 `!` |
| `mtree-tool/src/main.rs:207` | map_or 可简化 |

**建议**：修掉后把 `cargo clippy --workspace --all-targets -- -D warnings` 加进 CI。
不进门禁（`npm run gate`）是因为 gate 只跑 node，不编译；CI 里已经有完整编译。

---

## C. P2：结构与设计

### C-1 `device.rs` 1043 行，一个文件 5 个主项

规范：“Name source files and leaf modules after the primary user-defined item.”
一个文件应有一个主项。

`device.rs` 同时包含：`DeviceKind/DeviceStatus/DeviceError`（类型）、四个 trait、
`DevicePolicy`、四个 mock 实现、`mock_devices()`。生产 341 行、测试 703 行。

**建议**拆为：

```
device/
  mod.rs        re-export + Devices（组合根）
  traits.rs     Printer/CardReader/PinPad/Scanner
  policy.rs     DevicePolicy
  mock.rs       MockXxx + mock_devices()
```

**收益**：新增一个设备类型时不用滚 1000 行。**成本**：一次纯搬移，需重跑全部 22 个 device 用例。

### C-2 `tx.rs` 849 行，同类问题

含 `TxStatus`、`TxRequest`、`TxResult`、`HostError`、`TxHost`、`TxEngine`、`DemoTxHost`。
建议拆 `tx/{model,engine,host}.rs`，与 C-1 同一 PR 做，减少合并冲突。

### C-3 `commands.rs` 645 行：DTO 与命令混在一起

8 个 DTO（`BootInfo`、`LoadReport`、`LoginResult`、`UpdateReport`、`ReloadReport`、
`DevState`、`SessionState`）+ 20 条命令 + 审计/会话辅助函数。

**建议**：DTO 移到 `dtos.rs`，`commands.rs` 只留命令。
注意 `ts-rs` 按模块扫描，移文件后需重跑 `npm run bindings`（可验证）。

### C-4 两个 `HostError`

`packages/core/src/mock.rs` 有 `HostError`（菜单宿主），`packages/core/src/tx.rs` 也有
`HostError`（交易宿主），`lib.rs` 只 re-export 后者。

同名字段不同语义，读代码时必须确认是哪个。建议改名为 `MenuHostError` / `TxHostError`。
**成本**：改动集中在 core 内部，不影响契约。

### C-5 错误类型字符串化

`PackError::DecodeFailed(String)`、`StoreError::BadDir(String)`、`PkgError::BadPackageId(String)`
把结构化信息压成字符串。规范建议 `Result` 的错误要能让调用方区分失败模式。

**建议**：`DecodeFailed` 改为 `DecodeFailed { kind: DecodeKind, detail: String }`，
或至少在 `#[error]` 文案里保留可判定的前缀。**优先级低**——当前没有调用方按错误种类分支。

### C-6 `filter_visible` 的 clone 密度

`filter.rs` 生产代码 8 处 `.clone()`，每个节点都克隆 `code/title/route/icon/actions`。
2000 节点实测 0.07ms，**当前不是问题**。

记录在案：若菜单到数千节点且需要每秒重算，再改 `Arc<str>`。这属于"有实测再优化"，
现在改是过早优化。

---

## D. 合规确认（无需改动）

- 无 glob import；测试模块的 `use super::*` 属官方允许例外
- 无 `async`、无 `unsafe`、无自定义宏、无线程创建
- 错误统一 `thiserror`，`#[from]` 保留 source 链
- 选项结构体 `SessionPolicy`/`ReplayPolicy` 已实现 `Default`
- 初始化命名符合标准库风格（`new` / `open` / `with_journal` / `in_memory`）
- 中文注释与文案已按 §3.1 登记为有意偏离

---

## 建议的执行顺序

| 阶段 | 内容 | 风险 |
|---|---|---|
| 1（一次 PR） | B-5 clippy 5 条 + B-1 导入方式 + A-2 重复属性 + A-3 expect 改模式匹配 | 极低，机械改动 |
| 2（一次 PR） | B-3 给 7 个错误枚举加 `#[non_exhaustive]` + B-2 关键 `#[must_use]`；CI 接 clippy | 低，编译器会指出所有需要改的 match |
| 3（一次 PR） | A-1 毒锁：`SqliteJournal` 返回错误、mock 用 expect；补中毒用例 | 中，涉及 10 个调用点 |
| 4（一次 PR） | C-1/C-2 拆 `device/`、`tx/` 目录 | 中，纯搬移 + 重跑测试 |
| 5（按需） | B-4 断言消息（状态机/安全/契约三类）、C-3 DTO 分离、C-4 改名 | 低 |

**不建议做**：把 `tx`/`pkg`/`biz` 这些缩写改名成完整单词。它们已经是本项目与
契约（`contracts/`、生成的 TS 类型、包格式）的一部分，改名会牵动契约、门禁、生成物
与历史记录，而不会提升可读性——属于规范里 `IpAddr` 那类"外部不易误解的领域缩写"。
已在 `docs/rust-style.md` 登记。

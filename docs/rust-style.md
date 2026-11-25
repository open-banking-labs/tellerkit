# Rust 代码规范

> 采用：[Google Rust Style Guide](https://google.github.io/styleguide/rustguide.html)
> 采纳日期：2026-11-25
> 状态：生效。本文只写**本项目如何落地**，规则原文以官方为准。

## 1. 为什么采用它

它的目标顺序和本项目一致：可读性 > 可写性；一致性能减少无谓争论；
而且它把「API 设计」也算进 style，不只是缩进。对我们这种
"人 + AI 共同维护、代码要活很多年"的仓库，比纯格式化规范有用得多。

## 2. 怎么落地

规范分两类，处理方式不同：

| 类型 | 例子 | 落地方式 |
|---|---|---|
| 机器可判 | 格式化、clippy 能查的风格 | 工具强制：`cargo fmt`、`cargo clippy` 进门禁 |
| 需要判断 | 导入方式、API 形态、命名、文档 | 进评审清单（本文 §4），人工/Agent 评审时逐条对照 |

**只写进文档的规则等于没有规则。** 能自动化的必须自动化，不能自动化的必须有评审动作。

## 3. 本项目对规范的偏离（有意为之）

官方指南面向 Google 内部代码库，有三处与本项目语境冲突，明确偏离：

### 3.1 注释与错误文案用中文（偏离：无对应规则，但影响互通性）

官方未要求英文，但 Google 内部实际全英文。本项目保留中文，理由：

- 错误文案直接展示给柜员（`"网络不可用，且该交易不支持离线"`），必须中文；
- 审计与合规文档是中文，注释跟着走可减少一次翻译；
- 代码标识符仍用英文（`offline_capable`、`filter_visible`），不译。

**约束**：标识符、提交信息里的技术名词用英文；面向用户的文案用中文。

### 3.2 `Arc<SqliteJournal>`（偏离：Avoid Rc and Arc）

官方要求优先用所有权与借用，`Arc` 仅在替代结构过复杂时使用。我们有三处：

| 位置 | 为什么保留 |
|---|---|
| `apps/shell/src-tauri/src/lib.rs` `ArcJournal` | `TxEngine` 需要 `Box<dyn TxJournal>` 的所有权，命令层还要按需查审计。要避免 Arc 就得引入连接池或把引擎拆成两个对象，复杂度更高 |
| `device.rs` 测试里的 `SharedPrinter` | 只有一个消费方在测试里；为它引入生命周期参数不值得 |

**判定**：这属于官方允许的"替代结构复杂到不合理"的例外。新增 `Arc` 时必须在此表登记理由。

### 3.3 Tauri DTO 不加 `#[non_exhaustive]`（偏离：Apply #[non_exhaustive] to public structs with public fields）

官方要求公开字段结构体加 `#[non_exhaustive]`，防止加字段造成破坏性变更。
本项目**不加**，理由：

- 这些 DTO 的跨语言契约由 `ts-rs` 生成并进门禁（`check-contracts` 三方比对），
  字段变更本来就必须同步契约，不需要编译器再拦一次；
- 加了之后 `#[non_exhaustive]` 会禁止跨 crate 的结构体字面量构造，
  而 `commands.rs` 恰恰要构造 `AuditEvent` 等类型，得再加一堆构造函数。

**例外**：`tellerkit-core` 的错误枚举（`PackError`、`StoreError`、`DeviceError`、
`ReversalError`、`BootError`）视为库 API，**应当**加 `#[non_exhaustive]`——
调用方只该匹配自己关心的变体。见评审报告 A-3。

## 4. 评审清单

### 4.1 代码组织

- [ ] 没有 glob import（`use super::*` 仅限测试模块，属官方允许例外）
- [ ] 一条 `use` 只从**一个**模块导入（花括号里不出现 `::`）
- [ ] 类型名不带模块前缀使用（`MenuView` 而非 `core::MenuView`）
- [ ] 顶层函数**带模块名调用**（`filter::filter_visible(..)` 而非直接 `filter_visible(..)`）
- [ ] trait 在函数体内局部导入，除非全文件广泛使用

### 4.2 API 形态

- [ ] 不可变的借用参数尽量宽松（`&str` / `&[T]` / `&Path`，而非 `&String` / `&Vec<T>` / `&PathBuf`）
- [ ] 返回借用而非深拷贝，除非确实需要拥有
- [ ] 用途不明显的 `bool` 参数改成枚举
- [ ] 纯函数 / 结果不可忽略的函数加 `#[must_use]`（返回 `Result` 的已隐含，不必重复）
- [ ] 库错误枚举加 `#[non_exhaustive]`
- [ ] `Debug` 等标准 trait 用 `#[derive]`，不自写
- [ ] 选项结构体实现 `Default`

### 4.3 错误处理

- [ ] 生产代码不 `unwrap()`；必要时 `expect("说明为什么不可能失败")`
- [ ] 不用 `is_some()` + `unwrap()` 的组合，改用 `if let` / `let else`
- [ ] 不在 mutex 中毒后 `into_inner()` 取内部数据（**明令禁止**）
- [ ] 无 `async`（架构全同步；**唯一例外是 IPC 命令层**——Tauri 同步命令跑主线程会冻界面，
      见 [`changes/018`](../changes/018-ipc-async-threading.md)。其余任何一层引入 `async` 前须立 ADR）

### 4.4 命名与文档

- [ ] 避免生僻缩写；领域内通用缩写（`tx`、`pkg`、`mtree`）需在本文登记
- [ ] `new` / `open` / `with_*` 等初始化命名遵循标准库风格
- [ ] `pub` 项有文档注释，说明**为什么**与边界条件，而非复述签名

### 4.5 实现细节

- [ ] 不滥用类型推断：读代码的人能看出类型
- [ ] 迭代器链不比直接循环更清楚时才用
- [ ] 断言带上下文消息（失败时能知道是哪个 case）
- [ ] 不用 `panic::catch_unwind`

## 5. 当前落地状态

| 规则 | 状态 |
|---|---|
| `cargo fmt`（max_width=100） | ✅ 门禁已强制 |
| `cargo clippy -D warnings` | ✅ CI 已强制（本次清零） |
| 导入方式（4.1） | ⚠️ 2 处违规（`filter_visible` / `mock_devices`） |
| `#[must_use]` | ⚠️ 全仓库 0 处 |
| `#[non_exhaustive]`（错误枚举） | ⚠️ 全仓库 0 处 |
| 毒锁处理 | ❌ 4 处违反（评审报告 A-1） |
| 断言上下文消息 | ⚠️ 137/546 带消息 |
| 中文偏离（3.1） | ✅ 已在本文件登记 |

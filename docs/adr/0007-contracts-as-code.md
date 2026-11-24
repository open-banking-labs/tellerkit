# ADR-0007：契约即代码，前端类型由 Rust 生成

- 状态：已接受
- 日期：2026-11-24
- 决策者：项目负责人

## 背景

W2 之前，`apps/shell/src/contracts.ts` 是手写的：Rust 改了字段，要有人记得同步 TS。
本项目已经用 `contracts/fields.json` + `check-contracts` 做双向比对，能发现不一致——
但那是**事后发现**，不是**结构上不可能**。

而且手写类型还有一个更隐蔽的代价：命令名是字符串字面量，
`invoke("boot")` 拼错只会在运行时变成 "command not found"。

## 决策

1. 用 **ts-rs 12** 从 Rust 结构体生成 TS 类型，输出到 `apps/shell/src/generated/`。
2. 命令名从 `generate_handler!` 生成 `CommandName` 联合类型，所有 IPC 调用走
   `call<T>(cmd: CommandName, …)`，拼错即**类型错误**。
3. 生成物带 `manifest.json`（每文件 sha256），门禁据此拒绝手改与遗漏。
4. 手写 `src/contracts.ts` **禁止回流**，门禁直接拦。
5. `cargo test` 时 ts-rs 会自动导出（`.cargo/config.toml` 固定了导出目录）。

## 三方一致性

```
contracts/fields.json  ←→  Rust 结构体  ←→  generated/*.ts
        ↑                                        ↑
        └────── manifest.json（sha256）──────────┘
```

`check-contracts` 同时校验三份：任何一份落后于另外两份，门禁红。

## 为什么不用 tauri-specta

`tauri-specta` 能一次生成类型 + 命令绑定 + 客户端调用函数，体验更好。
但 2.0 仍是 `2.0.0-rc.29`（specta 2 是 `rc.28`）。

把 RC 版本放进"生产级团队骨架"的基础设施里，和宪法第 7 条（减法优先）冲突：
它的收益是省一层薄封装，代价是把契约链路押在预发布 API 上。
等 specta 2 正式发布再重新评估，届时替换成本只有 `api.ts` 一个文件。

## 代价

| 代价 | 缓解 |
|---|---|
| `cargo test` 会写文件（ts-rs 的导出测试） | 导出目录固定，幂等；CI 用 `git diff --exit-code` 检查是否落后 |
| 生成物包含 Rust 文档注释，改注释也会改 hash | 接受：注释是产物的一部分，`npm run bindings` 一条命令解决 |
| `u64` 默认映射成 `bigint`，与 JSON 实际不符 | 已在字段上加 `#[ts(type = "number")]`，显式声明而不是靠默认值 |
| 生成的类型是 `export type X = {...}` 而非 `interface` | 不影响使用；门禁的解析器同时支持两种 |

## 流程

```bash
# 改 Rust 后
npm run bindings     # 重新生成
npm run gate         # 三方一致性 + 手改检测
```

# 002 交易提交与凭证打印的 IPC 契约

## 现状

`contracts/ipc-commands.md` 只有启动、登录、菜单与演示命令。
`TxEngine`（P1.1）与设备抽象（P1.2）都只存在于 Rust 侧，前端无法发起交易。

## 建议新增三条命令

| 命令 | 参数 | 返回 | 说明 |
|---|---|---|---|
| `submit_transaction` | `txCode`, `idempotencyKey`, `amountCents`, `elements`, `offlineCapable` | `TxResult` | 幂等键由前端生成（`crypto.randomUUID()`） |
| `print_voucher` | `template`, `txCode`, `reference?`, `fields` | `PrintReceipt` | 凭证内容由交易结果驱动 |
| `tx_pending_count` | — | `number` | 待重放笔数，供界面提示 |

## 新增导出类型

`TxStatus`（`committed` / `pending` / `rejected`）、`TxResult`、`PrintReceipt`。
全部由 Rust `#[ts(export)]` 生成，前端不手写。

## 为什么 `TxResult` 直接作为返回值

它已经是"柜员可见的结论"（状态 + 流水号 + 文案 + 是否可重放），
再包一层 DTO 只会让两边各维护一份字段，且容易在包装层丢字段。

## 影响

- `contracts/ipc-commands.md`、`contracts/shell-api.md`、`contracts/fields.json`
- `apps/shell/src-tauri/src/commands.rs`、`lib.rs`
- `apps/shell/src/api.ts` 与新增的交易面板组件
- `packages/core/src/tx.rs`、`device.rs` 增加 `#[ts(export)]` 与 `Send + Sync` 约束

## 风险

`elements` 是 `BTreeMap<String, String>`，跨 IPC 传输时值一律字符串。
金额用 `amountCents: i64` 而不是浮点，避免前端出现 0.1+0.2 这类误差。

## 批准方式

本提案与实现放在同一个 PR 里。**合并该 PR 即视为批准本提案**；
若只批准契约而不批准实现，请在评审时拆开，我先落契约再补实现。

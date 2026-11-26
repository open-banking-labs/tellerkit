# 019 IPC 包身份上下文与冲正授权要素（对应 security-hardening T-01）

- 对应任务：`.specify/specs/040-security-hardening/tasks.md` 的 **T-01**
- 对应场景：S-05、S-06

## 现状

`contracts/ipc-commands.md` 中 `submit_transaction` 的参数是
`txCode, idempotencyKey, amountCents, elements, offlineCapable`，没有业务包身份；
`request_reversal` 只有 `key`、`reason`，授权要素由调用方给。
于是沙箱里的业务包可以冒用其他交易码，冲正授权也可能拿到伪造要素。

## 建议

由负责人评审后修改契约（本提案不直接改 `contracts/**`）：

1. `submit_transaction` 增加 `packageId?: string`（可选，主壳自发调用时为空）；
   `offlineCapable` 改为服务端/包清单判定值，不接受调用方覆盖。
2. `request_reversal` 增加只读的授权要素视图：交易码、金额、账号掩码、发生时间，
   来源固定为本地账本；调用方只能传 `key` 与理由。
3. 被拒绝时返回中文可读错误，并在审计中记录 `packageId` 与拒绝原因。
4. `contracts/fields.json` 同步登记新增字段；`generated/` 由 `npm run bindings` 重出，
   `check-contracts` 的有效参数个数必须随之通过。

## 影响

- Rust：`commands.rs` 参数、`app.rs` 用例入参、账本读取路径。
- 前端：`src/api.ts` 与沙箱桥接传 `packageId`。
- 测试：覆盖「包声明 t1 却提交 t2」与「冲正要素来自账本而非调用方」两条负向用例。

## 风险

`packageId` 为可选参数会放宽旧调用方；需在评审中确认是否改为必填，
以及在主壳自发交易时用什么固定标识。

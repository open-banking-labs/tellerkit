# 011 冲正记录是"意图"，不是"结果"

## 现状

`ReversalBook::request()` 在本地授权通过后立刻返回 `ReversalRecord`，
`outstanding()` 随即把这笔从待办里扣除。

也就是说：**客户端授权完就认为冲正完成了**，主机还没回执。

## 问题

冲正是账务操作。如果主机没受理、或者拒绝，客户端已经显示"已处置"，
柜员不会再去查——账上没冲，界面说冲了。

这与 `docs/desktop-scope.md` §4 冲突：冲正只有主机回了流水号才算处置完成。

## 建议

把记录拆成两阶段：

1. `request()` 产生 `ReversalRecord { host_reference: None }`——**申请已授权，未确认**；
2. `confirm(key, host_reference)` 由**主机回执处理路径**调用，写入流水号；
3. `outstanding()` 只扣除 `is_confirmed()` 为真的交易；
4. 重复申请时的错误区分两种：
   - 已授权未确认 → `AwaitingConfirmation`（还能等回执）
   - 已确认 → `AlreadyReversed`（只能人工查）

## 影响

- `packages/core/src/reversal.rs`
- `.specify/specs/030-reversal-authorization/spec.md`（新增 S-07）
- 主壳尚未接冲正命令，所以**没有调用方需要改**——这是改在这个时点的原因

## 风险

`confirm()` 由谁调用是新的信任点。如果将来有人为了"界面好看"直接本地 confirm，
这条就白做了。约束写在方法文档里，评审时按 `docs/desktop-scope.md` §6 打回。

# 013 超时不得报告为"拒绝"

## 现状

`TxEngine::submit` 在"超时/不可达 + 该交易不允许离线"时返回 `TxStatus::Rejected`，
文案是"网络不可用，且该交易不支持离线：请用同一笔业务**重试**"。

## 问题

超时的真实含义是**状态不明**：主机可能已经入账，也可能没有。
报告成 `Rejected` 会让柜员认为"这笔没成"，于是重做一笔——
而主机可能已经扣过款。这是重复扣账的经典路径。

`docs/adr/0007` 说的是"不设 Unknown 终态"，不是"把 Unknown 说成 Rejected"。

## 建议

新增第四态 `TxStatus::MarkedForReversal`：

- 超时/不可达 + 不支持离线 → 该状态；
- 不写幂等索引（`cacheable = false`），保持原键可重试；
- 上层用 `ReversalBook` 标记待冲正，进入查账/冲正流程；
- 账本里存 `marked_for_reversal`，与 `rejected` 分开，对账时能看出哪些需要查证。

## 冲正流程（同批实现）

```
交易状态不明 → ReversalBook::mark
柜员申请冲正 → 主管授权 → 主机 reverse() → 拿到流水号 → confirm()
```

只有 `confirm()` 之后才算处置完成。主机失败时保留"已授权未确认"，可重试。

## 影响

- `packages/core/src/tx.rs`（状态与分支）
- `packages/core/src/banking.rs`（`RemoteBanking::reverse`）
- `contracts/`：`TxException`、`tx_exceptions`、`request_reversal`
- `apps/shell`：用例、命令、交易面板

## 风险

`RemoteBanking::reverse` 是**账务操作**。它与提交交易一样必须由主机做幂等；
当前假后台与 mock 只是模拟。上线前需服务端确认反向接口的幂等语义。

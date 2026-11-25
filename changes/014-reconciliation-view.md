# 014 日终对账视图

## 现状

账本里已经有 `tx_log`（每笔交易的状态与金额），但界面上看不到。
柜员关账前只能凭记忆，或者去翻数据库。

## 建议

新增只读命令 `tx_reconciliation`：

```jsonc
{
  "since": "00000000001760000000",   // 当日 00:00（秒级，零填充）
  "summary": [ { "status": "committed", "count": 12, "amount_cents": 1234500 } ],
  "recent":  [ { "key": "…", "tx_code": "account.deposit", "status": "committed", "reference": "TX00000042" } ]
}
```

界面入口放在头部（登录后可见），弹出面板展示聚合 + 最近 20 笔明细。

## 为什么是只读

对账是**查看**行为，不改任何状态。真正的账务一致性以核心系统为准，
这里解决的是"柜员关账前先自查一遍"。

## 为什么不查主机

客户端没有对账接口（那是核心系统的能力）。本地视图只回答"我这台机器上记录了什么"，
不宣称与主机一致——文案里明确写了这一点。

## 影响

- `packages/journal-sqlite`：`summary` / `recent` 两个只读查询
- `apps/shell`：用例、命令、`ReconcilePanel.vue`、头部入口
- `contracts/`：`StatusRow`、`RecentTx`、`Reconciliation`、`tx_reconciliation`

## 风险

`start_of_day` 用的是 **UTC 日界**（`now/86400*86400`）。柜面日终如果按本地时间零点算，
跨时区网点会出现"昨天的交易算进今天"。要按本地时区切分需要引入时区库或从系统取偏移，
记为已知缺口（P4）。

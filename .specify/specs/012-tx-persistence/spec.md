# 交易落盘与重放 · 行为规格

- 能力 ID：`012-tx-persistence`
- 状态：approved（P1.4）
- 关联契约：`contracts/lifecycle.md`、`changes/003`

## 背景

P1.1 的挂起队列在内存里：进程一重启，断网时挂起的交易全丢。
柜面最不能接受的就是"客户走了、钱没记、系统还不知道"。

## 范围

**做**：持久化端口（`TxJournal`）、SQLite 账本、启动读回、手动重放、审计日志。

**不做**：自动重放（要等对账策略）、跨终端同步、账本加密（P2）。

## 场景

### S-01 挂起即落盘

```gherkin
GIVEN 交易 offline_capable=true 且主机不可达
WHEN 提交
THEN 返回 Pending
AND 该请求写入待重放队列（磁盘）
AND 审计日志记录一条 pending
```

### S-02 重启后能读回并重放

```gherkin
GIVEN 上次进程留下了 1 笔挂起交易
WHEN 重新启动并重放
THEN 引擎里能读到这 1 笔
AND 主机可用时重放成功并出队
AND 审计日志追加一条 committed
```

### S-03 重放仍失败不丢单

```gherkin
GIVEN 挂起队列里有交易
AND 主机仍不可达
WHEN 重放
THEN 状态仍为 Pending
AND 请求保留在队列中
```

### S-04 账本打不开时降级而不是打不开客户端

```gherkin
GIVEN 账本文件不可写
WHEN 启动
THEN 客户端仍能启动
AND 启动提示里说明"账本不可用：本会话不会落盘"
```

### S-05 审计日志保留每一次状态迁移

```gherkin
GIVEN 同一笔交易先挂起后成功
WHEN 查询该幂等键的历史
THEN 依次返回 pending 与 committed
```

## 验收标准

- `cargo test -p tellerkit-journal-sqlite` 全绿，且含"重开文件后队列还在"的用例
- `cargo test -p tellerkit-core tx::` 仍全绿（端口不得污染纯逻辑）
- 重放汇总文案能被柜员直接读懂

## 不变量

1. 记账失败不改变交易结论
2. 只有重放成功才出队；不可达绝不删
3. core 不依赖任何数据库

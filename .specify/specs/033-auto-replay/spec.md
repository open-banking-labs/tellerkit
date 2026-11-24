# 自动重放 · 行为规格

- 能力 ID：`033-auto-replay`
- 状态：approved（P3.4）

## 背景

主机刚恢复时最脆弱。一股脑把所有挂起交易推过去，容易打垮主机，
也会制造"部分成功"的半批状态。所以重放要有节奏、有上限、有退路。

## 范围

**做**：重放调度策略（到期判定、指数退避、放弃上限、成功移除）。

**不做**：执行重放（`TxEngine::replay_pending`）、与网络状态联动（上层决定何时 tick）。

## 场景

### S-01 新登记的交易立即可重放

```gherkin
GIVEN 调度器为空
WHEN 登记一笔交易于 t=100
THEN t=100 时它在 due 列表里
```

### S-02 失败后按指数退避推迟

```gherkin
GIVEN 基准退避 30 秒、上限 600 秒
WHEN 连续失败 1、2、3 次
THEN 下次重放时刻依次为 now+30、now+60、now+120
```

### S-03 退避不超过上限

```gherkin
GIVEN 基准退避 30 秒、上限 600 秒
WHEN 计算第 10 次失败后的退避
THEN 结果为 600 秒
```

### S-04 达到上限后放弃自动重放

```gherkin
GIVEN max_attempts = 3
WHEN 失败 3 次
THEN exhausted 为真
AND 它不再出现在 due 列表
```

### S-05 成功即移除

```gherkin
GIVEN 一笔交易在管
WHEN 重放成功
THEN 从调度器移除，tracked 减一
```

### S-06 到期顺序可复现

```gherkin
GIVEN 多笔交易同时到期
WHEN 取 due 列表
THEN 按幂等键排序，多次调用顺序一致
```

## 验收标准

- `cargo test -p tellerkit-core replay::` 全绿
- 退避用例逐次断言具体秒数，不能只断言"变大了"

## 不变量

1. 纯逻辑：时间由调用方传入
2. 成功后立即移除
3. 放弃后不再自动重放（必须人工）

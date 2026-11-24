# 交易闭环 · 行为规格

- 能力 ID：`010-transaction-loop`
- 状态：approved（P1 第一片）
- 关联契约：`contracts/lifecycle.md`（不变量）、`contracts/menu-biz-schema.md`（交易元数据）

## 背景

柜面最怕的不是交易失败，是**交易状态不明**：扣了钱没记账、记账了没凭证。
根因是客户端把"请求超时"直接当成"失败"，然后让柜员重做一笔。

P1 要解决的是：提交、判态、离线边界、幂等、可重放——先把**状态机**做对，
再谈界面与外设。

## 范围

**做**：交易请求模型、状态机（Committed / Pending / Rejected）、客户端幂等键去重、
离线边界（`offline_capable` 决定断网时拒绝还是挂起）、待处理队列。

**不做**（后续切片）：
- 不做真实主机接口（host 是 trait，测试用 mock）
- 不做 SQLite 落盘（P1.4）
- 不做冲正状态机（P3）
- 不做界面（P1.3）
- 不做外设调用（P1.2）

## 场景

### S-01 提交成功

```gherkin
GIVEN 主机可用
WHEN 提交一笔合法交易
THEN 状态为 Committed
AND 返回主机流水号作为 reference
```

### S-02 幂等：重复提交只入账一次

```gherkin
GIVEN 同一个幂等键已经提交并成功
WHEN 再次提交同一个键
THEN 返回首次的结果（同一个 reference）
AND 主机只被调用一次
```

### S-03 断网且交易不支持离线

```gherkin
GIVEN 交易声明 offline_capable=false
AND 主机不可达
WHEN 提交
THEN 状态为 Rejected
AND 不进入待处理队列
AND 提示文案说明"该交易不支持离线"
AND 该拒绝**不被永久缓存**：网络恢复后同一幂等键可以重试（见 S-06）
```

### S-04 断网且交易支持离线

```gherkin
GIVEN 交易声明 offline_capable=true
AND 主机不可达
WHEN 提交
THEN 状态为 Pending
AND 请求进入待处理队列且可重放
```

### S-05 业务拒绝不重试

```gherkin
GIVEN 主机返回业务失败（如余额不足）
WHEN 提交
THEN 状态为 Rejected
AND 标记为不可重放
AND 不进入待处理队列
```

### S-06 瞬时失败不得永久缓存

```gherkin
GIVEN 交易 offline_capable=false，首次因网络不可用被拒绝
WHEN 网络恢复后用**同一幂等键**重试
THEN 第二次真正提交到主机并可能成功
AND 不得直接返回首次的拒绝
```

理由：超时意味着"不知道主机是否已处理"。把拒绝永久缓存，柜员就只能换新键重试——
那才是重复扣账的真正来源。同一键重试 + 主机按键去重，才是唯一安全的路径。

### S-07 幂等键被挪作他用必须显式拒绝

```gherkin
GIVEN 某个幂等键已用于一笔成功交易
WHEN 用同一键但不同交易码或不同金额提交
THEN 拒绝，并说明"幂等键已用于另一笔交易"
AND 不得返回第一笔的流水号
AND 不得把请求发给主机
```

理由：只按 key 去重会让"另一笔业务"看起来成功了，柜员不会发现钱没动。

## 验收标准

- `cargo test -p tellerkit-core tx::` 全绿，且覆盖 S-01～S-07
- 幂等用例必须断言"主机调用次数"，不能只看返回结果

## 不变量

1. 幂等键由客户端生成（断网时拿不到服务端 ID）
2. 资金类交易默认 `offline_capable=false`，只有服务端配置能覆盖
3. 超时不得直接判失败：要么挂起可重放，要么明确拒绝，不能"假装成功"

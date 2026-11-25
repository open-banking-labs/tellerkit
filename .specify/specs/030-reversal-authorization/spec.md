# 冲正与授权 · 行为规格

- 能力 ID：`030-reversal-authorization`
- 状态：approved（P3.1）
- 关联：`contracts/lifecycle.md`（状态不明不得当失败）

## 背景

超时的交易可能已入账。把它当"失败"让柜员重做，就是重复记账；
把它当"成功"又不生成凭证，就是钱没记。两种都是事故。

正确做法是第三种状态：**异常待处置**。由人查账、由**他人**授权处置。

## 范围

**做**：异常标记（待冲正 / 待推进）、冲正申请、授权校验、幂等（不可重复冲正）。

**不做**：与主机的冲正接口对接（`Authorizer` 与主机都是端口）、跨日冲正、
自动判定交易是否异常（那要主机回执，P3 后续）。

## 场景

### S-01 异常交易进入待处置，而不是失败

```gherkin
GIVEN 一笔交易状态不明
WHEN 标记为 MarkedForReversal
THEN 它出现在待处置清单里
```

### S-02 没有异常标记的交易不能冲正

```gherkin
GIVEN 交易未标记
WHEN 申请冲正
THEN 返回 NotMarked
AND 不请求授权（不打扰主管）
```

### S-03 发起人不能自批

```gherkin
GIVEN 冲正由柜员 T01 发起
WHEN 授权人也是 T01
THEN 返回 SelfApproval
AND 即使授权端口返回 granted=true 也不成立
```

### S-04 授权被拒则不留痕

```gherkin
GIVEN 授权端口拒绝
WHEN 申请冲正
THEN 返回 NotAuthorized
AND 不产生冲正记录
AND 交易仍在待处置清单里
```

### S-05 不能重复冲正

```gherkin
GIVEN 一笔交易已冲正成功
WHEN 再次申请冲正
THEN 返回 AlreadyReversed
```

### S-07 授权完成不等于处置完成

```gherkin
GIVEN 冲正已获授权
WHEN 主机尚未回流水号
THEN 该笔仍在待处置清单里
AND 再次申请返回 AwaitingConfirmation（不是 AlreadyReversed）
WHEN 主机回执后确认
THEN 才从待处置清单移除，且不能重复确认
```

理由：冲正是账务操作，账只有一个地方算。客户端把"已授权"当"已冲正"，
会出现"柜员以为冲了、账上没冲"。

### S-06 待处置清单顺序稳定

```gherkin
GIVEN 多笔异常交易
WHEN 多次读取待处置清单
THEN 顺序一致（按幂等键排序）
```

## 验收标准

- `cargo test -p tellerkit-core reversal::` 全绿，覆盖 S-01～S-06
- 自批用例必须构造"授权端口返回 granted=true"的情形，证明是引擎拦下的

## 不变量

1. 冲正必须他人授权
2. 同一笔交易只能冲正一次
3. 授权被拒不改变任何状态

# <能力名> · 行为规格

- 能力 ID：`<NNN-capability>`
- 状态：draft | approved | implemented
- 关联契约：`contracts/xxx.md`

## 背景

一句话说明为什么需要这个能力，以及不做会怎样。

## 范围

**做**：
**不做**（明确写出来，防止 AI 扩范围）：

## 场景

### S-01 <场景名>

```gherkin
GIVEN 前置条件
WHEN 触发动作
THEN 可观察结果
```

### S-02 <场景名>

```gherkin
...
```

## 验收标准

能被 `npm run gate` 或人工按步骤复现的、可证伪的断言。不得写"体验良好"这类无法判定的标准。

## 不变量

本能力引入后，任何改动都不得破坏的约束。

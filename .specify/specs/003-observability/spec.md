# 可观测 · 行为规格

- 能力 ID：`003-observability`
- 状态：approved
- 关联：`docs/metrics.md`、`.agents/dispatches/README.md`

## 背景

W4 之后仓库里有了派发记录，但记录本身不是可观测——没人会去翻 50 个 JSON。
可观测的最低标准是：**指标变化时能给出下一步动作**，而不是只给一张图。

反例：打回率从 10% 涨到 40%，如果报告只说"40%"，读者仍不知道去改提示词、
拆任务、还是修验收命令。

## 范围

**做**：从派发记录聚合指标、追溯检查（记录引用的任务是否还在）、给出判断与动作建议。

**不做**：
- 不做仪表盘 / 时序数据库（当前量级不需要）
- 不采集 token 成本（需要宿主上报，见「已知缺口」）
- 不把指标做成门禁（指标用于决策，不用于拦截）

## 场景

### S-01 汇总指标

```gherkin
GIVEN 一组派发记录，含 PASS / FAIL / OUT_OF_SCOPE 三种已裁决状态
WHEN 运行聚合
THEN 输出总数、已裁决/未裁决、结论分布
AND 输出打回率（FAIL + OUT_OF_SCOPE 占比）与越界率
AND 输出耗时中位数、P90、最大值
```

### S-02 未裁决必须可见

```gherkin
GIVEN 存在 status 为 dispatched 但从未裁决的记录
WHEN 运行聚合
THEN 未裁决数量单独列出
AND 给出"未裁决等于没有结论"的判断
```

### S-03 追溯失效必须报警

```gherkin
GIVEN 某条记录引用的任务已从 tasks.md 中删除或改名
WHEN 运行聚合
THEN 指出该记录与失效原因
AND 提示补回任务或归档记录
```

### S-04 报告可机读

```gherkin
GIVEN 任意记录集
WHEN 以 --json 运行
THEN 输出稳定结构：dir / broken / metrics / trace_issues / hints
AND metrics 内包含 by_status、reject_rate_pct、by_preset、by_layer、by_task
```

## 验收标准

- `node tools/metrics.mjs --dir tools/fixtures/dispatches` 打印四类判断（样本、未裁决、打回率、越界、追溯）
- `--json` 输出可被 `JSON.parse` 且字段齐全
- 对真实目录运行时不报错（记录为 0 或 1 条时给出"样本不足"而不是崩溃）

## 不变量

1. 聚合器只读记录，不修改、不删除
2. 指标缺失时显示 `—`，不显示 0（0 和"没有数据"是两件事）
3. 每条判断必须指向一个可执行动作

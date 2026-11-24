# Agent 派发 · 行为规格

- 能力 ID：`002-agent-dispatch`
- 状态：approved
- 关联：`docs/team.md`、`docs/constitution.md` 第 4 条（AI 的职责边界）

## 背景

W3 解决了"怎么裁决"，W4 要解决"怎么派活"。

派活如果靠人口述，会出现三个问题：同一任务每次说法不同、约束被漏掉、
事后无法回答"当时到底告诉了它什么"。所以派发必须是**编译**出来的，
不是**手写**的。

## 范围

**做**：提示词编译器（预设 + 任务 + 场景 → 可复现文本）、派发记录、裁决回写。

**不做**：
- 不调用模型（编排由宿主的 subagent 承担）
- 不做运行时工具白名单（DSH 目前没有 preset 级 toolFilter，事前靠提示词约束）
- 不做多任务并行编排（并行由人决定，工具不做调度）

## 场景

### S-01 同一任务编译出同一份提示词

```gherkin
GIVEN 同一个 tasks.md 与任务 ID
WHEN 连续编译两次
THEN 两份提示词的 sha256 相同
AND 提示词中不含时间戳等易变内容
```

### S-02 contract 层不得派发给 Agent

```gherkin
GIVEN 任务的 layer 是 contract
WHEN 试图派发
THEN 拒绝并说明"契约变更只能由人走 changes/ 提案"
AND 退出码为 3
```

### S-03 派发记录可审计

```gherkin
GIVEN 一次带 --record 的派发
WHEN 查看记录
THEN 能看到任务 ID、预设、被引用的场景、允许改动的文件、验收命令、基线 commit
AND 产出的提示词有 sha256 便于核对
```

### S-04 裁决结果回写记录

```gherkin
GIVEN 一次带 --adjudicate 的派发
WHEN 执行器的裁决完成
THEN 记录状态更新为 PASS / FAIL / OUT_OF_SCOPE
AND 记录中包含 exit code、耗时、改动文件、越界文件
```

## 验收标准

- 两次 `--emit json` 的 `prompt_sha256` 相同
- `contract` 任务派发退出码为 3
- `--record` 产出的 JSON 通过 `check-ai-rules` 的记录结构校验
- `--adjudicate` 后记录状态与 `run-task.mjs` 的判定一致

## 不变量

1. 派发器不调用模型、不写业务代码
2. 契约变更永远不进 Agent 的任务队列
3. 记录只增不改（裁决回写是唯一例外）

# changes/ · 契约变更提案

AI 发现契约缺失或矛盾时，**不能自己改 `contracts/`**，只能在这里写提案。

## 流程

1. 新建 `changes/NNN-短横线标题.md`，编号递增。
2. 写清：现状、为什么契约不够、建议改成什么、影响哪些实现与测试。
3. 人工评审。批准后由**负责人**修改 `contracts/` 与 `fields.json`。
4. 实现跟进；`npm run gate` 在实现跟上之前应当红——这是预期的。

## 模板

```markdown
# 001 为交易提交补充幂等键字段

## 现状
`transactions.json` 只有交易码，没有幂等键策略，P1 无法实现"重放不重复入账"。

## 建议
新增 `idempotency: "client-uuid" | "server"` 字段，默认 `client-uuid`。

## 影响
- contracts/menu-biz-schema.md：新增字段
- crates/tellerkit-core：交易元数据结构
- src/contracts.ts：同步类型
- P1 交易引擎：发起时生成 UUID

## 风险
服务端若不支持幂等键，重放仍会重复入账——需要服务端同步改造。
```

## 不要做的事

- 不要为了实现方便去改契约（宪法第 1 条）。
- 不要在提案里夹带实现代码。
- 不要跳过评审直接改 `fields.json`。

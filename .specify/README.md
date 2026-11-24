# .specify/ · 规格层

工作台的输入。**AI 可以读、可以起草，但不能修改已有规格的验收标准**（宪法第 4 条）。

## 八步流程里，规格只属于第 1–2 步

```
1 写 spec          人
2 评审 spec/plan   人
3 分派任务         工作台
4 生成产物         AI
5 门禁            工具
6 人工评审         人
7 合并             人
8 索引快照         工作台
```

AI 只在 3–5 步工作。

## 目录

```
.specify/
  templates/spec.md      行为规格模板（GIVEN/WHEN/THEN）
  templates/tasks.md     任务模板（每项必须引用场景 ID）
  specs/<NNN-capability>/
    spec.md
    tasks.md
```

## 规则

- 每个 scenario 有唯一 ID（`S-01`），`tasks.md` 的每一项必须引用至少一个场景 ID。
- 任务必须声明 **改的是抽象还是实现**（`layer: contract | core | shell | ui`）。
- 规格变更走 `changes/` 提案，不在原地改验收标准。

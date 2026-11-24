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

## 执行

```bash
node tools/check-tasks.mjs                                  # 结构 + 覆盖率
node tools/run-task.mjs --tasks <tasks.md> --id T-01        # 单任务裁决
node tools/run-task.mjs --tasks <tasks.md> --all            # 回归（白名单需覆盖整分支）
```

`run-task` 只认两件事：验收命令的真实 exit code、以及改动是否落在「允许改动」内。
判定 `OUT_OF_SCOPE` 优先于 `FAIL`——命令跑绿了但改错地方，仍然是失败。

## 规则

- 每个 scenario 有唯一 ID（`S-01`），`tasks.md` 的每一项必须引用至少一个场景 ID。
- 任务必须声明 **改的是抽象还是实现**（`layer: contract | core | shell | ui`）。
- 规格变更走 `changes/` 提案，不在原地改验收标准。

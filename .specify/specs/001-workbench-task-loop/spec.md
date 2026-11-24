# 工作台任务闭环 · 行为规格

- 能力 ID：`001-workbench-task-loop`
- 状态：approved
- 关联契约：`.specify/templates/tasks.md`、`docs/constitution.md` 第 5 条

## 背景

R15 的核心机制是"确定性裁决"：AI 生成产物后，**由工具亲自执行验收命令**，
拿真实 exit code，再检查改动是否越出白名单。不接受任何自报的"已完成"。

不做会怎样：AI 会反复自我宣称通过，而真实行为无人验证；
越界修改（改了契约、动了别的模块）也不会被发现。

## 范围

**做**：任务清单的机器可读格式、执行器三态判定、清单结构与覆盖率校验。

**不做**：
- 不做多 Agent 编排（W4）
- 不做轨迹级门禁（W5）
- 不做自动重试（宪法第 5 条明确禁止）

## 场景

### S-01 验收通过

```gherkin
GIVEN 一个任务声明了验收命令与允许改动范围
AND 命令退出码为 0
AND 本次改动全部落在允许范围内
WHEN 执行器运行该任务
THEN 判定 PASS
AND 退出码为 0
```

### S-02 越界即失败

```gherkin
GIVEN 任务只允许改 packages/core/src/*.rs
AND 实际改动包含了 apps/shell/src/App.vue
WHEN 执行器运行该任务
THEN 判定 OUT_OF_SCOPE
AND 退出码为 2
AND 即使验收命令本身退出 0 也不得判为 PASS
```

### S-03 命令失败不被掩盖

```gherkin
GIVEN 验收命令以非零码退出
WHEN 执行器运行该任务
THEN 判定 FAIL
AND 退出码为 1
AND 输出中回显 stderr 的尾部内容，便于定位
```

### S-04 清单结构可校验

```gherkin
GIVEN 某个 tasks.md 缺少「验收命令」列，或引用了不存在的场景，或 spec 中有场景无人覆盖
WHEN 运行清单校验
THEN 退出码非 0
AND 逐条指出问题所在文件与修复方向
```

## 验收标准

- `node tools/check-tasks.mjs` 对当前仓库返回 0
- `node tools/run-task.mjs` 在三种构造场景下分别返回 0 / 1 / 2
- `npm run gate` 包含清单校验

## 不变量

1. 判定只依据真实 exit code 与真实改动，不依据任何自报字段
2. 失败不自动重试
3. 执行器本身不修改被检查的文件

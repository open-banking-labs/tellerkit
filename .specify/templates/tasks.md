# <能力名> · 任务

> 机器可读格式。`tools/check-tasks.mjs` 校验结构，`tools/run-task.mjs` 执行验收命令。
> 规则：单元格内不得出现 `|`（会破坏表格解析）；多路径用逗号分隔；验收命令只能有一条。

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | <一句话描述> | S-01 | `core` | `packages/core/src/*.rs` | `cargo test -p tellerkit-core` |

## 字段说明

| 列 | 取值 | 说明 |
|---|---|---|
| `ID` | `T-NN` | 在同一文件内唯一 |
| `任务` | 文本 | 一句话，动词开头 |
| `场景` | `S-NN`, `S-NN` | 必须存在于同级 `spec.md` |
| `layer` | `contract` \| `core` \| `shell` \| `ui` \| `docs` \| `tools` | `contract` 表示只改契约，**AI 不得执行**，必须人做 |
| `允许改动` | glob 列表 | `*` 不跨目录，`**` 跨目录；改出白名单的文件即判越界 |
| `验收命令` | 单条 shell 命令 | 由执行器亲自运行，exit code 为准 |

## 执行

```bash
node tools/run-task.mjs --tasks .specify/specs/<id>/tasks.md --id T-01
node tools/run-task.mjs --tasks .specify/specs/<id>/tasks.md --all
```

结果三态：`PASS` / `FAIL`（命令非零退出）/ `OUT_OF_SCOPE`（改了白名单外的文件）。
后两者都是失败，且**不自动重试**——交人工判断。

**执行时机**：一个任务一次，`--base` 指向该任务开始前的 commit。
执行器比对的是 `base..当前工作区` 的改动；一次跑多个任务时白名单会互相污染，
所以 `--all` 只适合回归验证（此时白名单应覆盖整个分支）。

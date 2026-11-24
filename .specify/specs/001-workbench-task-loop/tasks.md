# 工作台任务闭环 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现任务执行器与三态判定 | S-01, S-02, S-03 | `tools` | `tools/run-task.mjs`, `tools/lib/glob.mjs` | `node tools/run-task.mjs --help` |
| T-02 | 实现清单结构与覆盖率校验 | S-04 | `tools` | `tools/check-tasks.mjs` | `node tools/check-tasks.mjs` |
| T-03 | 迁移既有任务清单到新格式 | S-04 | `docs` | `.specify/**` | `node tools/check-tasks.mjs` |
| T-04 | 接入门禁与文档 | S-04 | `docs` | `package.json`, `AGENTS.md`, `docs/**` | `npm run gate` |

## 完成定义

- 四个任务的验收命令全部 PASS（以执行器的真实 exit code 为准）
- `npm run gate` 绿

## 备注

T-01 的验收命令用 `--help` 而不是"跑一遍全部用例"：执行器要能被执行器验收，
但**不能自己验自己**——真正的三态验证放在 /tmp 的构造场景里，属于人工验收步骤。

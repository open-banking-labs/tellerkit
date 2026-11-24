# Agent 派发 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现提示词编译器与拒绝规则 | S-01, S-02 | `tools` | `tools/dispatch.mjs`, `tools/lib/tasks.mjs` | `node tools/dispatch.mjs --help` |
| T-02 | 实现派发记录与裁决回写 | S-03, S-04 | `tools` | `tools/dispatch.mjs`, `.agents/dispatches/*` | `node tools/dispatch.mjs --tasks .specify/specs/001-workbench-task-loop/tasks.md --id T-02 --emit json` |
| T-03 | 记录结构纳入门禁 | S-03 | `tools` | `tools/check-ai-rules.mjs` | `npm run gate` |
| T-04 | 文档与命令接入 | S-03 | `docs` | `AGENTS.md`, `docs/**`, `package.json` | `npm run gate` |

## 完成定义

- 四个任务的验收命令在干净树上全部 PASS
- `npm run gate` 绿
- 至少有一条真实派发记录（`.agents/dispatches/*.json`）落库

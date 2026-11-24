# 可观测 · 任务

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 实现指标聚合与判断 | S-01, S-02 | `tools` | `tools/metrics.mjs` | `node tools/metrics.mjs --dir tools/fixtures/dispatches` |
| T-02 | 实现记录追溯检查 | S-03 | `tools` | `tools/metrics.mjs`, `tools/fixtures/dispatches/*` | `node tools/metrics.mjs --dir tools/fixtures/dispatches --json` |
| T-03 | 定义指标含义与动作 | S-01, S-04 | `docs` | `docs/metrics.md`, `package.json` | `npm run gate` |
| T-04 | 接入 CI 与文档索引 | S-04 | `docs` | `.github/workflows/gate.yml`, `README.md`, `AGENTS.md` | `npm run gate` |

## 完成定义

- 四个任务的验收命令在干净树上 PASS
- `npm run gate` 绿
- `docs/metrics.md` 里每个指标都写明"变了该做什么"

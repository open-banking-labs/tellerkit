# dispatches/ · 派发记录

每次 `node tools/dispatch.mjs --record` 落一条 JSON，记录"当时到底告诉了 Agent 什么"。

## 为什么留在仓库里

- **可复核**：提示词有 sha256，能确认事后没被改过
- **可追溯**：出问题时能回答"它被允许改哪些文件、验收标准是什么、基线是哪个 commit"
- **可度量**（W5）：统计打回率、越界率，判断是提示词不好还是任务拆得不好

## 字段

| 字段 | 含义 |
|---|---|
| `task_id` / `tasks_file` / `spec` | 任务出处 |
| `preset` / `layer` | 用哪个角色、哪个层次 |
| `scenarios` | 被引用的验收场景 |
| `allowed_files` | 白名单（裁决依据） |
| `acceptance_cmd` | 验收命令（由工具执行，不由 Agent 执行） |
| `base` / `base_sha` | 裁决基线 |
| `prompt_sha256` | 提示词指纹 |
| `status` | `dispatched` → `PASS` / `FAIL` / `OUT_OF_SCOPE` |
| `verdict` | exit code、耗时、改动文件、越界文件 |

结构由 `check-ai-rules` 校验；记录**只增不改**，裁决回写是唯一例外。

## 已作废的记录

任务/预设演进后，旧记录的提示词指纹会与当前复算结果不符。此时不要手改记录，
而是在 `.agents/dispatch-superseded.json` 里登记「文件名 → 作废原因」。
`check-trajectory` 会点名跳过并统计作废条数，不会把它算进"可复算"。

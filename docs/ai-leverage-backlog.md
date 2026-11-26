# AI 利用待办（backlog）

- 来源：2026-11-25 的一次分析（"以善于利用 AI 的专家角度，这个项目还能怎么充分利用 AI"）
- 性质：**草案，不是规格**。本文不新增场景、不新增验收标准（宪法第 4 条），采纳前需人确认
- 用法：其他会话照着做时，一次只做一行；每行都自带「证据 / 改哪个文件 / 怎么判绿 / 停手条件」
- 状态：`pending` · `doing` · `blocked` · `done` · `rejected`

## 状态总览

| ID | 事项 | 优先级 | 量级 | 状态 |
|---|---|---|---|---|
| AI-01 | 派发记录补齐 attempt / model / usage / cost | P0 | 小 | pending |
| AI-02 | "已完成任务必须有匹配 PASS 记录"的证据门禁 | P0 | 小 | pending |
| AI-03 | 决策清单（已否决 / 已被取代）+ 派发注入 | P0 | 中 | pending |
| AI-04 | 确定性 context pack + 上下文预算 | P0 | 小 | pending |
| AI-05 | 场景 ↔ 测试双向追溯检查 | P1 | 中 | pending |
| AI-06 | 变异测试基线（4 个关键模块） | P1 | 中 | pending |
| AI-07 | 评审输出 JSON + 跨模型 + 红线清单 | P1 | 小 | pending |
| AI-08 | 规格候选场景生成（人审后写入） | P2 | 中 | pending |
| AI-09 | seam 自动生成 + 拓扑集成 | P2 | 大 | pending |
| AI-10 | 端侧 AI 预留改成"可验证的空槽"（changes 提案） | P2 | 中 | pending |
| AI-11 | 柜面 AI 的离线 eval 骨架 | P2 | 中 | pending |

## 证据快照（2026-11-25，别重测，除非状态变了）

- 规模 `npm run stats`：能力 15、任务 55、场景 84、契约结构体 16、生成 TS 类型 17、IPC 命令 23。
- 派发 `npm run metrics`：记录 3 条（全已裁决），**覆盖率 3/55 = 5.5%**，PASS 3 / FAIL 0 / OUT_OF_SCOPE 0，耗时中位 390ms。
- 代码 `docs/code-review-rust.md`：21 个 .rs、8385 行（生产 4420 / 测试 3965），clippy 5 条警告；毒锁 `into_inner()` 4 处：`packages/journal-sqlite/src/lib.rs:86`、`packages/core/src/device.rs:335`、`packages/core/src/tx.rs:176`、`packages/core/src/tx.rs:180`。
- 上生产阻断：8 项（`docs/production-readiness.md` B1–B8），其中 5 项不是代码问题。
- 已知口径冲突：`docs/constitution.md` 第 7 条仍写"不做 iframe 沙箱"，而 `changes/012` 与 README 已采用 iframe 沙箱——AI 只读宪法会被误导（这正是 AI-03 的由来）。

## 已知障碍（开工前先知道，别当成自己的锅）

- 2026-11-25 实测：`tools/check-ai-rules.mjs` 会**扫描 `.agents/worktrees/` 下的旧 worktree 拷贝**，而那份拷贝里含有 `dbg!(` 正则字面量，于是误报 `残留 dbg!()`，`npm run gate` 因此非 0。这是门禁自身的盲区（walkAll 未排除 `.agents/`），与本次文档无关；修法：给 `walkAll` 加排除，或清理旧 worktree。属于 AI-02 之外的独立小修，谁遇到谁提 `chore/` 分支。
- `docs/reviews/` 目前未纳入版本管理（`git status` 里是 untracked）；AI-07 落地前先决定它是否入库。

## 全局约束（每个会话开工前读一遍）

1. 只读区不得改：`contracts/**`、`docs/adr/**`、`docs/constitution.md`、`.agents/presets/**`、`changes/**`（只能新增提案）。
2. 不在本机跑 cargo；编译走 `scripts/remote-build.sh`。本地只跑 `npm run gate` 的 node 部分。
3. 门禁结论只认 `npm run gate` 的 exit code；失败不自动重试——一次失败、一次分析、一次人工决定。
4. 一次任务一个文件 / 一个 trait / 一个页面；单次新增 >300 行退回重拆。
5. 不得为实现改契约；不得跨层 import；前端不判权限。
6. 别把指标当 KPI 刷（`docs/metrics.md` 的反模式一节）。

---

## P0

### AI-01 派发记录补齐 attempt / model / usage / cost

- **证据**：`docs/metrics.md` 自己列了三个缺口（无 token/成本、无重跑次数、无趋势）；现在只有 3 条记录，改字段最便宜。
- **改哪里**：`tools/dispatch.mjs`（记录写入处）、`tools/check-ai-rules.mjs`（必填字段校验，**新增字段要向后兼容**：旧记录缺新字段不判红）、`tools/metrics.mjs`（输出总花费与单 PASS 花费）。
- **怎么判绿**：`npm run metrics` 能跑通并打印新指标；新派发记录含四个字段；3 条旧记录仍可解析。
- **停手条件**：宿主拿不到用量数据时，写 `usage: null` + note，**不许编造数字**；这条不必等宿主。
- **备注**：`attempt` 在 `run-task` 重派时自增；不要为了实现它改成自动重试。

### AI-02 "已完成任务必须有匹配 PASS 记录"的证据门禁

- **证据**：覆盖率 5.5%，等于工作台的观测层没有原料。
- **改哪里**：新增 `tools/check-dispatches.mjs`（或并入 `tools/check-trajectory.mjs`），加进 `package.json` 的 `gate`。
- **规则**：`tasks.md` 中标记为完成的任务，必须在 `.agents/dispatches/` 找到 `status=PASS` 且 `inputs{tasks_sha256,spec_sha256,preset_sha256}` 与当前文件一致的记录。**这是证据检查，不是比例阈值**。
- **怎么判绿**：故意删一条记录 → 退出码非 0；仓库现状全绿。
- **停手条件**：现有 55 个任务大多无记录，首版只对「spec 状态 implemented 的能力」或带完成标记的任务生效，否则 gate 立刻红——先小范围，别一次性全量。
- **备注**：不要引入阈值（如"覆盖率 ≥60%"），那会变成可刷的 KPI。

### AI-03 决策清单（已否决 / 已被取代）

- **证据**：宪法第 7 条 vs `changes/012` 的口径冲突；`docs/PLAN.md` 的"砍掉/延后"清单只存在于散文里，新 Agent 会重提 GPUI 全量、按角色分包、字段级权限、dylib 插件。
- **改哪里**：`docs/decisions.json`（schema：`id / title / status(accepted|superseded|rejected) / supersedes / scope / reason / refs`）；新增 `tools/check-decisions.mjs`（id 唯一、status 枚举、supersedes 指向存在、`rejected` 必须有 `reason`）；`tools/dispatch.mjs` 注入"不要重提以下已否决方案"摘要。
- **怎么判绿**：`node tools/check-decisions.mjs` 退出 0；`dispatch --print` 的提示词里能看到决策摘要。
- **停手条件**：宪法第 7 条需要人改（走 `changes/`），AI 只提案；`docs/adr/**` 只读，别直接改 ADR 正文。
- **备注**：先录 5 条最有价值的（iframe、GPUI、按角色分包、字段级权限、dylib 插件），别追求全。

### AI-04 确定性 context pack + 上下文预算

- **证据**：读什么全靠 AGENTS.md 逐级下发串起来，没有预算也没有度量。
- **改哪里**：新增 `tools/context-pack.mjs`：输入 `--tasks <tasks.md> --id T-xx`，输出"最近的 AGENTS.md + 相关契约条目 + 引用到的场景 + 同目录先例 + 同类历史 FAIL"文件清单与估算字节；`tools/dispatch.mjs` 记录 `context_bytes`。
- **怎么判绿**：对 `.specify/specs/030-reversal-authorization/tasks.md` 的 T-01，输出 ≤10 个文件且必含该任务的契约条目与 S-01..S-07；同一输入两次输出完全一致（确定性）。
- **停手条件**：**不要引入向量库/embedding**（宪法第 7 条减法优先；15 份 spec、84 个场景的规模下 grep + 显式引用更准更可复现）。spec 过百再评估。
- **备注**：零新依赖，只用 node 内置模块。

---

## P1

### AI-05 场景 ↔ 测试双向追溯检查

- **证据**：84 个场景、`test-agent.md` 约定用例名带 `s01_...`，但**没有检查器**；"每场景至少一个用例"只是完成定义里的一句话。
- **改哪里**：新增 `tools/check-scenarios.mjs`：从 `.specify/specs/*/spec.md` 抽 `### S-xx`，从 `packages/**`、`apps/shell/src-tauri/**` 抽 `sNN_` / `S-NN` 引用；双向报孤儿（场景无测试、测试无场景）。
- **怎么判绿**：临时删一个用例 → 红；新增场景未引用 → 红；现有 84 场景清零后转 error。
- **停手条件**：先 warn 模式跑一轮，把现有缺口列出来再定是否阻断；不要为了清零去改断言。

### AI-06 变异测试基线（4 个关键模块）

- **证据**：门禁只证明"命令绿了"，不证明"测试在测东西"；AI 同时写实现和测试，存在同源偏差。
- **改哪里**：CI 夜间 job（不在本机跑 cargo；Windows runner 或 `scripts/remote-build.sh` 的机器），`cargo mutants` 限定：权限剪枝（`packages/core/src/filter.rs` 或等价）、冲正（`reversal.rs`）、重放（`replay.rs`）、更新（`update.rs`）；分数作为 artifact 与基线记录，**不进门禁**（太慢）。
- **怎么判绿**：首跑产出基线数字并写进 `docs/metrics.md`；对 `reversal.rs` 至少能发现一条现有测试未覆盖的分支。
- **停手条件**：若变异跑一轮 >30 分钟，先缩到两个文件，别硬上。

### AI-07 评审输出 JSON + 跨模型 + 红线清单

- **证据**：`review-agent.md` 是散文四段，不可判定；作者与评审同模型则同源偏差。
- **改哪里**：`.agents/presets/review-agent.md` **是只读区**——AI 只能出草案到 `changes/`，由人改预设。工具侧新增 `tools/check-review.mjs` 校验 JSON schema（`verdict / basis / risks[] / decisions[] / findings[{severity,file,line,rule_id}]`）。
- **怎么判绿**：`node tools/check-review.mjs docs/reviews/<x>.json` 通过；在一个真实 PR 上试跑一次，人工确认无误报。
- **停手条件**：评审 Agent 不得重跑门禁换结果（沿用现有规则）；跨模型评审若成本不可接受，至少做到"同一 PR 由未参与实现的会话评审"。

---

## P2

### AI-08 规格候选场景生成（人审后写入）

- **证据**：团队文档明说瓶颈是规格质量；宪法允许 AI 起草、不允许新增场景。
- **改哪里**：新增 `tools/spec-candidates.mjs`（或直接在会话里跑，产物落 `docs/reviews/`）：输入一份 spec，输出"可能缺失的场景"候选（Gherkin 草稿 + 为什么可能缺 + 对应哪条不变量）；人审后由**人**写入 spec。
- **怎么判绿**：对 `030-reversal-authorization` 产出 ≥3 条候选，其中 ≥1 条被人采纳；每份 spec 留一节"AI 候选但未采纳 + 理由"。
- **停手条件**：AI 不得直接改 spec 的场景与验收标准；不得改已有 S-xx 的 ID 或语义。

### AI-09 seam 自动生成 + 拓扑集成

- **证据**：`docs/parallel-work.md` 的 seam 先行靠手写；`tools/plan-parallel.mjs` 已有冲突检测。
- **改哪里**：新增 `tools/seam.mjs`：从 `contracts/fields.json` + 任务 allowed 文件生成签名 stub（含文档注释与 `unimplemented!()`）；人工调整后提交；worktree 批量派发，按拓扑合并。
- **怎么判绿**：生成的 stub 在构建机 `cargo check` 通过；两个并行任务的签名冲突能被 `plan-parallel` 报出。
- **停手条件**：AI-05 / AI-06 未完成前不要做——验证不强时并行只是放大器。

### AI-10 端侧 AI 预留改成"可验证的空槽"（changes 提案）

- **证据**：`docs/PLAN.md` §11.6 故意只写规则不写字段，因为"现在加空字段会立刻被门禁当成契约与实现不一致"；而门禁的 `AI_TOKENS` 拉黑让"加字段"更难。
- **改哪里**：新增 `changes/` 提案：①契约登记 `assist.suggestTx/prefill/riskHint` 为 unimplemented（contract 层本来就被派发器拒派，只能人改）；②审计事件枚举加 `ai_suggestion/ai_prefill/ai_risk_hint`；③`aiAssistCapable` 字段；④新门禁 `check-assist`：槽存在 + **无任何实现** + 事件已登记；⑤威胁模型：数据出境、业务包内容注入提示词、建议溯源、模型离线降级。
- **怎么判绿**：提案被人批；`check-assist` 绿；前端仍无任何模型调用。
- **停手条件**：宪法第 10 条不能由 AI 改；AI 只能写提案，不能动手实现。

### AI-11 柜面 AI 的离线 eval 骨架

- **证据**：仓库已有确定性假银行 + 84 个场景 + 可重放账本，这正好是柜面 AI 助手的离线评测环境——现在搭比将来做 UI 便宜一个数量级。
- **改哪里**：新增 `tools/eval-assist.mjs`（`--dry-run`，不打模型：用固定假建议跑评分管道）+ `fixtures/assist/*.json`；从 84 场景里挑可判定"建议好坏"的做评分表（如差错文案是否可懂、预填字段是否正确、风险提示是否命中）。
- **怎么判绿**：`node tools/eval-assist.mjs --dry-run` 输出评分表，无网络调用；换一版假建议分数会变（证明 eval 有效）。
- **停手条件**：这是评测基础设施，不是端侧实现；不得借此在第 10 条下偷偷接模型。

---

## 明确不做（rejected，别再提）

| 想法 | 为什么不做 |
|---|---|
| "AI 自动改到 gate 绿"的循环 | 优化目标是绿，不是对；违反宪法第 5 条"失败不自动重试" |
| AI 自评自批 | 同源偏差；现在 3 条样本 0 打回，不构成质量证据 |
| 打回率 / 越界率当 KPI | `docs/metrics.md` 已列为反模式 |
| 现在上向量库 / RAG 基础设施 | 规模不匹配（15 spec / 84 场景），增加不确定性，违反减法优先 |
| 先上并行编排 | 验证强度不够时并行只是生产更多未验证代码 |
| 在第 10 条下私自做端侧助手 | 门禁会红，且会毁掉"契约先行"的信誉 |

## 怎么把一行变成可派发的任务

1. **先判断性质**：契约 / 核心逻辑走 `layer: core` 并进 `.specify/specs/<id>/tasks.md`；纯工具与文档不属于四层，**不要伪造 layer**，按 `chore/…` 分支 + `npm run gate` 验收即可。
2. 任务行格式（与 `030-reversal-authorization/tasks.md` 一致）：`| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |`。
3. 场景列：工具类任务没有对应 S-xx，**必须由人先在 spec 里补场景**，否则 `check-tasks` 不认。
4. 一条任务一个文件；预计 >300 行就先拆。
5. 做完用 `npm run dispatch -- --tasks … --id … --record` 留痕，再用 `npm run task -- --tasks … --id …` 裁决——这样 AI-01 / AI-02 的指标才有原料。

## 验收总原则

- 每条都以**可执行命令的 exit code** 或**人能按步骤复现的断言**收尾，不写"体验良好"。
- 指标只用于观察，不用于卡人（`docs/metrics.md`）。
- 任何一条如果做起来要改契约或改宪法，停下来写 `changes/` 提案。

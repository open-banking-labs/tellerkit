# 项目级整改总台账（2026-11-25）

> 基线 `155e0fa`。本文件不替代各专项台账，只做三件事：**去重、定级、排序**。
> 细项的证据、行号、验收命令仍在源文件；这里回答“先做什么、为什么、做到什么算完”。

## 1. 来源与口径

| 来源 | 覆盖 | 细项 |
|---|---|---|
| [规模评审](reviews/scale-readiness-review-2026-11-25.md) | 运行时、状态、合规、规模化 | P0-01…11 / P1 / P2 |
| [安全评审](reviews/2026-11-25-security-review.md) | 生产不成立的五个控制点 | P0-1…4 / P1-5…9 |
| [界面评审](reviews/ui-ux-review-2026-11.md) | 主壳产品形态 | P0-1…5 / P1-1…9 |
| [架构评审待办](reviews/architecture-review-backlog.md) | 29 条端到端缺口 | A / B / C / D |
| [安全加固规格](../.specify/specs/040-security-hardening/tasks.md) | T-01…T-20 可执行任务 | 启动链 / 命令链 / 账本链 |
| [UI/UX 待办](ui-ux-backlog.md) · [AI 杠杆待办](ai-leverage-backlog.md) | 19 条 UX · 11 条 AI | 单项验收 |

**去重示例**：架构 A-01 ≈ 安全 P0-1 ≈ 规模 P0-06/P0-09 的一部分；A-02 ≈ 安全 P0-3；A-03 ≈ 规模 P0-01；A-04 ≈ 规模 P0-02；B-07 ≈ 安全 P1-6 ≈ 规模 P0-10。合并后按“一个可交付的控制点”立项，不再按评审来源分头排期。

## 2. 排序原则

“项目级”指满足任一条件：**改变信任边界**、**决定资金状态是否可恢复**、**决定产物能否交付**、**决定故障能否定位**。单项体验、局部重构、工具打磨不占项目级名额。

顺序是：**真相 → 信任 → 资金 → 交付 → 规模 → 体验**。

- 真相先于一切：门禁如果本身在说谎，后面所有“已验证”都不可信。
- 信任先于功能：会话可伪造时，界面越顺手风险越大。
- 资金先于交付：装不出去只是晚一点，账对不上是事故。
- 体验放最后不是不重要，而是它可以在门做真之后并行推进。

## 3. 项目级清单

| ID | 项目级控制点 | 覆盖细项 | 完成的硬证据 | 批次 |
|---|---|---|---|---|
| P0-1 | 命名与提交门禁 | 本轮新增 | `npm run gate` 含银行名检查；pre-commit 同规则；全仓 0 命中 | ✅ 已完成 |
| P0-2 | 裁决链可信 | C-01 C-02 C-05 | 故意让一条测试红，CI 必红；`--locked` 构建通过；门禁负向用例可复算 | 1（C-02/C-05 完成，C-01 待提案批准） |
| P0-3 | 文档与事实一致 | C-07 D-02 D-05 D-06 | README 不再声称未实现的能力；伪门禁删除或接真数据；版本单一来源 | ✅ 已完成（runbook D-06 留待批次 3 交付链） |
| P0-4 | 信任根：生产公钥 | A-01 安全 P0-1 | release 缺 `TK_MENU_PUBKEY_HEX` 构建失败（已验证）；启动摘要显示实际公钥指纹；演示公钥不进发布产物（P0-5 后 release 不再回退 mock） | ✅ 已完成 |
| P0-5 | 启动/配置 fail-closed | A-06 安全 P0-2 规模 P0-09 | release 缺端点/配置非法/未签名/明文端点一律拒绝启动并给可读错误页；非法开关值不静默当作关；debug 才回退演示远端且必须带 note | ✅ 已完成 |
| P0-6 | 会话与锁屏真门 | A-02 安全 P0-3 | `ShellApp::require_unlocked` 只信 Rust 侧会话状态；锁定/未登录时资金/打印/冲正/重放/业务包入口一律 Err 且不触达主机，拒绝写审计；后台重放同门；负向用例 5 命令全拒 + 解锁放行对照 | ✅ 已完成（Rust 闸门；前端卸载包宿主留 UX-08 批次） |
| P0-7 | 业务包能力执行 | 安全 P0-4 | 未声明的交易码被拒绝；包身份进审计；能力白名单按包而非方法名 | ✅ 已完成（能力执行；调用来源不可伪造需契约字段，见 changes/017） |
| P0-8 | 写前意图与恢复 | A-04 | 先记 intent 再发远端；崩溃后 `in_flight` 转异常；两条自动提交路径都有事务 | ✅ 已完成（结论单事务收口；P0-9 的幂等落盘另计） |
| P0-9 | 幂等、待冲正、重放落盘 | 规模 P0-02/P0-03 安全 P1-8 B-03 B-02 | 重启后同键拒绝异内容；待冲正清单不丢；重放次数不重置；冲正带原交易要素 | ✅ 已完成（幂等索引/异常/冲正记录/重放 attempts/原交易要素全部落盘并启动读回；PR #42） |
| P0-10 | IPC 并发 | A-03 规模 P0-01 | 网络调用不持全局锁；锁屏 tick 不被交易阻塞；单飞防重复提交 | 2 |
| P0-11 | 审计不可抵赖 | B-07 安全 P1-6 规模 P0-10 | 哈希链 + 单调序号；审计失败阻断资金操作；安全事件必审计 | ✅ 已完成（哈希链+篡改校验 PR #38；写前闸门+结论审计可失败+登录失败/包加载必留痕 PR #43；外部锚定/服务端存证与配置变更审计留批次 4） |
| P1-1 | 交付链 | A-05 | 正式 identifier、可安装包、签名、更新器、SBOM、release 流程 | 3 |
| P1-2 | 单实例与账本排他 | A-07 规模 P0-11 | 第二个实例被拒；同一账本不能被两个进程写 | 3 |
| P1-3 | 可观测性与灰度 | B-08 规模 P1 | tracing/轮转/诊断包；灰度急停与版本差分可用 | 3 |
| P1-4 | 账本加密与国密 | B-06 规模 P0-07 | 密钥不进同目录；SM2/SM3/SM4 与国密 TLS 可切换 | 4 |
| P1-5 | 主壳产品化 | UX-01…07 UX-09…13 B-09 | 两列工作台、任务优先、完成态冻结、真实会话区、无死按钮 | 4 |
| P1-6 | 可访问性与细节 | UX-08 UX-14…19 | 对比度/字号/触控区达标；锁屏保留上下文；动效可关 | 4 |
| P2-1 | 工程质量与坏味道 | C-03 C-04 C-06 C-08 D-01 D-03 D-04 | 覆盖率进指标；契约门禁比类型；供应链检查；无用门禁清除 | 5 |
| P2-2 | AI 证据链 | AI-01…11 | 证据门禁复跑产物；decisions.json；场景轨迹进 CI | 5 |

## 4. 批次与依赖

### 批次 0（本轮）
P0-1 命名门禁 + 本台账 + 评审/待办入库。

### 批次 1：先把“可信”做出来（试点前）
`P0-2 → P0-3 → P0-4 → P0-5 → P0-6`

- P0-2 必须最先：门禁不含测试/编译却自称唯一裁决，是后面所有结论的根问题。
- P0-3 与 P0-2 同批：文档继续声称 N-1、覆盖率、唯一入口，会让整改再次失真。
- P0-4 与 P0-5 是一条链：公钥注入只管“信谁”，fail-closed 管“信不过时怎么办”。
- P0-6 收口：锁屏从展示层变成命令层前置条件。

### 批次 2：资金状态可恢复
`P0-7 → P0-8 → P0-9 → P0-10 → P0-11`

- P0-8 依赖 P0-9 的账本表结构，先定“写什么”，再定“怎么恢复”。
- P0-10 会改所有命令签名，放在状态模型稳定之后，避免返工。
- P0-11 最后做：审计要记录前面引入的 intent、冲正、包身份，否则链上缺字段。
- 排序偏离说明（维护规则要求写理由）：实际把 P0-11 拆成两段（PR #38 哈希链、PR #43 收尾）提前到 P0-10 之前完成。理由是两段与状态模型正交、文件集独立且可并行验证；P0-10 要改全部 23 条命令签名，留到最后统一做可避免与 P0-7/P0-8/P0-9 反复冲突返工。P0-11 收尾只依赖 P0-9 的 `RecoveryJournal` 接线，已满足。

### 批次 3：能交付、能运维
`P1-1 → P1-2 → P1-3`

### 批次 4：产品与合规并行
`P1-5 → P1-6`（界面线）与 `P1-4`（合规线）可并行，但都必须等批次 1 的会话门就位。

### 批次 5：规模化与智能
`P2-1 → P2-2`

## 5. 阻塞项（需要人或契约）

| 阻塞 | 影响的待办 | 需要什么 |
|---|---|---|
| B1 手动锁屏 | UX-08 P0-6 | `changes/` 提案：锁屏命令与会话语义 |
| B2 草稿恢复 | UX-08 P1-5 | 会话状态是否允许落盘的契约决定 |
| B3 授权队列 | P0-9 P1-5 | 后端队列接口；端侧只做入口 |
| B4/B5 业务日期与日界 | 对账、审计 | 后端 Clock/业务日期契约 |
| B6 打印历史 | UX-09 | 后端查询接口；没有就不做按钮 |
| B7 异常处置动作 | 对账面板 | 处置动作与字段的契约 |
| T-01 | P0-7 P0-9 | 包身份上下文、冲正原交易要素的契约提案 |

**硬规则**：接口不存在的能力不做按钮；契约缺失写 `changes/`，不先改实现。

## 6. 本轮执行记录

1. 具体银行名称/缩写清零：评审文件改名、正文改泛称、README 同步。
2. 新增 `tools/check-bank-names.mjs` + `tools/bank-names.json`，接入 `npm run gate` 与 `.githooks/pre-commit`；锁文件/生成物/哈希行不误伤。
3. 修复 `check-ai-rules.mjs` 扫描 `.agents/worktrees` 的误报（此前会把别的分支的调试残留算到本仓库）。
4. 建立本台账；把四份评审/待办纳入版本控制。
5. P0-4 注入完成：`lib.rs` 增加 `resolve_menu_verifying_key`，release 缺 `TK_MENU_PUBKEY_HEX` 时 `build.rs` 直接 panic；启动摘要报告实际公钥指纹。远端验收：无 key `cargo check --release` exit 101 + 预期文案；带 key 同一命令 exit 0；`cargo test --workspace --locked` 251 passed。
6. P0-2 锁文件先行：远端解析出的 21 个缺失依赖已固化，CI/verify/远端构建统一 `--locked`；本轮 `--locked` 全量测试通过。
7. P0-2 轨迹门禁转真：`check-trajectory` 不再对无法复算/指纹不符的记录放行；T-11 旧记录因预设更新已在 `.agents/dispatch-superseded.json` 显式作废（负向验证：去掉作废登记后 gate exit 1）。C-01 的宪法措辞矛盾写进 `changes/015-gate-adjudication.md`，等负责人批准。派发提示词/输入哈希统一 LF + 新增 `.gitattributes`，消除 Windows CRLF 导致的跨平台指纹漂移。
8. P0-5 启动 fail-closed：新增 `bootstrap.rs`（策略与判定纯函数），release 强制配置验签/TLS、缺端点/非法/未签名/明文一律拒绝进入业务界面；失败原因存入 `AppState.startup_fatal`，由唯一加锁入口 `lock()` 拦下全部 23 条命令与后台重放，前端错误页可读。debug 才回退演示远端且必须留 note。行为变更写进 `changes/016-fail-closed-startup.md`（提案）。远端验收：49 passed / 0 failed、clippy `-D warnings` 通过、`npm run gate` exit 0。
9. P0-6 会话真门：`ShellApp::require_unlocked` 统一判定 Rust 侧会话（未登录/锁定），命中即写审计并返回可读错误，绝不触达主机；资金/打印/冲正/重放/业务包五个入口全部接入；后台重放改走同一闸门，锁定时写 skip 审计。负向测试 5 命令全拒、未登录 3 命令全拒、解锁放行对照。远端验收：52 passed / 0 failed、`npm run gate` exit 0。前端卸载包宿主/键盘锁时屏蔽归 UX-08 批次。
10. P0-3 文档与事实一致：README 澄清 gate 是本地静态预检、合并以 CI required checks 为准；PLAN/team/security-boundary 同步裁决口径与真实路径；删掉 check-ai-rules 的标题/短语存在性检查（陈旧文档也能绿，D-02），check-boundaries 的 `boot_load` 等五处改为正则匹配任意模块路径，堵住别名 import 绕过；新增 `tools/check-version.mjs`（Cargo.toml 单源 + 三处副本校验，负向测试 exit 1）与 `CHANGELOG.md`。`npm run gate` exit 0。D-06 runbook 与 C-07 文档路径全量校验留待批次 3。
11. 并行化：P0-7 / P0-11 文件集互斥，分别在独立 worktree 并行实现；远端构建共用单一路径，必须串行验证（rsync --delete 会互相删文件，且远端仅余约 26G）。P0-7 初版靠 map 保留键传包身份，经审查指出 Rust 无法独立证明调用来源，改为明确信任边界（Tauri 主框架独占 IPC + iframe 无 same-origin + 闭包身份覆盖）并写入 changes/017；一等来源身份留契约提案。两项均远端 clippy + 全量测试通过。
12. P0-11 审计哈希链：`seq` 单调 + `prev_hash` + `hash`，链头单独存表以防尾部删行；域分隔长度前缀 SHA256；单事务推进；旧库 ALTER TABLE 迁移保留 legacy 行；`audit_verify()` 可检出字段篡改、删行、重排、链头不符。远端 27 passed / clippy 干净。417 行超阈值按「同一控制点不可拆」裁决接受。
13. P0-8 写前意图：`TxJournal` 增 `record_intent`/`settle`/`in_flight_intents`；提交前先落意图，失败即拒绝且不触主机；结论单事务写日志 + 清意图 + 入/出队；启动把遗留意图转 `MarkedForReversal` 提示查账，不自动重发。远端 176（core）+ 29（ledger）passed，clippy 干净。经验：`rsync -az` 保留 mtime，切 worktree 后须先 touch 源文件，否则 cargo 误判为最新。
14. P0-9 状态跨重启持久化：`TxJournal` 增 `load_idempotent`/`save_idempotent`，`cache_result` 统一三处写入；新建 `packages/core/src/recovery.rs`（`RecoveryJournal` 端口 + 异常种类稳定字符串 + 解析），`ReversalBook::restore` / `ReplayScheduler::restore/snapshot`；账本新增 5 张表（tx_idempotent/tx_exception/tx_reversal/replay_attempt/tx_request_elements，要素加密），`record_intent` 单事务同写原交易要素供冲正回捞；主壳启动读回异常/冲正/重放进度，冲正从账本取 `tx_code`/`amount_cents`（查不到即拒绝「请先查账」），重放后落盘 attempts。远端 clippy 干净、54+177+6+32+10 passed / 0 failed；本地 `npm run gate` exit 0。8 文件 662 行超 300 阈值，按「同一控制点不可拆（拆出中间态无法自洽）」裁决接受，PR 描述写明按三段评审。修过的坑：`tx_request` 返回 `Result<Option<TxRequest>, String>`，误写 `Ok(request) => (Some(request), None)` 造成 `Option<Option<TxRequest>>` 双重包装，clippy 报 `no field tx_code on type Option<TxRequest>`。
15. P0-11 收尾（S-09/S-15，PR #43，单文件 +168/-10）：`apps/shell/src-tauri/src/app.rs` 新增 `require_auditable()` 前置闸门——账本不可用或 `audit_write("attempt","写前留痕")` 失败即在**触达主机之前**拒绝；新增 `audit_write()`/`audit_checked()`，资金类操作结尾不再用吞错的 `audit()`，写失败返回「业务已发生，请立即查账，不要重复提交」。只 gate `submit_transaction` 与 `request_reversal`（S-09 GIVEN 原话）；`replay_pending` 由后台 worker 周期调用不能 gate，`print_voucher`/`package_module` 本期不 gate。闸门**不跑 `audit_verify()`**（全表 O(n) 扫描放热路径会退化成 O(n²)，架构评审 C 类已点名），链头一致性由 `journal.audit()` 写事务内读 `audit_chain_state` 保证。`login()` 包装 `try_login()`，未知角色/主机拒绝/菜单未就绪全部记审计，累计次数取自 `audit_tail` 而非进程内计数；`boot()` 包加载与跳过记 `package_load`；`audit=None` 不再静默。远端 clippy -D warnings + 全量测试 exit 0（57+177+6+32+10 passed），3 个新测试全绿；本地 `npm run gate` exit 0。范围外：配置变更审计（app.rs 无入口，需契约）、`audit_verify()` 接启动自检、打印/包模块/后台重放的闸门、外部锚定与服务端存证。

## 7. 维护方式

- 每完成一个项目级控制点，更新本表“批次”为 ✅，并在源台账勾掉细项；不新开平行台账。
- 新评审先归入现有控制点；确实新开的，先写“为什么它是项目级”，再排批次。
- 排序变化必须写理由；不允许因为“做起来顺手”插队。

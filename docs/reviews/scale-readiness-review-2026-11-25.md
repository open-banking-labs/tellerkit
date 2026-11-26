# 大规模生产评审 · 整改待办（2026-11-25）

> **评审基线**：`155e0fa`。仅基于当时代码与文档，未跑 `npm run gate`、未在本机编译。
> **结论**：作为架构原型 8/10，作为大型银行生产系统 3/10。差距集中在运行时模型、状态可靠性、合规准入、规模化运维四块。
> **本文用途**：整改台账。后续会话按 ID 逐项实施；每个 PR 只做一个 ID，PR 标题带 ID。
>
> 进场先读：根 [`AGENTS.md`](../../AGENTS.md) → [`docs/constitution.md`](../constitution.md) → [`docs/architecture.md`](../architecture.md) → [`docs/production-readiness.md`](../production-readiness.md) → 本文件。

---

## 0. 怎么用这份清单

1. **P0 全部关闭前，不进入试点。** 顺序按 ID 走；有依赖的 ID 在条目里写明。
2. 每个 ID 先读"现状/证据"，再动手。验收条件必须真实执行，**"测试通过"以 `npm run gate` 的 exit code 为准**。
3. 改契约（`contracts/**`）先写 `changes/` 提案，等人批准；本清单"契约"一栏标了是否需要。
4. 编译不在本机跑：`bash scripts/remote-build.sh`。本地只跑 gate 的 node 部分。
5. 遵守单文件/300 行纪律：一个 ID 需要跨多文件时，按"拆分"一栏拆成多个 PR，不一次性重构 `ShellApp`。
6. 新想法不在本清单里夹带，另开 ID 或在 `changes/` 提案里讨论。
7. 完成后在 §6 台账登记；未登记的项视为未完成。

**不在本文范围**：`contracts/**`、`docs/adr/**`、`docs/constitution.md`、`.agents/presets/**` 本身不能改；`changes/**` 只能新增提案。

---

## 1. 批次与出口条件

| 批次 | 时间 | 目标 | 包含 | 出口条件 |
|---|---|---|---|---|
| A 内功 | 2–3 周 | 单网点影子运行 | P0-01 ~ P0-11 | 断网、超时、重启、双开四类故障演练通过；gate 绿 |
| B 试点 | 1–3 月 | 30–50 个网点 | P1-01 ~ P1-05、P1-09 ~ P1-12、P1-15 | 一个分行连续运行 2 周，无 P0 级事故；审计可集中检索 |
| C 放量 | 3–6 月 | 一个一级分行 | P1-06 ~ P1-08、P1-13、P1-14、P1-16 | 国密/信创适配通过；IAM/授权联调；对账闭环 |
| D 全行 | 6–12 月 | 全行推广 | P2 全部 + 容量/灾备 | 终端资产大盘可用；灰度→全量→回滚演练通过 |

> P1 不要求全部在 B 批次做完；按依赖和风险挑。P0 没有优先级差别，全是阻断。

---

## 2. P0：试点前必须关闭

### P0-01 命令异步化：网络 I/O 移出全局锁

**现状**：`apps/shell/src-tauri/src/commands.rs:319` 的 `submit_transaction` 是同步 `pub fn`，持 `Mutex<AppState>` 走到主机；Tauri 同步命令默认在主线程执行。默认 3s 超时、按优先级切端点，最坏接近 10 秒界面冻结。每秒一次的 `sessionTick`（`apps/shell/src/App.vue:415`）也要同一把锁——交易卡住时锁屏判定一起停摆。锁屏本身只是 Vue 覆盖层，不是系统级锁屏；进程挂起/崩溃时屏幕上仍是客户数据。

**后果**：柜员以为死机 → 重复点击；离柜时屏幕不锁；营业网点柜台屏幕朝外，这是安全检查项。

**要做**：
1. `commands.rs` 的 23 个命令改 `async`（或 `#[tauri::command(async)]`）；阻塞调用放 `tauri::async_runtime::spawn_blocking`。
2. 锁只保护内存状态；`ShellApp` 的网络调用移到锁外；交易走独立 actor/队列，避免全局串行。
3. `App.vue`：提交 in-flight 时禁用按钮；锁屏改 OS 级（Windows `LockWorkStation`/安全桌面，macOS 锁屏），Vue 覆盖层只做提示。

**验收**：
- mock 主机 `sleep 5s` 时，界面仍可输入，`session_tick` 在 1s 内返回并触发锁屏。
- 代码审查：`Mutex<AppState>` 的作用域内没有 `remote.`/`transport` 调用。
- `npm run gate` 绿。

**拆分**：`commands.rs`（async 化）→ `lib.rs`+`app.rs`（锁粒度/actor）→ `App.vue`（in-flight/锁屏）。三个 PR。
**量级**：M。**依赖**：无。**契约**：不改 23 个命令的签名；新增命令另走 `changes/`。
**坑**：`std::sync::Mutex` 不能跨 `await`；poison 映射要保留中文文案；Tauri v2 的 sync 命令默认主线程，不是线程池。

---

### P0-02 待冲正/异常清单持久化

**现状**：`apps/shell/src-tauri/src/app.rs:308-322` 的 `tx_exceptions` 只读 `state.reversal.outstanding()`；`ReversalBook` 是 `HashMap`（`packages/core/src/reversal.rs:96`）。`tx_log` 里其实已有 `marked_for_reversal` 记录，但没有重建逻辑。

**后果**：进程重启、升级、崩溃后"待冲正"清单归零。柜员关机下班，第二天开机清单没了，未平账交易无人处理——这是账务风险，不是体验问题。

**要做**：
1. `TxJournal` 增加"未决异常"查询（按 `tx_log.status` + `created_at`），启动时 `ReversalBook::restore(...)`。
2. `mark_if_uncertain` 先落 journal 再改内存；`confirm` 成功/失败都写回。
3. 清单按时间排序，界面显示原交易要素（交易码/金额/时间），不只显示 key。

**验收**：
- 提交超时 → 杀进程 → 重启 → `tx_exceptions` 仍显示该笔。
- 冲正确认后重启，该笔不再出现。
- `npm run gate` 绿。

**拆分**：`journal-sqlite`（查询）→ `tx.rs`/`reversal.rs`（restore）→ `app.rs`（接线）。
**量级**：S。**依赖**：无。**契约**：`TxJournal` 是内部 trait，不改对外契约。
**坑**：不能只从 `tx_queue` 重建——被标记的交易不在队列里；要按 `tx_log` 的最终状态重建。

---

### P0-03 幂等键生命周期：按业务事件生成，重试复用

**现状**：`apps/shell/src/components/TxPanel.vue:65` 每次提交都 `crypto.randomUUID()`。`busy` 只挡并发点击，挡不住"超时后柜员再点一次"。

**后果**：重试 = 新键 = 主机视为新交易。**即使 B1 的服务端幂等联调通过，真实 UI 上也不会发生"同一键连发 3 次"**，重复记账风险仍在。`TxEngine` 里"保留原键重试"的设计被界面绕过了。

**要做**：
1. 幂等键在打开表单/确认业务要素时生成一次，存入组件状态；失败/超时后重试复用同一个键。
2. 提交中禁用按钮；超时后只给"原键重试 / 查账 / 转冲正"三个动作，不给"重新提交"。
3. 只有交易到达终态（committed/rejected）或柜员明确"放弃"后才清键。

**验收**：
- 连续点提交 3 次、超时后重试，审计与抓包中的 `idempotency_key` 相同。
- UI 在 in-flight 时按钮不可点；超时后的操作路径符合上述三选一。
- `npm run gate` 绿。

**拆分**：`TxPanel.vue` 一个文件即可；若抽 composable，再拆一个 PR。
**量级**：S。**依赖**：无。**契约**：无。
**坑**：幂等键客户端生成是对的，不要改成"服务端生成"；不要用金额+账号拼 key（要素可变，会撞）。

---

### P0-04 冲正报文补全 + 授权分级

**现状**：`apps/shell/src-tauri/src/app.rs:327-333` 构造的 `ReversalRequest` 是 `tx_code: String::new()`、`amount_cents: 0`；`remote.reverse(key, reason)` 只传键。真实授权中心无法按金额定级，主机也无法核对原交易。B2 用 mock 联调会通过，接真主机才暴露。

**后果**：冲正报文要素不全，主机可能拒绝或冲错；授权形同虚设（DemoSupervisor 固定 T0101）。

**要做**：
1. 从 `tx_log` 取原交易的 `tx_code`/`amount_cents`/`reference`/`created_at`，填入 `ReversalRequest` 和主机报文。
2. `RemoteBanking::reverse` 入参改为结构体（原交易要素 + 原因 + 发起人），不再只传 key。
3. `Authorizer` 按金额阈值/交易码分级；`DemoSupervisor` 只允许在 dev 构建存在。

**验收**：
- 冲正请求报文包含原交易要素（抓包/审计可见）。
- 金额阈值以上必须主管授权；自批被拒；授权记录进审计。
- B2 与真主机联调通过。

**拆分**：`reversal.rs`（要素/授权接口）→ `banking.rs`（reverse 签名）→ `app.rs`（取数/接线）→ `changes/` 提案。
**量级**：M。**依赖**：B2、B3。**契约**：是——`reverse` 的线上报文变更，先写 `changes/`。
**坑**：不要从内存 `ReversalBook` 反查（重启丢了）；从 `tx_log` 查。`amount_cents: 0` 是当前代码的硬伤，不要带到真主机。

---

### P0-05 离线能力可达 + 端到端演练 + 幂等保留期

**现状**：`apps/shell/src/components/TxPanel.vue:72` 写死 `offlineCapable: false`；`RoutedBanking` 默认 `supports_idempotency = false`，`app.rs:357` 会拒绝离线挂起。业务包桥接（`PackageHost.vue:27-33`）可以传 `true`，但业务包仍是 demo。整条离线队列路径从未在真实界面跑过。

**后果**："断网不断业"是大型银行的硬要求（专线抖动、灾备切换）。首次真实断网 = 首次测试这条代码路径。

**要做**：
1. 交易码级离线策略：来源为菜单包或主机能力响应（`offline_capable` + 限额 + 白名单），不要写死在界面。
2. 主机能力响应增加 `idempotency_retention`（幂等键保留期）；契约要求保留期 ≥ 最长离线窗口（建议 7 天）。
3. UI 在策略允许时提供"离线挂起"选项，并显示限额/风险提示。
4. 分行级断网演练脚本与记录。

**验收**：
- 断网 → 提交 `offline_capable` 交易 → Pending → 恢复网络 → 重放成功。
- 断网超过保留期 → 拒绝挂起并提示查账。
- 演练记录（网点/时间/笔数/结果）归档。
- `npm run gate` 绿。

**拆分**：`changes/` 提案（能力字段/保留期）→ `banking.rs`（能力模型）→ `app.rs`（策略判定）→ `TxPanel.vue`（选项）。
**量级**：M。**依赖**：B1、B2。**契约**：是——主机能力字段与幂等保留期。
**坑**：`supports_idempotency` 默认 false 是防御式设计，不要为了"能离线"直接改 true；离线限额必须来自服务端，不能由客户端自己定。

---

### P0-06 终端身份、mTLS、请求签名、默认 TLS

**现状**：`apps/shell/src-tauri/src/lib.rs:118-146` 的 `plaintext = !TK_REQUIRE_TLS` **默认 true**；`packages/transport-net` 明确不支持客户端证书；TLS 只信 `webpki-roots` 公共根，无 OCSP/CRL/pinning（B6）。

**后果**：任何拿到 `endpoints.json` 的机器都能连主机；交易无法归因到终端/网点；默认可能走明文。

**要做**：
1. mTLS：客户端证书（TPM/USBKey/DPAPI 保护的文件），行内 CA 根显式加载；证书轮换与吊销（B6）。
2. 请求级签名：`terminal_id + outlet + teller + timestamp + nonce + body hash`，服务端验签+防重放。
3. 默认 fail-closed：`TK_REQUIRE_TLS` 默认 true；明文端点需要显式开发开关，且 release 构建禁用。
4. 交易、审计、日志统一带 `terminal_id/outlet/teller/request_id`。

**验收**：
- 无客户端证书连接被拒；过期/吊销证书被拒；负向用例进 CI 或联调清单。
- 默认配置下明文端口不可用；release 包无法通过环境变量打开明文。
- 一笔交易的请求头与日志能用 `request_id` 串起来。

**拆分**：`transport-net`（mTLS/根证书）→ `config_sig`/`lib.rs`（默认值/fail-closed）→ 请求签名（`banking.rs`/`remote.rs`）→ `changes/`。
**量级**：L。**依赖**：行内 CA/PKI、网关、B6、B8。
**坑**：rustls 不读系统证书库，行内根必须显式加载；IP 直连时 SNI/主机名校验要单独处理。

---

### P0-07 国密算法套件（CryptoProvider）

**现状**：Ed25519 + SHA-256 + AES-GCM + rustls，全是国际算法；包格式、账本加密、TLS 都绑死。

**后果**：大型银行受《密码法》、监管 317 号文、等保 2.0/密评约束，柜面终端通常要求 SM2/SM3/SM4 与国密 TLS（GM/T 0024）。这是采购/测评准入，不是"优化"。

**要做**：
1. 抽 `CryptoProvider`（签名/验签、摘要、对称加密、随机数），现有实现作为 `intl` 后端。
2. 国密后端：SM2 签名/验签（菜单包、配置）、SM3 摘要、SM4-GCM（账本）；国密 TLS 由网关终结或客户端套件支持，二选一并记录 ADR。
3. 包格式/配置格式加算法标识；`format` 版本递增，兼容期双算法验签。
4. 选型必须是有商用密码资质的库，不自己实现 SM2/SM3。

**验收**：
- SM2 签名的菜单包可验签；SM4 账本可读写；国密 TLS 握手成功（按选定方案）。
- 算法标识写入包格式文档与 `changes/`；旧包仍可验（兼容期）。
- 密评自查表逐项有结论。

**拆分**：`CryptoProvider` trait（core）→ 国密后端（新 crate）→ `pack.rs`/`journal-sqlite` 接入 → 包格式 `changes/`。
**量级**：L。**依赖**：国密库选型、网关方案、密评咨询。**契约**：是——包格式/配置格式/线上 TLS。
**坑**：不要用未审计的纯 Rust 实验库；SM2 签名格式（DER vs raw）要与服务端对齐；性能基线要重测（SM2 比 Ed25519 慢，20 万台启动验签会放大）。

---

### P0-08 菜单包损坏时回退本地 N-1

**现状**：`packages/core/src/sync.rs:66` 起，`reload()` 遇到任何 `StoreError`（包括最新包验签失败）直接联网；`store.load_version()` 存在却没用。

**后果**：离线 + 最新包一个位翻转 = 开不了机，哪怕上一版完好。20 万台终端，这是必然事件。

**要做**：
1. `load_best_available()`：按版本降序逐个 `load_version` → 验签 → decode，第一个成功即用。
2. 全部失败才联网；降级要写入 `BootSummary.notes` 并在界面提示"已回退到 vN"。
3. 降级事件上报（接入 P1-09 的遥测）。

**验收**：
- 删除/翻转最新包的一个字节，断网启动仍能用 N-1，界面有降级提示。
- 所有本地包都损坏时才走网络；网络也失败才报错。
- `npm run gate` 绿。

**拆分**：`sync.rs` + `store.rs`（一个 PR，两文件需说明）；`app.rs`（提示）。
**量级**：S。**依赖**：无。**契约**：无。
**坑**：`list_versions` 要按数值排序，不是字符串；`load_latest` 失败时不要 `panic`/`unwrap`。

---

### P0-09 配置 fail-closed + 生产剔除 mock + 集中下发

**现状**：`apps/shell/src-tauri/src/lib.rs:118-146` 配置非法时 `eprintln!` 后回退内置 mock；`endpoint_config.rs` 失败也回退 mock。配置依赖 `TK_ENDPOINTS`/`TK_CONFIG_PUBKEY`/`TK_REQUIRE_SIGNED_CONFIG`/`TK_REQUIRE_TLS` 环境变量。

**后果**：生产终端静默进入"演示模式"；20 万台无法集中管理配置；环境变量可被本机用户篡改。

**要做**：
1. release 构建用 feature 剔除 mock 实现；dev 构建才允许 mock。
2. 配置缺失/验签失败 → 启动错误页（fail-closed），不进入业务界面；错误文案给网点 IT 可执行的指引。
3. 配置由域策略/SCCM/配置中心下发，本地缓存带版本与签名；支持灰度更新与回滚。
4. B8：签名私钥进 HSM/受控签名服务。

**验收**：
- 删除配置、改一字节签名 → release 包拒绝启动；dev 包仍可 mock。
- release 包里 `strings` 搜不到 mock 端点/演示角色；或在编译期用 feature 断言。
- 配置更新可灰度到指定网点，失败可回滚。

**拆分**：`Cargo.toml` feature + `lib.rs`（构建分支）→ `endpoint_config.rs`（fail-closed）→ `changes/`（配置下发协议，如需要）。
**量级**：M。**依赖**：B8、终端管理/域策略。**契约**：配置下发协议如需新增，走 `changes/`。
**坑**：不要用运行时环境变量切换"生产/演示"——那等于把开关交给本机用户；用编译期 feature。

---

### P0-10 审计可信化：集中、时钟、防篡改、保留

**现状**：`packages/journal-sqlite/src/lib.rs:79` 的 `audit_log` 本地单文件、只增无删 API、无哈希链；`now()` 用本机 `SystemTime` epoch 秒；`app.rs:601` 审计失败只 `eprintln!`；无集中上报。

**后果**：改系统时间即可伪造顺序；本地库可删；20 万台审计无法集中检索。大型银行审计要求集中、防篡改、留存、可检索——当前只是"本地日志"。

**要做**：
1. 审计双写：本地 WAL + 上报队列；断网缓存、恢复补传；上报失败告警但不阻断交易。
2. 服务端时间戳/序号；本地事件带 `prev_hash` 哈希链，定期锚定。
3. 本地保留策略（如 90 天）+ 归档；`audit_tail` 加索引；集中检索按柜员/终端/交易/时间。
4. 审计失败不能静默：本地告警 + 上报。

**验收**：
- 改本机时间不影响服务端排序；删除本地库后服务端仍有完整记录。
- 哈希链校验脚本能发现篡改/断链。
- 100 万行时 `audit_tail` 查询 < 100ms；保留期策略生效。

**拆分**：`journal-sqlite`（哈希链/索引/归档）→ 上报客户端（新模块）→ `changes/`（上报协议）。
**量级**：L。**依赖**：日志/审计平台。**契约**：是——审计上报协议。
**坑**：审计不能阻断交易，但也不能静默丢；队列要有上限与丢弃告警。哈希链的 `prev_hash` 要按终端维度，不是全局。

---

### P0-11 单实例锁

**现状**：`apps/shell/src-tauri/tauri.conf.json` 无 single-instance 配置；菜单安装用固定 `.tmp` 路径；SQLite 单连接但多进程可同时打开。

**后果**：双开 → 两套内存幂等表、双份审计、菜单安装竞争（同一 `.tmp`）、会话锁屏各自为政。Windows 快速启动/远程桌面/误双击都可能触发。

**要做**：
1. 接 `tauri-plugin-single-instance`：第二个实例聚焦已有窗口后退出。
2. 菜单安装的临时文件名带 pid/随机后缀；安装前加文件锁。
3. SQLite 打开加 `busy_timeout`，并对"多进程"写一条明确不支持的日志。

**验收**：
- 双击两次只有一个窗口；第二个进程退出。
- 并发 boot/安装菜单无 `.tmp` 冲突、无损坏包。
- `npm run gate` 绿。

**拆分**：`Cargo.toml`+`lib.rs`（插件）→ `store.rs`（tmp/锁）。
**量级**：S。**依赖**：无。**契约**：无。
**坑**：插件版本要与 Tauri 2 匹配；macOS 的"再次打开"行为与 Windows 不同，两个平台都要验。

---

## 3. P1：规模化与合规加固

| ID | 任务 | 证据 | 验收（可执行） | 量级 | 前置/契约 |
|---|---|---|---|---|---|
| P1-01 | HTTP 连接池/keep-alive/压缩 | `packages/transport-net` 每次新建 TCP，明确不支持连接复用/压缩 | 同一端点连续 10 次调用只有 1 次 TCP/TLS 握手（计数器/抓包）；菜单包 gzip 后 ≤ 原体积 40%；gate 绿 | M | 无/线上协议不变 |
| P1-02 | 重放 jitter + attempts 持久化 + 服务端准入 | `packages/core/src/replay.rs` 纯内存、无 jitter；`replay_pending` 同步逐笔 | 1000 台模拟终端恢复，重放时间分散（统计 P95）；重启后 attempts 保留；服务端 429 时退避 | M | P0-02 |
| P1-03 | 灰度急停/遥测/稳定分桶/差分升级 | `packages/core/src/update.rs:85`、`:132-137` hash 含 version；无 kill switch/遥测/执行器 | 服务端 kill switch 后 `decide=Skip`；同 outlet 同版本调 percent 不横跳；下载/安装成功率上报；差分包可安装 | L | B5/`changes/` |
| P1-04 | 登录探测定时抖动 | `apps/shell/src/App.vue:201-214` 固定 600ms | 20 万台模拟，探测分布在 30 分钟；ETag 304 生效 | S | P1-01 |
| P1-05 | 业务包进程隔离/配额/manifest 权限 | `apps/shell/src/platform/sandbox.ts:80` postMessage `"*"`；同进程无配额 | 死循环包 5s 超时被杀；内存上限生效；未声明 API 调用被拒；消息校验 source/origin | L | 无/`changes/`（包 manifest） |
| P1-06 | 权限快照签名 + TTL + 即时失效 | `packages/core/src/perm.rs` 无签名/无过期 | 改快照签名 → 拒绝；TTL 过期重拉；`permVersion` 变更 60s 内生效 | M | B3/`changes/` |
| P1-07 | IAM/金额授权/双人复核 | `apps/shell/src-tauri/src/app.rs:325` 固定 T0101 | 大额交易触发主管授权；自批被拒；授权记录可检索；B3 联调 | L | B3、P0-04 |
| P1-08 | 对账双边化 + 会计日 | `apps/shell/src-tauri/src/app.rs:94` UTC 日界；只读本地账本 | 主机流水 vs 本地 `tx_log` 差异清单；跨时区日界正确；差异进差错队列并闭环 | M | 主机接口/`changes/` |
| P1-09 | 可观测性：日志/trace/心跳/诊断包 | 全仓仅 3 处 `eprintln!`；无日志文件/心跳/崩溃上报 | 结构化滚动日志；`request_id` 贯穿；心跳/版本/错误码上报；一键诊断包（脱敏） | M | P0-06（身份） |
| P1-10 | 错误码结构体过 IPC | `commands.rs` 统一 `Result<_, String>`；`contracts/error-codes.md` 只作文案 | 前端按 `code` 分支；错误结构 `{code,message,retryable,traceId}`；契约文档同步 | M | `changes/` 批准 |
| P1-11 | 菜单校验/过滤去掉 O(n²) | `packages/core/src/model.rs:172-180`；`filter.rs` visited 线性查重 | 2 万节点 `validate` < 50ms、`filter_visible` < 20ms（bench）；gate 绿 | S | 无 |
| P1-12 | 查账/状态查询接口 | 冲正前无主机状态查询，只能人工判断 | `query_tx_status` 返回主机状态；冲正前强制查询；mock + 真主机用例 | M | B2/`changes/`（新命令） |
| P1-13 | 审计/流水索引、归档、保留 | `journal-sqlite` 无 created_at/at 索引；无 VACUUM/归档 | 100 万行 `audit_tail` < 100ms；归档后仍可查；保留策略生效 | S | P0-10 |
| P1-14 | 信创机型适配矩阵 | Tauri/WebKitGTK 未验证；无国密 | 麒麟/统信 + 鲲鹏/飞腾 启动、菜单、交易、打印跑通；性能基线达标 | L | P0-07 |
| P1-15 | 真机外设联调（B4） | `packages/core/src/device.rs` 仅 Mock* 实现 | 打印/读卡/密码键盘/扫描各一笔完整交易；脱敏/白名单在真机验证 | L | 硬件/驱动 |
| P1-16 | 终端管理/资产大盘 | 无在线率/版本分布/远程诊断 | 在线率、版本分布、故障 Top、远程诊断可用 | L | P1-03、P1-09 |

---

## 4. P2：体验、成本与交付

- [ ] **P2-01 老终端与可访问性**：1024×768、125%/150% 缩放、高对比度、字体放大。验收：目标最低配机器上无溢出/遮挡；柜员可用性走查。
- [ ] **P2-02 KIOSK/看门狗/自启/安装包签名**：`bundle.active=false`，无签名、无 SCCM 分发。验收：MSI 静默安装、开机自启、崩溃自动重启、安装包签名验证。
- [ ] **P2-03 打印队列/重打控制/模板版本**：打印失败无补偿，重打无审计。验收：断纸/脱机可重打、重打留痕、模板版本可回滚。
- [ ] **P2-04 PIN/敏感内存**：`PinBlock(String)` 无 zeroize；崩溃转储可能落盘。验收：zeroize、禁 core dump、DPAPI 保护。
- [ ] **P2-05 时钟校验**：依赖本机时钟做 TLS/审计/日界。验收：启动 NTP 偏差检查，超阈值告警/拒绝交易。
- [ ] **P2-06 日志脱敏与留存**：日志不得出现 PAN/身份证/金额明文。验收：脱敏规则 + 抽样审计。
- [ ] **P2-07 多语言**：当前硬编码中文。验收：至少 en/zh 资源化，少数民族地区可按需扩展。
- [ ] **P2-08 业务包签名到厂商/审核流**：当前只有 demo key。验收：厂商证书链、包审核记录、撤销列表。
- [ ] **P2-09 业务埋点**：菜单点击、交易时长、失败率、重放次数。验收：指标进大盘，口径写进 `docs/metrics.md`。
- [ ] **P2-10 启动/内存优化**：WebView2 冷启动、包体积、首屏。验收：目标机型冷启动 ≤ 3s、空闲 RSS ≤ 200MB（沿用 PLAN 的口径）。
- [ ] **P2-11 高可用/灾备演练**：主机切换、专线中断、分行级故障。验收：演练脚本 + 记录 + 恢复时间目标。
- [ ] **P2-12 代码/依赖治理**：`cargo-deny`/SBOM/许可证扫描、依赖升级策略。验收：CI 出 SBOM，无高危许可证/漏洞。

---

## 5. 与 B1–B8 的关系

`docs/production-readiness.md` 的 8 条仍是硬阻断，本清单**不替代**它们：

| 既有阻断 | 本清单推进项 |
|---|---|
| B1 服务端幂等未联调 | P0-03、P0-05（键生命周期与保留期） |
| B2 冲正接口未联调 | P0-04、P1-12 |
| B3 IAM 未接 | P1-06、P1-07 |
| B4 真机外设未验证 | P1-15 |
| B5 更新执行器未接 | P1-03 |
| B6 证书吊销缺失 | P0-06 |
| B7 账本密钥未进钥匙串 | P0-06 的证书/密钥管理一并做；B7 本身仍单列 |
| B8 配置签名私钥保管 | P0-09 |

**B1–B8 之外，本清单新增的代码级阻断**：P0-01（同步阻塞）、P0-02（状态不落盘）、P0-03（幂等键被 UI 绕过）、P0-04（冲正要素不全）、P0-08（不回退 N-1）、P0-09（fail-open），外加 P0-11（单实例）。

---

## 6. 进度台账

| 日期 | ID | PR | gate | 备注 |
|---|---|---|---|---|
|  |  |  |  |  |

> 建议每行只记一个 ID；"gate" 填 `PASS`/`FAIL` 及失败项。未登记的项视为未完成。

---

## 7. 评审结论摘要（给未来的会话）

- 骨架是对的：分层、契约只读、离线优先菜单、幂等交主机收口、`TxStatus` 无 Unknown、不确定结果不缓存、229 个测试。
- 真正的工作量在骨架之外：**运行时（阻塞 I/O）、状态（内存态）、合规（国密/信创/终端身份/审计）、运维（日志/灰度/分发/终端管理）**。
- 大规模部署下，单机 TPS 不是主要矛盾；主要矛盾是**20 万台同时启动、同时探测、同时重放、同时升级**，以及**没有集中观测时的一次故障定位**。
- 如果只能做三件事：P0-01 + P0-02 + P0-03（运行时与状态），P0-06 + P0-07（身份与国密），P1-09 + P1-03（观测与灰度）。

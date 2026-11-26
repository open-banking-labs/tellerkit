# 架构评审待办 · TellerKit

> 来源：2026-11-25 架构评审（基线 `155e0fa`，工作区 worktree `dsh-20ed3bca-448c`）。
> 用途：跨会话逐步实施。**一项一次会话、一项一个 PR**，不要合并扫描。
> 总排序以 [项目级整改总台账](../project-backlog.md) 为准；本文件的 M1–M4 是细项证据与验收标准。
> 状态：全部 `未开始`。完成一项就把该行改成 `已完成（PR #NN）`，不要删条目。
>
> 阅读顺序：本文件 → `docs/architecture.md`（分层前提）→ `docs/production-readiness.md`（B1–B8 已知阻断）→ 相关 `AGENTS.md` → 目标目录 `README.md`。
>
> 约束（不变）：`contracts/**` 只读，改契约先写 `changes/` 提案；不自主扩范围；单次新增 >300 行退回重拆；不声称"测试已通过"，以命令 exit code 为准。
>
> 优先级：**M1 = 试点前必须（缺一项就不能给分行）**；**M2 = 上生产前必须**；**M3 = 门禁纵深（可与 M1 并行）**；**M4 = 减法与债务**。
> 规模：S ≤ 0.5 天 · M ≤ 2 天 · L ≤ 5 天（单 PR 超 M 一律再拆）。

---

## M1 · 试点前必须（P0）

### A-01 信任锚点：生产公钥注入，移除演示密钥

- **严重度** P0 · **规模** M · **依赖** 无
- **证据** `apps/shell/src-tauri/src/lib.rs:222` `MenuStore::new(menu_dir, demo_verifying_key())`；`apps/shell/src-tauri/src/app.rs` 业务包 `pkg::scan_dir(..., demo_verifying_key(), ...)`；`packages/core/src/mock.rs:16` `DEMO_SEED` = ASCII `TK-DEMO-ONLY-NOT-PRODUCTION-0001`；仓库内无任何生产公钥注入路径。
- **危害** 拿到源码的人可签发合法菜单包/业务包（任意菜单、任意 action、任意 JS），客户端验签通过并执行。
- **改法**
  1. `build.rs`（或 `option_env!`）读取构建期 `TK_MENU_PUBKEY_HEX`，编译进常量；缺失即编译失败（release）。
  2. `packages/core/src/mock.rs` 的 `demo_signing_key/demo_signing_key` 整个模块 `#[cfg(any(test, debug_assertions))]`；业务包/菜单包各一把公钥，签名头带 `key_id` 支持轮换。
  3. 启动断言：生效公钥 ≠ demo 公钥，否则 release 直接拒绝启动并写审计。
- **验收** `cargo build -p tellerkit --release` 在未注入公钥时失败；注入后 `boot()` 返回的 `pubkey_hex` 等于注入值；`grep -r "demo_verifying_key" apps/` 在 release 编译单元中为空；新增一条单测：错钥包被拒 + 审计有记录。

### A-02 锁屏变成真门（Rust 侧授权门）

- **严重度** P0 · **规模** M · **依赖** 无
- **证据** `apps/shell/src-tauri/src/app.rs` 仅 `session_tick/session_blur/session_touch` 读锁状态；`submit_transaction`、`print_voucher`、`request_reversal`、`replay_pending`、`package_module` 均不检查；`apps/shell/src/App.vue:273` 键盘处理不看 `locked`；LockScreen 只是 `v-if` 覆盖层，`PackageHost` 与其 iframe 仍持有 RPC 通道。
- **危害** 柜员离岗后任何人可提交交易/打印凭证，审计仍记原柜员工号。
- **改法** `AppState` 增加 `require_unlocked(&self) -> Result<(), String>`（返回中文"会话已锁定，请重新验证"），在上述每个变更类命令入口调用；`onKeydown` 在 `locked` 时直接 return；锁屏挂载时销毁 `PackageHost`；`clipboard.ts` 增加锁屏清除；`logout` 失败时保持锁屏。
- **验收** 单测：锁定后调用 5 个变更函数均返回 Err 且审计记 `result=failed`；手动：锁屏后按方向键/回车无反应；`npm run gate` 绿。

### A-03 IPC 并发模型：同步命令 + 全局锁跨网络 IO

- **严重度** P0 · **规模** L · **依赖** 无
- **证据** `apps/shell/src-tauri/src/commands.rs` 全部为同步 `fn`（Tauri 同步命令跑在主线程），每个命令 `state.lock()` 独占 `Mutex<AppState>`；`submit_transaction`/`replay_pending`/`request_reversal` 在持锁状态下发远端调用（`endpoint_config.rs` 默认 `timeout_ms=3000`）；`lib.rs` 后台重放线程每 60s 抢同一把锁并做网络调用。
- **危害** 一次网络抖动导致整窗冻结；后台重放期间 UI 线程阻塞在锁上；所有命令全局串行。
- **改法（建议分三步，各自一个 PR）**
  1. 全部命令改 `async fn` + `#[tauri::command(async)]`（跑在异步运行时线程池），先止血；
  2. 把 `AppState` 拆成 `SessionState`/`LedgerHandle`/`MenuState` 三把独立锁，网络调用一律移出锁（取快照→放锁→调用→回写）；
  3. 重放线程改为命令层驱动的 `tokio::spawn` + 单飞标志，与 UI 命令互不阻塞。
- **验收** 端点注入 `timeout_ms=8000` 且主机不可达时，`session_tick` 的 P99 < 50ms（可在 `dev` 模式加计时日志断言）；并发 10 次 `submit_transaction` 无 deadlock；`cargo test --workspace` 绿。

### A-04 写前记录（write-ahead intent）与半原子修复

- **严重度** P0 · **规模** M · **依赖** A-03（建议先做）
- **证据** `packages/core/src/tx.rs` `submit()` 先 `host.submit()` 再 `journal.record()`，Pending 时再 `enqueue()`；崩溃落在这两步之间 = 主机已入账、本地零记录；`packages/journal-sqlite/src/lib.rs` 的 `record` 与 `dequeue` 是两条自动提交语句，`tx.rs` 重放成功路径先 `record` 后 `dequeue`。
- **危害** 日终少一笔且无任何可查线索；或"已成功但仍在队列"导致重启后重复提交。
- **改法** 新增 `tx_intent` 表（`idempotency_key, tx_code, amount_cents, state[in_flight|done], created_at`）：主机调用前写 `in_flight` → 调用后更新 `done` 并写 `tx_log`；`TxJournal` 增加 `record_committed(&TxRequest,&TxResult)`，用单个 `Connection::transaction()` 同时写 `tx_log` 与 `dequeue`；启动时把所有遗留 `in_flight` 转入异常队列（见 B-01）。
- **验收** 断言测试：杀进程模拟（`drop` 引擎后重建）→ `pending()` 含该笔且异常队列含该笔；`record_committed` 单测证明 sqlite 事务失败时两表均无变化。

### A-05 交付链：安装包、签名、更新器

- **严重度** P0 · **规模** L · **依赖** 无（可与 A-01 并行）
- **证据** `apps/shell/src-tauri/tauri.conf.json` `bundle.active: false`、`identifier: "com.tellerkit.demo"`；`src-tauri/Cargo.toml` 无 `tauri-plugin-updater`；`.github/` 只有 `gate.yml`（无 release workflow、无签名、无制品上传）；`packages/core/src/update.rs` 只有 `decide`/`rollout_bucket`，无执行器。
- **危害** 产不出可安装产物；安全补丁没有下发通道；demo 包名破坏升级身份。
- **改法** `bundle.active: true` + NSIS 目标 + 正式 reverse-DNS identifier；`tauri-plugin-updater` 接 `update.rs` 的决策（灰度桶 → 下载 → 验签 → 原子切换 → 失败回滚）；新增 `.github/workflows/release.yml`：Windows 构建 → 签名 → SBOM → 制品上传 → 更新 manifest 签名。私钥不入库（CI secret + 保管流程写进 `docs/`）。
- **验收** 在 Windows 构建机上产出 `.msi/.exe` 并通过签名校验；装 v1 → 发 v2 → 客户端探测到并完成升级；断网升级失败可回滚到 v1；`verify-p0.sh` 在打包产物上跑通一次。

### A-06 失败模式收紧：不再静默降级

- **严重度** P0 · **规模** M · **依赖** A-01
- **证据** `apps/shell/src-tauri/src/config_sig.rs:79` 默认"没配公钥就不要求签名"；`endpoint_config.rs` `require_tls` 默认 false 且 `tcp` 无 TLS 选项；`apps/shell/src-tauri/src/lib.rs` 端点配置非法时回退 `MockRemote` 并只打一行 note；`commands.rs` 的 `dev_*` 在 release 仍注册，只靠运行时 `cfg!(debug_assertions)` 拦；`apps/shell/src/platform/index.ts` 运行时选择 mock。
- **危害** 改 `endpoints.json` 即可把交易导走；JSON 拼错即"演示模式照常成功"；明文 TCP 传账号；release 仍暴露 `dev_corrupt_local` 写盘能力。
- **改法** release 下强制签名（删除可关闭签名的 env 覆盖）、`require_tls` 默认 true、`tcp` 传输在 release 构建中移除或强制 TLS；配置非法 → 致命错误页（"端点配置未就绪，请联系管理员"）+ 不启动 mock；`dev_*` 函数与注册都加 `#[cfg(debug_assertions)]`；`platform/index.ts` 在 `import.meta.env.PROD` 且无 `__TAURI_INTERNALS__` 时报错而非静默 mock。
- **验收** 篡改 `endpoints.json`（未签名）→ 启动失败；删公钥 env → 启动失败；`strings` release 二进制不含 `dev_corrupt_local`；单测覆盖 `load_router_with` 的 require_tls/require_signed 分支。

### A-07 单实例守卫 + 账本排他锁

- **严重度** P0 · **规模** S · **依赖** 无
- **证据** 无 `tauri-plugin-single-instance`；`lib.rs` 无条件 `spawn_replay_worker` 并 `SqliteJournal::open(appdata/tx.db)`。
- **危害** 双开 = 双重重放 + `database is locked` + 两个"同一柜员"会话。
- **改法** 加 `tauri-plugin-single-instance`（第二个实例聚焦已有窗口）；账本启动时取独占锁（`tx.db.lock` 或 `PRAGMA locking_mode=EXCLUSIVE`），拿不到就致命报错。
- **验收** 双开第二个实例不启动重放线程；`journal.pending()` 不被双写；日志有明确"已有实例在运行"。

### A-08 业务包沙箱 bootstrap 修复 + 握手超时

- **严重度** P0 · **规模** M · **依赖** 无
- **证据** `apps/shell/src/platform/sandbox.ts:77-127` bootstrap 是内联 `<script>` 且内联 `import(blob)`；`tauri.conf.json` CSP 为 `script-src 'self' blob:`（无 `unsafe-inline`）；`srcdoc` 文档继承父 CSP。headless 实验显示打包态两段内联脚本均被拦。`PackageHost.vue` 等待 `sandbox.done` 无超时，打包后永久卡"正在加载业务包模块…"。
- **危害** 试点现场业务页面全部打不开；诱使修复者加 `'unsafe-inline'`，把 RPC 桥暴露给注入脚本。
- **改法** bootstrap 改为外部 asset/blob URL 文档（自身带 CSP），或用 `script-src` 里放该 bootstrap 的 sha256；`mountSandboxed` 加 3–5s 握手超时，超时渲染错误态 + 诊断码。
- **验收** 打包构建（`tauri build`）下业务包页面能加载；故意不发 ready 的假包在 5s 内显示错误而非旋转；单测覆盖 `dispatchCall` 白名单拒绝路径。
- **备注** 这条的静态推导较强，落地前先在打包构建做一次冒烟确认。

---

## M2 · 上生产前必须（P1）

### B-01 异常闭环：`gave_up` 必须有人接手

- **严重度** P1 · **规模** S · **依赖** A-04
- **证据** `packages/core/src/replay.rs:388` 收集 `gave_up`；`apps/shell/src-tauri/src/app.rs` 只把条数拼进返回文案；`reversal` 账本与 `tx_exceptions` 完全不知道它们；`tx_pending_count` 仍统计它们。
- **改法** 重放耗尽即 `reversal.mark(key, ExceptionKind::MarkedForRollForward)` 并写审计；`tx_exceptions` 暴露；UI 顶部常驻告警条。
- **验收** 单测：连续失败至 `exhausted` 后 `tx_exceptions()` 返回该 key；UI 快照测试能看到告警。

### B-02 冲正回捞原交易上下文

- **严重度** P1 · **规模** M · **依赖** A-04
- **证据** `apps/shell/src-tauri/src/app.rs` `request_reversal` 构造 `ReversalRequest { tx_code: String::new(), amount_cents: 0, ... }`；`ReversalBook` 不读账本。
- **危害** 冲正申请里没有金额与交易码，审计与主机都无法确认冲的是哪笔。
- **改法** 账本增加按幂等键查 `tx_log` 的查询；`ReversalRequest` 从账本填充 `tx_code/amount_cents/elements`；冲正前弹确认框展示金额与流水号。
- **验收** 集成测试：提交超时交易 → 异常列表 → 冲正请求体含正确金额与交易码；审计 detail 含金额。

### B-03 重放预算持久化

- **严重度** P1 · **规模** S · **依赖** A-04
- **证据** `packages/core/src/replay.rs` 的 attempts 只在 `HashMap` 内存；`tx_queue` 无 `attempts/next_at` 列；重启归零。
- **改法** 队列行加 `attempts`、`next_at`、`abandoned`；调度器读写持久值；`abandoned` 行不再参与调度但保留可查。
- **验收** 重启后 attempts 续算；给满 attempts 的行不再被 `due()` 选中。

### B-04 N-1 回退（README 声称已有，实际没有）

- **严重度** P1 · **规模** S · **依赖** 无
- **证据** `packages/core/src/store.rs` `load_latest` 只试最高版本并传播验签错误；`keep=2` 仅用于清理；`sync.rs` 回退路径要求联网。
- **危害** 断网 + 最新包损坏 = 开不了机，而好的旧包就在旁边。
- **改法** `load_latest` 按 `list_versions()` 降序逐个验签，返回第一个通过者并记录实际版本；`LoadSource` 增加 `LocalFileFallback{version, from}`。
- **验收** 单测：删掉 N 的文件头/改一个字节 → 加载到 N-1 且 `source` 标明回退；全部损坏才报错。

### B-05 版本语义与命名空间校验

- **严重度** P1 · **规模** S · **依赖** B-04
- **证据** `packages/core/src/store.rs` `current_version` 直接信任文件名，不验签；`packages/core/src/pack.rs` 的 `shell_compat_min/max` 解出来后从不比较；`packages/core/src/pkg.rs:263` 只校验 `node.code`，`model.rs` 接受任意存在的 `parent_code`。
- **改法** `current_version` 改为"最近一次验签通过的版本"；`install`/`boot_load` 用 `SHELL_COMPAT_*` 常量校验，不匹配返回专用 `StoreError`；非根节点的 `parent_code` 必须等于 `manifest.id` 或以 `"{id}."` 开头。
- **验收** 单测：残留 `menu.999999.mtree` 不再导致"永远有新版本"；compat 不符的包被拒；跨命名空间挂载的包被拒。

### B-06 账本加密加固

- **严重度** P1 · **规模** M · **依赖** 无
- **证据** `packages/journal-sqlite/src/cipher.rs` `seal` 用 `Aad::empty()`；`open_or_create` 在 `ledger.key` 缺失时**新建**密钥而不检查库里是否已有 `v1:` 行；`restrict_permissions` 在 Windows 是 no-op；密钥与 `tx.db` 同目录（= B7）。
- **危害** 密文未绑定行 → 行间互换可篡改"转给谁"；密钥丢失即静默产生一批永久不可解密的挂起交易；Windows 上密钥权限等同 DB。
- **改法** AAD = 表名 + 幂等键；DB 有加密行且密钥缺失 → 拒绝打开并告警；密钥移出 DB 目录（`ProgramData`/凭据管理器），Windows 用 DPAPI 封装；存储值加格式前缀并拒绝在 v1 库里读明文。
- **验收** 单测：互换两行密文 → 解密失败；删密钥后打开失败且提示可读；`PRAGMA user_version` 标记格式版本。

### B-07 审计不可抵赖

- **严重度** P1 · **规模** M · **依赖** B-06
- **证据** `packages/journal-sqlite/src/lib.rs` `audit_log` 是普通表，无哈希链、无导出、无校验；`app.rs` 审计失败只 `eprintln!`；`docs/production-readiness.md` 把"本地审计可删"列为非阻断偏差。
- **改法** 每条审计写 `prev_hash` + `hash(prev_hash||payload)`；提供 `audit_verify` 命令与日终导出（可选上传服务端锚定）；写失败 → UI 常驻红色告警并禁止提交资金类交易。
- **验收** 手工改一行 → `audit_verify` 报出断点行号；账本不可写时 `submit_transaction` 被拒。

### B-08 可观测性与诊断包

- **严重度** P1 · **规模** M · **依赖** 无
- **证据** 全仓无 `tracing`/`sentry`/`opentelemetry`，无 `tauri-plugin-log`；无日志轮转/保留；现场排障只能靠用户截图。
- **改法** `tauri-plugin-log` 滚动文件 + 30 天保留 + 敏感字段脱敏规则；新增 `export_diagnostics` 命令（版本、包哈希、账本计数、最近 200 条审计、最近错误）打成 zip；文档写清存放路径与上交流程。
- **验收** 现场模拟故障 → 导出的 zip 能在离线环境定位原因；导出内容不含卡号/金额明文（用测试卡号扫描）。

### B-09 前端错误语义与交互缺陷打包修复

- **严重度** P1 · **规模** M · **依赖** A-03
- **证据** `apps/shell/src/components/TxPanel.vue:65` 每次点击新建 `crypto.randomUUID()` 幂等键（"重试"= 新交易）；同文件 `catch` 把任何 IPC/传输错误映射成 `status:"rejected"`；`App.vue:32` `showReconcile` 永远 false，`ReconcilePanel` 不可达（#28 成果是死代码）；`App.vue` `logout` 先清锁再调 IPC 且无 catch；`app.rs` 业务日界用 UTC 截断（`start_of_day`），面板文案却写"本地时间当日 00:00"。
- **改法** 幂等键随表单生命周期生成、提交失败复用、终态才清除；未知错误显示"状态不明，请勿重复提交"并写入异常；对账面板接到菜单/工具栏入口；`logout` 失败回锁；日界走 `Clock`+时区端口（或由主机给 `since`）。
- **验收** 模拟超时后重试 → 请求体幂等键相同；`tx_exceptions` 出现该笔；对账面板可从 UI 打开且合计与账本一致；UTC 0 点前后各测一次。

### B-10 打印凭证收口

- **严重度** P1 · **规模** S · **依赖** 无
- **证据** `apps/shell/src/components/TxPanel.vue:90` 模板名由菜单 code 拼接；`commands.rs:337` 原样接受 `template`/`fields`；金额在打印时从表单实时读（`amountCents.value! / 100`）。
- **危害** 任意模板名/字段可打印；业务包经白名单可直接 `printVoucher`，可造出"看起来像真的"凭证。
- **改法** 模板注册表按 `txCode` 白名单，未知模板拒绝；打印字段从已落库的 `TxResult` 取值；`fields` 按模板 schema 校验。
- **验收** 未知模板返回中文错误；打印金额与账本记录一致（单测 + 手工）。

### B-11 登录凭据（依赖 B3/IAM）

- **严重度** P1 · **规模** M · **依赖** 外部 IAM（`production-readiness.md` B3）
- **证据** `apps/shell/src/components/LoginView.vue:51` 只有角色按钮；`apps/shell/src/api.ts:10` 硬编码 ROLES；`app.rs` 直接信任 `role_id`。
- **改法** 接主机会话令牌或主管 PIN；客户端不得授予主机未返回的角色；`ROLES` 从 release 构建剔除。
- **验收** 主机返回低权角色时前端无法自选主管；无凭据登录失败。

---

## M3 · 门禁纵深（可与 M1 并行）

### C-01 `gate` 必须是真裁决

- **严重度** P0（治理） · **规模** S · **依赖** 无
- **证据** `package.json` `gate` = 5 个 node 检查 + `cargo fmt --check`，**不含测试/clippy/编译**；`package.json` 里已有的 `verify` 反而没人用；`AGENTS.md` 却写"gate 是唯一裁决入口"。
- **改法** `gate` 改为 `verify`（test + clippy + build + verify-p0），或保留快 gate 但改名为 `gate:fast` 并把"唯一裁决"改成 CI 状态。二者择一，同步改 `AGENTS.md`/`docs/constitution.md`/`docs/PLAN.md` 的措辞。
- **验收** 故意让一个单测失败 → `npm run gate` 退出码非 0；文档与实际命令一致。

### C-02 锁文件与可复现构建

- **严重度** P0 · **规模** S · **依赖** 无
- **证据** workspace 6 个成员，`Cargo.lock` 只有 `tellerkit`/`tellerkit-core`/`tellerkit-journal-sqlite`，缺 `tellerkit-transport-net`、`tellerkit-fake-bank`、`mtree-tool`，且无 `rustls/ring/webpki-roots/rcgen`；CI 与 `scripts/remote-build.sh` 都不带 `--locked`。
- **改法** 重新生成并提交锁文件；CI、`remote-build.sh`、`verify` 全部加 `--locked`；加一步 `cargo metadata --locked`。
- **验收** `cargo metadata --locked` 在干净 clone 上通过；改一个依赖版本后不带 `--locked` 才允许通过。

### C-03 覆盖率

- **严重度** P1 · **规模** S · **依赖** C-01
- **证据** 无 tarpaulin/llvm-cov/c8；`AGENTS.md` 宣称 `npm run metrics` 含覆盖率，实际 `tools/metrics.mjs` 算的是"有派发记录的任务比例"。
- **改法** CI 加 `cargo llvm-cov --workspace --fail-under-lines <当前值>`（先测出基线再定阈值，逐步抬）；把 metrics 里那项改名"派发留痕率"。
- **验收** CI 打印真实行覆盖率；阈值以下构建失败。

### C-04 契约门禁真的读契约

- **严重度** P1 · **规模** M · **依赖** 无
- **证据** `tools/check-contracts.mjs:112` 只提取字段名集合，不比类型/可空性/枚举变体/命令参数名；`contracts/*.md` 除 `fields.json` 外根本没被解析；已漂移但绿的例子：`generated/UpdateReport.ts` 的 `local/latest` 是非空 `number` 而契约写 `number|null`，`TxStatus` 多一个 `marked_for_reversal`。
- **改法** 解析 markdown 表格（或由表格生成 `fields.json`）；生成 TS 命令参数类型替换 `Record<string, unknown>`；比对枚举变体集合；gate 里加 `npm run bindings && git diff --exit-code apps/shell/src/generated`。
- **验收** 改一个字段类型/加一个枚举变体/改一个命令参数名 → gate 红。

### C-05 门禁负向测试

- **严重度** P1 · **规模** M · **依赖** 无
- **证据** `tools/` 下无测试、无违规 fixture；`check-trajectory.mjs` 把 `checked++` 放在跳过分支之前，打印"3/3 可复算"而实际 0 条被复算（假绿灯）。
- **改法** 每条规则配一个违规 fixture + 期望失败；`node --test tools/*.test.mjs` 进 CI；修 `check-trajectory` 的计数并让哈希覆盖 `status/changed_files/violations`。
- **验收** 每条检查都有至少一个 red 用例；违规 fixture 全绿说明检查失效。

### C-06 供应链

- **严重度** P1 · **规模** M · **依赖** 无
- **证据** 无 `deny.toml`/`cargo audit`/`npm audit`/license/SBOM；`gate.yml` 用 `@v4`/`@stable` 浮动标签且无 `permissions:`；无 `rust-toolchain.toml`、无 `.nvmrc`、无 `rust-version`、无 `clippy.toml`、无 `.gitattributes`；锁里有 `sha2 0.10` 与 `0.11` 并存。
- **改法** actions 固定到 SHA + 最小 `permissions`；`rust-toolchain.toml` 固定版本与组件；`cargo deny check`（advisories/bans/licenses/sources）+ `npm audit --audit-level=high`；release 产 CycloneDX SBOM；加 `.gitattributes`（`* text=auto eol=lf`）与 `.nvmrc`。
- **验收** 新增一个高危 advisory 的依赖 → CI 红；`cargo deny` 无 unmaintained/重复版本告警。

### C-07 文档可执行性校验

- **严重度** P2 · **规模** S · **依赖** 无
- **证据** `docs/constitution.md`、`docs/security-boundary.md` 引用了不存在的 `crates/tellerkit-core`；`contracts/shell-api.md` 引用 `src/contracts.ts`；`docs/PLAN.md` 写 3 个检查而实际 5 个；`check-ai-rules.mjs` 只检查标题与 3 个短语存在性——陈旧文档反而"合规"。
- **改法** `check-docs.mjs`：把文档里反引号包裹的仓库路径逐个对 `git ls-files` 校验；删掉标题/短语存在性这类奖励陈旧的检查。
- **验收** 故意写一个不存在的路径 → gate 红。

### C-08 裁决/验收工具跨平台

- **严重度** P2 · **规模** S · **依赖** 无
- **证据** `tools/run-task.mjs:149` `spawn("sh", ["-c", command])`（POSIX only）；`scripts/verify-p0.sh` 需要本机 cargo + python3，未进 gate/CI；部署目标却是 Windows。
- **改法** 改用 `spawn(cmd, {shell:true})`/`execa`；把 `verify-p0` 的字节篡改段改写成 Node，纳入 CI 的 Windows job。
- **验收** 在 windows-latest 上 `npm run task` 与 `verify-p0` 都能跑。

---

## M4 · 减法与债务

### D-01 派发/轨迹机制：接进运行时或删掉

- **严重度** P2 · **规模** S · **证据** `.agents/dispatches/` 55 个任务只有 3 条记录（5.5%），全部集中在同一天；`plan-parallel.mjs`/`check-trajectory.mjs` 无消费方。
- **改法** 二选一：接进 agent 运行时让它"由构造产生"记录，或删除这两个工具与对应门禁。保留 `specs/tasks`（便宜、记录意图）。**不要**保留一个只度量自己的子系统。

### D-02 删除伪门禁

- **规模** XS · **证据** `check-ai-rules.mjs` 的标题存在性/短语存在性检查；`check-boundaries.mjs` 对 `sync::boot_load(` 的纯字符串匹配（`use ... as s; s::boot_load(` 即可绕过）。
- **改法** 删掉前者；后者换成解析调用表达式（swc / syn 小工具），或至少匹配"任意路径形式的 `boot_load`"。

### D-03 代码坏味道清理（一次 PR 即可，勿与功能改动混合）

- **规模** S · **证据** `apps/shell/src-tauri/src/commands.rs:398-438` `dev_only()?;` 重复 5 次；同文件 `reload_from_disk` 把 `recovered` 与 `verify_failed` 赋成同一个值；`App.vue` 的 `showReconcile` 死代码（随 B-09 处理）；`apps/shell/src-tauri/gen/schemas/*.json`（约 300KB 生成物）被提交；`.gitignore` 未忽略 `.agents/worktrees/`。
- **改法** 清理重复调用、修 `recovered` 语义并加断言测试、忽略 `gen/`、把生成物移出版本库。

### D-04 宪法与现实的冲突裁决

- **规模** XS · **证据** `docs/constitution.md` 第 7 条"不做 iframe 沙箱，直到出现不可信第三方包"，但 #26 已落地 iframe 沙箱且业务包定位就是"不可信代码"。
- **改法** 写一条 ADR（`docs/adr/0009-*.md`）记录触发条件已满足、沙箱成为正式约束，并修订宪法第 7 条的措辞。**只改 ADR 与宪法，不改代码。**

### D-05 版本单一来源 + CHANGELOG

- **规模** XS · **证据** `0.1.0` 出现在 `Cargo.toml`、`package.json`、`apps/shell/package.json`、`tauri.conf.json` 四处；无 `CHANGELOG.md`；灰度桶按 app 版本计算，版本错位会让灰度队列洗牌。
- **改法** 由 `gen-bindings.mjs` 从单一源生成其余三处，gate 校验不一致即失败；加 `CHANGELOG.md` 与发布检查单（含客户端回滚步骤）。

### D-06 交接文档：回滚与恢复演练

- **规模** S · **证据** `docs/` 无 runbook、无账本备份/恢复、无日志保留策略。
- **改法** 写 `docs/runbook.md`：升级失败回滚、账本恢复、现场"菜单包被拒"处置、诊断包收集；每项配一次演练记录。

---

## 明确不做（防止范围漂移）

- 不引入 ORM / 连接池 / 消息队列——柜面是单柜员进程，`Mutex<Connection>` 够了。
- 不改 `contracts/**`（除按 A-06/C-04 需要的 `changes/` 提案流程）。
- 不做端侧 AI 实现（宪法第 10 条）。
- 不做多窗口/多屏、不做 Web 版。
- 不为"看起来完整"补伪造数据。
- 不在本机跑 `cargo`（编译走 `scripts/remote-build.sh`）——但 C-01 之后需要重新定义"本地能跑什么"，一并写进 `AGENTS.md`。

---

## 建议的执行顺序（一条直线，避免交叉污染）

1. **C-02**（锁文件，半小时，先让可复现成立）
2. **C-01**（gate 含测试，之后每步都有真实信号）
3. **A-01** → **A-02** → **A-06**（信任链与授权门，一周内可完成三项）
4. **A-03** 第一步（async 止血）→ **A-04**（写前记录）→ **B-01**（异常闭环）
5. **A-05**（安装包/签名/更新器，可与上面并行，单独一条工作线）
6. **A-07**、**A-08**、**B-04**、**B-05**（小项，一批扫）
7. **B-06** → **B-07** → **B-08**（账本与运维）
8. **B-09** → **B-10** → **B-11**
9. **C-03…C-08**、**D-01…D-06**（穿插进行，不要阻塞试点）

> 里程碑口径：完成 1–4 = 可试点（分行内测，无真实资金）；完成 5–8 = 可上生产（仍需 B1–B3 的外部联调）；9 是长期健康度。

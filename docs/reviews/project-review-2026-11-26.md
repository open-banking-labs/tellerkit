# TellerKit 项目评审（2026-11-26）

评审对象：`main` @ `2a744a2`（工作区干净）。

修复状态（2026-11-26 收口）：P0/P1 全部落地并合并到 main——

| 编号 | 问题 | 合并 |
|---|---|---|
| P0 §2.1 | 504/5xx 当业务拒绝 | #64 |
| P0 §2.2 | 审计无哈希链、写失败不阻断 | #73 |
| P1 §3.1 | 幂等签名碰撞 | #74 |
| P1 §3.2 | 幂等索引不跨重启 | #71 |
| P1 §3.3 | 重放不闭环 | #71 |
| P1 §3.4 | 无写前 intent、两步非事务 | #71 + #73 |
| P1 §3.5 | 冲正待确认死锁 | #74 |
| P1 §3.6 | 稳态网络持全局锁 | #77 |
| P1 §3.7 | TLS 被 tcp 绕过 | #65 |
| P1 §3.8 | 包能力按方法名 | #68 |
| P1 §3.9 | 契约门禁只比名字 | #67 |
| P1 §3.10 | 深模块门禁恒绿 | #67 |
| P1 §3.11 | 前端把所有错误渲染成拒绝 | #66 |
| P1 §3.12 | 假后台探测形状违约 | #72 |
| P2 | 空流水号/静默更新/密文降级/沙箱/金额/登出/审计排序/解码/定长/实例锁/配置指纹/设备接口/门禁收紧 | #65 #66 #71 #72 #73 #74 #75 #76 |
| 附带 | 菜单 O(n²)、结构化日志/追踪/急停 | #78 #80 |

---
方法：OCR 原生流水线（45/73 个变更文件）+ 5 路独立只读评审（core / 主壳 / 持久化与传输 / 前端 / 门禁与契约），
关键结论均回源复核；**未在本机执行 cargo / 测试**（遵循 `AGENTS.md` 编译纪律），未接真机与真实主机。

严重度口径：

- **P0**：可直接导致错账 / 重复入账，或某项被依赖的合规控制在 `main` 上实际不存在。
- **P1**：特定条件下导致资金或安全问题，或让"声称已完成"的控制失效。
- **P2**：局部正确性、可观测性与门禁强度问题。

---

## 1. 结论

1. **分层与契约纪律是真实的**：命令层无 `unwrap`、23 条命令与契约逐一对齐、菜单包验签先验后解、金额全程 `i64` 分、五个特权入口统一过 `require_unlocked`、Tauri capabilities 只给 `core:default`。
2. **最严重的一条是资金语义**：`504` 被归成"业务拒绝"并写进幂等缓存，与 `contracts/remote-api.md:111` 直接冲突，柜员按提示重做即可能重复入账（§2.1）。
3. **`main` 明显落后于台账描述**：审计哈希链、审计 fail-closed、IPC 放锁、幂等/重放落盘、写前意图、包能力校验都只存在于**未合并分支**。台账 §8.1 写明了"均未合并"，但 §8.2 的 ✅ 与 README/提交信息读起来像已完成——对只看 `main` 的人，这些控制点不存在（§5）。
4. **门禁强度被高估**：第八道"深模块门禁"以观测模式接入 `gate`，恒 exit 0；契约门禁只比字段名不比类型；`contracts/` 本身没有变更保护。宪法称 `gate` 是唯一裁决入口，README 说它只是静态预检，上线清单又说含 clippy——三套口径并存（§6）。
5. **前端把"命令失败"统一渲染成"已拒绝"**：会话锁定、IPC 故障、审计写失败都会显示成业务拒绝，柜员会重做，而重做会生成新幂等键（§3.11）。

---

## 2. P0

### 2.1 `504` / `5xx` 被归类为业务拒绝，且写入幂等缓存

**位置**：`packages/transport-net/src/lib.rs:319`、`packages/core/src/remote.rs:124`、`packages/core/src/banking.rs:275`、`packages/core/src/tx.rs:344`

**契约**：`contracts/remote-api.md:110-113`——`503` 可重试；`504` **状态不明，标记为待冲正，不得当失败**。

**实际链路**：

```rust
// transport-net:319 —— 非 2xx 一律 Protocol
if !(200..300).contains(&status) {
    return Err(TransportError::Protocol(format!("HTTP {status}：…")));
}
// remote.rs:124 —— Protocol 不可重试
matches!(self, TransportError::Unreachable(_) | TransportError::Timeout)
// banking.rs:275 —— 不可重试 → Rejected
Err(e) => Err(HostError::Rejected(e.detail().to_string())),
// tx.rs:344 —— Rejected = 终态，且 cacheable = true
Err(HostError::Rejected(reason)) => (TxResult::rejected(&format!("主机拒绝：{reason}")), true),
```

后果：

- 网关超时（主机可能已入账）被显示为"主机拒绝"，柜员理解为"没做成"。
- 该结论还会写进幂等索引（`cacheable = true`），同键重试直接返回缓存的"拒绝"，根本不碰主机。
- 柜员改用新键重做 → 服务端无法按幂等键去重 → **重复入账**。

即使响应是 2xx + `{"error": {...}}`，`banking.rs:175` 也只读 `message`、忽略 `code`/`retryable`，同样把 `GATEWAY_TIMEOUT` 当业务拒绝。

**修复**：传输层保留 HTTP 状态码；`5xx` 可重试、`504` 映射为超时/`MarkedForReversal`；按 `error.code` 分支；补 503/504 负向用例（现有 `http_transport_surfaces_error_status` 只测 422，把错误分类固化进了测试）。

### 2.2 审计不可抵赖在 `main` 上不成立

**位置**：`packages/journal-sqlite/src/lib.rs:117-125`（表结构）、`apps/shell/src-tauri/src/app.rs:780-787`

台账 P0-11 声称"哈希链 + 单调序号；审计失败阻断资金操作"，`main` 上两条都不成立：

```sql
CREATE TABLE IF NOT EXISTS audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT, actor TEXT, action TEXT, …);
```

没有 `seq` / `prev_hash` / `hash`，也没有任何校验入口——有 DB 写权限者可 `UPDATE`/`DELETE` 任意行而不被发现。写入侧：

```rust
fn audit(&self, …) {
    let Some(journal) = &self.state.audit else { return; };
    if let Err(e) = journal.audit(&event) { eprintln!("审计写入失败：{e}"); }
}
```

磁盘满/库损坏时资金操作照常返回成功，只落一行 stderr。后果：等保/审计取证所依赖的"只增、可验、失败即阻断"三项全部缺失。

修复分支已存在：`agent/p11-audit-chain`、`fix/audit-fails-closed`。在合并前，台账不应标 ✅。

---

## 3. P1

### 3.1 幂等载荷签名可碰撞

`packages/core/src/tx.rs:471` 用 `format!("{k}={v}")` + `join(",")` 拼签名：

```text
{"a":"b","c":"d"}   → a=b,c=d
{"a":"b,c=d"}       → a=b,c=d
```

两组不同要素得到同一签名，`tx.rs:327` 的冲突检查放行，第二笔拿到第一笔的 `reference` 且不打主机。**修复**：改用带长度前缀的规范序列化（如 `serde_json::to_vec(&BTreeMap)`）后哈希。

### 3.2 幂等索引不跨重启

`tx.rs:281` 的 `idempotent: HashMap<…>` 只在内存；`with_journal`（`tx.rs:306`）只回读 `pending`，账本也没有幂等表（`journal-sqlite` 的 `tx_log`/`tx_queue` 均无签名列）。重启后同键重提会再次调用主机。修复分支：`agent/p09-persistent-state`。

### 3.3 重放路径不闭环

- `replay_one`（`tx.rs:430`）只在 `Committed` 时 `dequeue`；`Rejected` 是终态却把行留在 `tx_queue`，重启后又进 `pending()` 再打主机。
- `replay_pending`（`tx.rs:445`）整段不调用 `journal.record/dequeue`——手动或后台整批重放成功也不出队、不记账，重启后同一批再来一遍。

### 3.4 无写前 intent，记账与入队不是同一事务

`tx.rs:371` 先打主机、后 `record`；`record` 失败只在文案尾部追加"（记账失败：…）"，结论仍是 `Committed`。`record` 与 `enqueue` 是两次独立 autocommit（`journal-sqlite/src/lib.rs:346`、`:364`），中间崩溃会留下"有结论、无队列"或反之。`TxJournal` trait（`tx.rs:196-205`）根本没有 `record_intent`。修复分支：`agent/p08-write-ahead`。

### 3.5 冲正"已授权未确认"没有出口

`reversal.rs:281`：记录已存在且 `host_reference` 为空时再次申请直接返回 `AwaitingConfirmation`；而 `confirm()` 只能由主机回执调用，命令层也没有确认/放弃冲正的入口。于是 `app.rs:512` 注释里"失败时保留…柜员可以重试"是死路——主机超时后这笔异常永远清不掉。`s05_reversal_cannot_repeat` 反而把这个死锁写成了预期。

### 3.6 稳态网络路径仍持全局锁

`commands.rs:319` 的 `submit_transaction` 等仍是同步 `pub fn`，`lock()` 后直到返回都持有 `Mutex<AppState>`；后台 `lib.rs:198-208` 的 worker 同样持锁逐笔打主机。主机超时 3 秒期间，`session_tick`/`session_blur` 及其他命令全部阻塞——锁屏响应被交易拖住，这与"锁屏是授权状态的一部分"的安全属性冲突。修复分支：`feat/ipc-network-out-of-lock`。

### 3.7 release 的 TLS 强制被 `tcp` 端点绕过

`endpoint_config.rs:229-236` 只在 `spec.url()` 以 `http://` 开头时拒绝；`transport: "tcp"`（`:261`）完全不检查是否加密。一份已签名的 `endpoints.json` 可把 `acct.core` 配成 TCP 明文端点并正常启动，界面却按"已强制 TLS"运行。

### 3.8 业务包能力只按方法名，参数完全由包决定

`PackageHost.vue:20-35` 给每个包同一套 `shell` 代理；`sandbox.ts:26` 的白名单是 `["printVoucher","submitTransaction","log"]`；`app.rs:533` 只校验会话解锁，不校验 `txCode` 是否属于该包清单、金额是否越权。包清单里的 `TxMeta`（`pkg.rs:127`）在分发时未被使用。修复分支：`agent/p07-package-capability`（调用来源身份仍受 `changes/017` 契约阻塞）。

### 3.9 契约门禁只比名字，`contracts/` 本身不受保护

`check-contracts.mjs:119` 的 `diff()` 只做字段名 `Set` 差集，类型、可选性、参数签名全部丢弃；命令正则（`:166`）只取 `pub fn (\w+)`。把 `Option<String>` 改成别的类型、或调换命令参数顺序，只要重生成 bindings 并同步 manifest，`gate` 仍绿。CI 的"生成物必须与源码同步"也只 `git diff -- generated/`——同一 PR 改 `fields.json` + 实现 + 生成物即可绕过"契约只读"。

### 3.10 第八道"深模块门禁"恒 exit 0

`tools/check-module-depth.mjs:21` 读 `--strict`，`failures` 只在 `STRICT` 分支填充（`:140-143`）；`package.json:17` 的 `gate` 不带 `--strict`，于是永远打印"观测模式"并成功。宪法第 11 条 / 提交标题"深模块全量落地"因此没有任何本机门禁兜底。即使加 `--strict`，也只看"纯 `pub` 字段"和"`pub fn` 数量"，不查 `lib.rs` 的 12 个 `pub mod` 平铺与 `pub use` 内部实现。

### 3.11 前端把所有命令错误渲染成"已拒绝"

`TxPanel.vue:75-81` 的 `catch` 无条件构造 `status: "rejected"`。会话已锁定、审计/账本故障、IPC 失败、`offlineCapable` 被拒都会显示红色"已拒绝"；柜员重做时 `crypto.randomUUID()` 生成新幂等键（`:65`），服务端按新键处理——与 §2.1 叠加即重复入账。修复：把"命令失败"与"业务拒绝"分成两种 UI 状态，失败态提示"状态不明请查账"，并禁止用新键直接重试。

### 3.12 假后台的菜单探测响应违反契约，端到端用例假阳性

`fake-bank/src/lib.rs:207-211` 返回扁平 `shell_compat_min/max`；契约 `contracts/remote-api.md:60` 要求 `{"latest":7,"shell_compat":{"min":1,"max":1}}`，而 `MenuVersionProbe`（`banking.rs:18-22`）也只认扁平字段。按契约实现的真后台会让 `probe_menu()` 反序列化失败、启动 fail-closed 起不来；测试只断言 `latest()`，恰好被错误形状喂绿。

---

## 4. P2

**资金与状态**

- 空冲正流水号被当作确认完成：`banking.rs:239` 接受 `""`，`confirm()` 写 `Some("")`，异常从清单消失（`reversal.rs:247`）。
- 静默更新不核对下载包版本：`sync.rs:197` 只 `install`（验签），提示文案用探测版本；服务端下发旧包会"更新"到更低版本。

**安全**

- 账本密文可降级：`cipher.rs:130` 把无 `v1:` 前缀的数据当旧明文原样返回，能写 `tx.db` 的人改要素后重放不会被发现。
- 沙箱消息未校验来源：`sandbox.ts:98` 的 iframe `message` 监听只查 `__pkg`，不查 `event.source === parent`；父侧 `postMessage(…, "*")`（`:209`）。
- 配置公钥只来自 `TK_CONFIG_PUBKEY`（`config_sig.rs:55`），启动摘要只报菜单公钥指纹；能改环境变量的人可整体替换信任根。

**传输与协议**

- HTTP 读体 `read_to_end`（`transport-net:250`），不按 `Content-Length` 收尾、不支持 chunked；服务端不按 `Connection: close` 关闭时会等到读超时，把完整响应判成 `Timeout`。
- 解码失败不改 `all_retryable`（`remote.rs:352-361`），主端点已受理但响应不可解析时会拿同一幂等键打备端点。
- 定长报文数字解析失败静默降级为字符串、文本 `from_utf8_lossy`（`remote.rs:560-575`），上层无法区分"主机给了 0/乱码"。

**审计与可观测**

- 重放审计结果硬编码 `"ok"`（`app.rs:603`），有 `gave_up` 也记成功。
- 审计不可用时 `audit_tail` 返回 `Ok(vec!["审计日志不可用…"])`（`app.rs:754-757`），把故障混进数据通道。
- 审计尾部按秒级 `at` 排序（`lib.rs:186`），时钟回拨会漏最新事件；应改单调 `seq`。
- `login`、`reload_from_disk`、`tx_reconciliation` 直接透传内部错误（`app.rs:389`、`:451`、`:728`），与 `contracts/ipc-commands.md:38` 的"人可读文案"不符。

**前端**

- `Math.round(yuan * 100)`（`TxPanel.vue:19-24`）：`1.005` 元被截成 1.00 元。
- 登录失败只写 `logs`，登录页看不到原因（`App.vue:179`）。
- 登出先清 `sessionTimer`/`locked` 再 `await api.logout()`（`App.vue:186-198`），失败后界面显示未锁定且不再自动锁屏。
- `refreshPending`/`replay`/`printVoucher` 无 catch（`TxPanel.vue:35-99`），失败静默。
- 剪贴板守卫漏掉备注框（`TxPanel.vue:121`）。

**门禁**

- `check-bank-names.mjs:98` 的 `allow` 只取 `path`、不校验 `reason`；拼音、加空格写法漏检；`fixtures/`、`generated/` 与大文件跳过。
- `check-tasks.mjs:114` 在 `.specify/specs/` 为空时 exit 0；`:247` 的"提案"检查只要 `tasks.md` 出现"提案"二字即短路。
- `check-trajectory.mjs:74-82` 的豁免文件同 PR 可改、理由不验真伪。
- `check-boundaries.mjs` 的命令层规则按文件名 `commands.rs` + 固定调用形态匹配（`paths.mjs:75`、`:74`），改名/别名/`#[path]` 即失效；架构 §8 的"已实现"标注偏强。
- `gen-bindings.mjs:33` 生成前不清目录，`ts-rs` 不再导出的旧类型会长期"同步"通过。

**账本**：`journal-sqlite` 的 `open()` 无 `locking_mode=EXCLUSIVE` 或实例探测；两个进程可各自载入同一 `tx_queue` 重复重放（主壳是否另有实例锁未穷尽核实，P1-2 分支待合并）。

---

## 5. 声称 vs 事实（只看 `main`）

| 控制点 | 文档声称 | `main` 实际 | 修复所在分支 |
|---|---|---|---|
| P0-11 审计哈希链 | 哈希链 + 单调序号 ✅ | 表无 `seq`/`hash`，无校验 | `agent/p11-audit-chain` |
| P0-11 审计阻断 | 写失败阻断资金 ✅ | `eprintln!` 吞掉 | `fix/audit-fails-closed` |
| P0-10 IPC 放锁 | 稳态网络不持锁 ✅ | 全同步命令，网络在锁内 | `feat/ipc-network-out-of-lock` |
| P0-9 状态落盘 | 幂等/异常/冲正/重放全部落盘 ✅ | 幂等表与重放计数仅内存 | `agent/p09-persistent-state` |
| P0-8 写前意图 | 先 intent 再发远端 ✅ | 无 `record_intent`，两步非事务 | `agent/p08-write-ahead` |
| P0-7 包能力 | 按包而非方法名 ✅ | 按方法名，参数全由包定 | `agent/p07-package-capability` |
| 宪法 11 深模块 | 本机门禁 ✅ | 观测模式，恒 exit 0 | — |
| README 回退 | "篡改后回退上一版" | 只回退服务端；断网即 `Offline` | — |
| P1-2 实例互斥 | 第二实例被拒 ✅ | journal 层无排他锁 | `feat/single-instance` |

> 台账 §8.1 已写明这些 PR "均未合并"，所以不算说谎；但 §8.2 的 ✅、提交信息与 README 的读法会让人误以为 `main` 已具备。建议要么合并，要么把 ✅ 改成"分支已完成，未合并"。

---

## 6. 门禁强度

| 门禁 | 声称 | 实际 |
|---|---|---|
| `check-boundaries` | 跨层/编排禁令 | 静态文本扫描，多数规则实在；命令层按文件名 + 调用形态 |
| `check-contracts` | 字段/命令双向比对 | 仅字段名集合 + 命令名；不比类型/参数/错误码；`contracts/` 无变更保护 |
| `check-tasks` | 场景全覆盖、契约任务需提案 | 空集合 exit 0；"提案"字样短路 |
| `check-ai-rules` | 宪法称查安全表述 | 已改为只查文件存在（宪法未同步） |
| `check-trajectory` | 真复算 | 真复算，但豁免可自登记 |
| `check-bank-names` | 全仓 0 命中 | 字面匹配，allow 不校验理由，有跳过目录 |
| `check-version` | 单一版本源 | 真实有效 |
| `check-module-depth` | 深模块 | 观测模式，恒 exit 0 |
| `cargo fmt` | 格式 | 真实有效 |

另：宪法第 5 条说 `gate` 是唯一入口，README 说它是静态预检、合并看 CI，`docs/production-readiness.md:4,37` 又说"门禁与测试以 `npm run gate` 退出码为准""六道门禁（含 clippy）"。同一仓库三套裁决口径，`changes/015` 仍未批准。

---

## 7. 建议顺序

1. **先修 §2.1**：错误分类下沉到传输/路由层，`504` → 状态不明，`5xx` 可重试，停止缓存"拒绝"。补 503/504 用例（含"已受理但响应不可解析"）。
2. **合并或降级台账**：§5 的六项要么按序合并 PR（`#38→#43→#44…`），要么把 ✅ 改回"分支完成、未合并"。不要让人按 README 判断 `main` 的生产能力。
3. **状态机缺口**：幂等签名、跨重启幂等、重放出队、写前 intent、冲正出口——与 `agent/p09`、`p08` 合并后统一复核一遍。
4. **门禁收口**：`check-module-depth --strict` 接进 `gate`；契约门禁加类型比对；CI 增加 `contracts/**` 变更须带 `changes/` 提案的检查。
5. **前端错误语义**：区分"命令失败"与"业务拒绝"，金额改字符串拆分，登录/登出/打印失败可见。
6. P2 批量清理，优先安全项（cipher 降级、沙箱 source、配置公钥指纹）。

---

## 8. 方法与覆盖

- **OCR 原生流水线**：`ocr review --commit HEAD --analysis`，模型 `tencent-cloud/hy4-flash`（magpie 本地网关）。45/73 个文件，20 条 finding，12m53s，`review_status=complete`。完整原文见 [OCR 附录](project-review-2026-11-26-ocr.md)。
- **5 路评审**：core 资金状态机；`src-tauri` 命令层与启动链；`journal-sqlite` / `transport-net` / `fake-bank`；Vue 前端与沙箱；`tools/` 门禁、`contracts/` 与文档真实性。
- **复核**：P0 与主要 P1 均按 `file:line` 回源核对（错误分类链、签名碰撞、重启幂等、重放出队、冲正死锁、TLS 分支、包能力、两道门禁、前端 catch、假后台形状、cipher 前缀）。
- **未覆盖**：本机不跑 cargo/test（项目纪律，编译在远端）；未跑 Tauri 真机与真实主机；未验证分支保护设置；OCR 的 `code_search` 首轮因本机默认 git 无 PCRE 失败，改用 Apple `/usr/bin/git` 垫片后完成，仍有少量模型工具调用错误（不存在的路径、非法正则）。

## 9. 值得保留的部分

- 命令层与契约的高度一致（23 条命令、DTO 字段、错误码抽查均对齐）。
- 菜单/业务包"先验签后解码"、`tmp → fsync → rename` 的安装顺序正确。
- 金额整数分、`checked_shl`/`saturating_*`、会话时钟回退处理。
- 五个特权入口的 `require_unlocked` 无遗漏，且只信 Rust 侧状态；后台重放复用同一闸门。
- capabilities 只有 `core:default`，CSP 未放开外部连接；Vue 侧未发现权限判断、`v-html`、token 落盘。

---

## 10. OCR 原生结果（20 条）

按评审技能要求分两组，保留 OCR 顺序；每条都在源码上做了复核。原文与变更影响分析见
[OCR 附录](project-review-2026-11-26-ocr.md)。

### 已复现（17 条）

| # | 级别 | 结论 | 位置 |
|---|---|---|---|
| 1 | low | `StartupFatal(pub Option<String>)` 是元组结构体，深模块门禁只扫花括号结构体，公开字段漏检 | `bootstrap.rs:47` |
| 3 | low | `reload()` 返回 `ReloadOutcome`，但 crate 根未导出该类型，外部无法命名 | `core/src/lib.rs:24-28` |
| 4 | medium | pre-commit 读工作区文件而非索引：先 stage 带银行名的文件、再清理工作区即可绕过 | `.githooks/pre-commit`、`check-bank-names.mjs:83-113` |
| 5 | medium | 深模块门禁默认观测模式（同 §3.10） | `check-module-depth.mjs:21,140` |
| 6 | medium | 边界门禁正则要求 `模块::` 段；`use tellerkit_core::boot_load` 后裸调用可绕过 | `check-boundaries.mjs:74` |
| 7 | medium | check-ai-rules 已删除短语检查，宪法第 3 条仍在声称（同 §6） | `check-ai-rules.mjs:91-96` |
| 9 | medium | 记账失败文案先写进 `TxResult.message` 再入幂等缓存，重复提交永久带"记账失败" | `tx.rs:370-372` |
| 10 | medium | `replay_one` 出队失败只追加文案，结果仍 `Committed`；重启后同一笔再次重放 | `tx.rs:430-436` |
| 11 | medium | 后台 worker 每 tick 调 `replay_pending`，未登录时每 60s 写一条 `denied` 审计（actor=`unknown`） | `lib.rs:195-208`、`app.rs:300-317` |
| 12 | medium | `run_once` 只看 `status`，丢弃 `result.message`，账本写失败无法上报/审计 | `replay.rs:418-425` |
| 13 | medium | 契约门禁结构体索引按名字；`StatusRow`/`Reconciliation` 在 `app.rs` 与 `commands.rs` 重名，后者覆盖前者 | `check-contracts.mjs:103` |
| 15 | medium | `PrintReceipt` 字段私有且无构造器，外部 `Printer` 适配器无法构造返回值 | `device.rs:116-119` |
| 16 | medium | `Voucher` 只有 setter、没有 `reference()`/`fields()` getter，适配器无法渲染凭证内容 | `device.rs:82-110` |
| 17 | high | `pending()` 读回失败只 `eprintln!`，启动后队列为空、旧挂起永不重放且无告警（同 §3.2/§3.3） | `tx.rs:306-313` |
| 18 | high | `login` 无条件把 session 覆盖成 unlocked，锁定态可被重新登录解锁；当前无凭据校验，定性取决于 IAM | `app.rs:404-408` |
| 19 | high | `require_tls` 只查 `http://`，`tcp` 端点不受约束（同 §3.7） | `endpoint_config.rs:231-236` |
| 20 | high | 自动锁屏只由前端 `session_tick`/`session_blur` 触发；前端停止 tick 就永不锁屏，Rust 侧没有定时器 | `app.rs:660-680` |

### 待确认（2 条）

| # | 级别 | 结论 | 缺什么 |
|---|---|---|---|
| 8 | medium | `check-trajectory` 在 `.agents/dispatches` 缺失时提前成功；registry 解析异常/`null` 未处理 | 未逐行读该分支，需构造空目录与坏 JSON 的只读用例 |
| 14 | medium | debug 构建下丢掉了"配了公钥就必须签"的默认（`release \|\| require_signed.unwrap_or(false)`），`TK_CONFIG_PUBKEY` 存在但开关未设时不强制签名 | release 不受影响；需维护者确认 debug 是否要保留该默认 |

### 已反驳（1 条）

- #2（low）称 `check-contracts.mjs` 的 `rel` 是死导入——**不成立**：`rel` 在 `check-contracts.mjs:103` 的 `rustStructs.set(name, { file: rel(file) })` 中使用。

### OCR 变更影响分析要点

- 明确记录本次提交的行为变更：release 缺端点/未签名/明文 → 拒绝启动；非法开关值 → 报错；缺 `TK_MENU_PUBKEY_HEX` → 构建失败；锁定/未登录 → 五个资金类入口被拒且不触达主机。
- 指出兼容性代价：部署必须随包提供 `endpoints.json` + `.sig`、设置 `TK_CONFIG_PUBKEY`、构建期注入公钥。
- 同时指出账本错误处理被弱化：`take_journal_error()` 被移除，写失败混进 `TxResult.message`，启动读回失败只剩 `eprintln!`（与 §3.3/§3.4 一致）。

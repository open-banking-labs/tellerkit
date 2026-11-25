# 架构：三条边界与四层模型

> 状态：目标架构（2026-11-25 起）。当前代码正在按本文迁移，差距见 §7。
> 相关：[docs/rust-style.md](rust-style.md)、[docs/constitution.md](constitution.md)

## 0. 一句话

**所有逻辑必须先能用一个接口描述清楚，然后才允许写实现。**
读代码的人从接口就能看到"这条链路要做什么"，不需要钻进实现里拼凑。

## 1. 三条边界

这个系统里有三类通信，它们的失败模式、契约形态、演进节奏完全不同，
**必须分开定义、分开测试、分开版本化**：

| # | 边界 | 两端 | 失败模式 | 契约在哪 |
|---|---|---|---|---|
| ① | 桌面内部组件 | Vue 组件 ↔ 客户端门面 | 前端状态错乱 | `apps/shell/src/platform/client.ts`（TS 接口） |
| ② | Tauri ↔ Rust（本地 IPC） | WebView ↔ 主壳 | 命令未注册、DTO 漂移 | `contracts/ipc-commands.md` + `fields.json` + ts-rs 生成 |
| ③ | 桌面 ↔ 服务端 | 主壳 ↔ 后台 | 超时、幂等、版本不兼容 | `contracts/remote-api.md` |

把三者混在一起的典型症状：命令名写在前端、远端 DTO 和 IPC DTO 是同一套类型、
超时处理散落在命令里。当前代码有其中两处（§7）。

## 2. 四层模型

```
┌─────────────────────────────────────────────────────────────┐
│ L1 · 界面  apps/shell/src/**                                 │
│     只依赖 ShellClient 接口，不知道 invoke、不知道命令名        │
└───────────────────────────┬─────────────────────────────────┘
                            │ ① 组件 ↔ 门面（TS 接口）
┌───────────────────────────▼─────────────────────────────────┐
│ L2 · IPC  apps/shell/src-tauri/src/commands.rs               │
│     只做：加锁 → 调用例 → DTO 映射。**不写编排、不做判断**      │
└───────────────────────────┬─────────────────────────────────┘
                            │ ② invoke（命令 + DTO，契约门禁）
┌───────────────────────────▼─────────────────────────────────┐
│ L3 · 应用  apps/shell/src-tauri/src/app.rs                   │
│     用例编排：读哪个端口、按什么顺序、失败怎么降级              │
│     只依赖"端口 trait"，不知道实现是 mock 还是 http            │
└───────────────────────────┬─────────────────────────────────┘
                            │ 端口（trait）
┌───────────────────────────▼─────────────────────────────────┐
│ L4 · 领域  packages/core/**                                  │
│     纯逻辑：状态机、剪枝、验签、退避、锁屏判定                 │
│     **不依赖文件系统、网络、时钟、Tauri**                      │
└─────────────────────────────────────────────────────────────┘

适配器（实现端口，可替换）：
  packages/journal-sqlite   → TxJournal
  apps/shell/src-tauri/adapters/remote_mock.rs  → RemoteBanking（当前）
  apps/shell/src-tauri/adapters/remote_http.rs  → RemoteBanking（目标，骨架已留）
```

### 依赖规则（单向，可检查）

| 层 | 允许依赖 | 禁止 |
|---|---|---|
| L1 | `platform/client.ts` 的接口、Vue | `invoke`、命令名字符串、DTO 之外的 Rust 概念 |
| L2 | L3 用例、DTO | 直接调 L4 领域函数做编排、读写文件、发请求 |
| L3 | L4 领域、端口 trait | 具体适配器（`SqliteJournal`、`reqwest`）、Tauri 的 `State` |
| L4 | 仅 `serde`/`thiserror` 等纯库 | IO、时钟、环境变量、Tauri |

`check-boundaries` 已强制其中两条（前端不得判断权限、core 不得依赖 tauri）。
新增规则见 §8。

## 3. 接口清单：一条链路看完

**这是本文最重要的表。** 任何功能，先在这里占一行，再写代码。

| 能力 | L1 接口（TS） | L2 命令 | L3 用例 | L4 领域 | 出站端口 | ③ 远端端点 |
|---|---|---|---|---|---|---|
| 启动 | `client.boot()` | `boot` | `ShellApp::boot` | `MenuTree`/`Manifest` | `MenuRepository`·`RemoteBanking` | `GET /menu/version`·`GET /menu/pack` |
| 登录 | `client.login(role)` | `login` | `ShellApp::login` | `PermSnapshot`·`filter_visible` | `RemoteBanking` | `POST /auth/login` |
| 登出 | `client.logout()` | `logout` | `ShellApp::logout` | — | — | — |
| 菜单 | `client.currentMenu()` | `current_menu` | `ShellApp::current_menu` | `filter_visible` | — | — |
| 提交交易 | `client.submitTransaction(req)` | `submit_transaction` | `ShellApp::submit_transaction` | `TxEngine` | `RemoteBanking`·`TxJournal` | `POST /tx/submit` |
| 打印凭证 | `client.printVoucher(v)` | `print_voucher` | `ShellApp::print_voucher` | `Voucher`/`Devices` | `Devices` | — |
| 重放 | `client.replayPending()` | `replay_pending` | `ShellApp::replay_pending` | `ReplayScheduler` | `RemoteBanking`·`TxJournal` | `POST /tx/submit` |
| 待重放数 | `client.txPendingCount()` | `tx_pending_count` | `ShellApp::tx_pending_count` | `TxEngine` | — | — |
| 版本探测 | `client.checkUpdate()` | `check_update` | `ShellApp::check_update` | `plan_load` | `RemoteBanking` | `GET /menu/version` |
| 审计 | `client.auditTail(n)` | `audit_tail` | `ShellApp::audit_tail` | — | `TxJournal` | — |
| 会话 | `client.sessionTick()` 等 | `session_*` | `ShellApp::session_*` | `SessionGuard` | `Clock` | — |
| 业务包 | `client.packageModule(r)` | `package_module` | `ShellApp::package_module` | `BizPackage` | — | — |

**读法**：想知道"提交交易"怎么走，看一行就知道它穿过了 UI 接口、一条 IPC 命令、
一个用例、一个领域状态机、两个出站端口和一个远端端点。**不需要读实现。**

## 3.1 远端通信：三层分离

边界 ③ 不是一个 `HttpClient`，而是三层可独立替换的组合：

```
应用层调用   router.call(Request { service: "acct.core", op: "submit_tx", body })
                │
                ▼
        Router（core::remote）  按 service 找端点，按 priority 主备切换，聚合失败原因
                │
        ┌───────┴────────┐
        ▼                ▼
   Codec            Transport
   （报文长什么样）    （怎么送过去）
   JsonCodec        TcpTransport（短连接 + 长度前缀）
   FixedWidthCodec  HttpTransport（POST + Content-Length）
                    MockTransport（测试）
```

**为什么必须分三层**：

| 变化 | 只改哪层 |
|---|---|
| 后台从 REST 换成 TCP 自定义报文 | Transport + Codec，Router 与业务不动 |
| 同一服务加一个备用机房 | 只加一条 `Endpoint`（改 priority） |
| 报文从 JSON 改成定长 | 只换 Codec |
| 某个业务模块走独立专线 | 给它单独的 service 名与端点 |

### 一个业务模块对应多个服务端

路由键是 **service**，不是模块名。所以：

```rust
// 账户模块：主用行内核心，备用同城灾备
Endpoint { service: "acct.core",  addr: Http{..},   priority: 0, .. }
Endpoint { service: "acct.core",  addr: Tcp{..},    priority: 1, .. }   // 换协议也能备
// 同一个模块还要调卡系统：另一个 service，不同传输与编解码
Endpoint { service: "card.auth",  addr: Tcp{..},    priority: 0, codec: FixedWidth, .. }
```

`Router::call` 只在**可重试**失败时切换（连接失败/超时），
协议错误与服务端拒绝不切换——换台机器照样拒绝，切换只会把请求打到不该去的地方。

### 编解码的两种形态

- `JsonCodec`：`{"op":..., "payload":...}`，REST 后台；
- `FixedWidthCodec`：`[4 字节长度][按 layout 拼接的字节定长字段]`，老系统报文。

定长**按字节对齐**（不是字符数）。这条踩过坑：`format!("{:<8}", "柜面")` 按字符补齐，
CJK 会算出 12 字节而不是 8，报文长度对不上。已加回归用例
（`fixed_width_roundtrip_pads_numeric_and_text`）。

### 端到端验证

`packages/fake-bank` 是一个**真的 TCP 服务端**，`tests/end_to_end.rs` 用它跑通：

```
RoutedBanking → Router → TcpTransport → 假后台（真 socket）
```

覆盖：三个服务各自路由、二进制下载走 RawCodec、业务拒绝映射为 `Rejected`、
主端点不可达时自动切到备用端点。

## 4. 三类 DTO 必须分开

同一笔交易在三个边界上形状不同，混用会让某一侧的演进绑架另外两侧：

| 边界 | 类型家族 | 例子 | 谁定义 |
|---|---|---|---|
| ① | 视图模型 | `MenuRow`、`LogEntry` | 前端（`ui-types.ts`） |
| ② | IPC DTO | `BootInfo`、`TxResult`（生成到 `generated/`） | `commands.rs` + ts-rs |
| ③ | 远端 DTO | `RemoteTxRequest`、`RemoteTxAck` | `contracts/remote-api.md` |

**当前问题**：② 直接复用了 ④ 的领域类型（`TxResult`、`MenuView`）。
后果：改了领域字段就改了 IPC 契约。目标是把领域类型与 IPC DTO 之间放一层映射，
映射是纯函数、可单测。

## 5. 三个问题：抽象的准入协议

对**粗粒度**（层/端口）和**细粒度**（类型/函数）都要问，答案写进 §6 的表。

1. **这里能抽象出来一层吗？** —— 存在被替换的实现？存在第二个调用方？存在跨边界？
2. **抽象后收益大吗？** —— 换实现时改动点从 N 变 1？测试不用起真环境？链路可读？
3. **不抽象是不是太局限？** —— 换语言/换协议/换存储时会不会整片重写？）

判据（三者取一即值得抽）：
- 有**两个以上实现**（mock/真实、文件/数据库）
- 跨**边界**（IPC、网络、文件系统）
- 有**独立演进节奏**（如业务包格式、远端协议）

不满足时抽象就是负债：多一层跳转，读者要多跳一次才能看到真实行为。

## 6. 已做的抽象决策

### 粗粒度（层与端口）

| 对象 | 抽了吗 | 三问结论 |
|---|---|---|
| 远端后台 | ✅ `RemoteBanking` | 有 mock/http 两个实现、跨边界、独立演进 → 收益大 |
| 时钟 | ✅ `Clock` | 锁屏/退避/审计时间都要可注入，否则测试只能 `sleep` → 收益大 |
| 账本 | ✅ `TxJournal` | sqlite 与内存两个实现、跨边界 → 收益大 |
| 设备 | ✅ 四个 trait | 型号可换、真机/mock → 收益大 |
| 菜单包存储 | ✅ `MenuStore` | 文件系统跨边界；已有 db 替代可能 → 保留 |
| **每个命令**建 use case 结构体 | ❌ | 一个命令一个 struct 只是把函数改个形状，无第二实现 → 不抽 |
| **泛型化** `TxEngine<J: TxJournal>` | ❌ | `Box<dyn>` 已足够；泛型会传染到所有调用点 → 不抽 |

### 细粒度（类型与函数）

| 对象 | 抽了吗 | 三问结论 |
|---|---|---|
| `DeviceStatus` 区分未连接/故障 | ✅ | 柜员处置不同 → 收益大 |
| `TxStatus` 三态不设 `Unknown` | ✅ | "不知道"不是终态 → 收益大 |
| `ReversalError::NotReversible` | ✅ | 待推进与待冲正是两个方向 → 收益大 |
| 单独 `Amount` newtype | ❌ | 只有 `i64` 一处语义，无单位混用风险 → 抽了是噪音 |
| 每个错误一个 struct | ❌ | 枚举变体已表达；拆 struct 只增加 import → 不抽 |

## 7. 现状差距（迁移清单）

| 项 | 现状 | 目标 | 严重度 |
|---|---|---|---|
| 用例层 | ✅ `app.rs::ShellApp`；`commands.rs` 只剩加锁 → 用例 → DTO 映射 | — | 完成 |
| 远端端口 | ✅ `RemoteBanking` 已接线；底层已有 `Router` + `Transport` + `Codec` 三层 | 写 `RoutedBanking`（把 `RemoteBanking` 落到 Router 上） | 中 |
| 时钟 | ✅ 注入到 `AppState.clock`，用例层只读端口；测试用冻结时钟验证 | — | 完成 |
| 前端门面 | ✅ `platform/client.ts` 接口 + `tauri.ts`/`mock.ts` 两个实现；`check-boundaries` 强制"只有 tauri.ts 能碰 invoke" | — | 完成 |
| IPC DTO 与领域类型 | ⚠️ 混用 | 保留混用但**记录理由**：DTO 已由 ts-rs 生成并门禁，加映射层会多一层无收益转换 | 低 |
| 远端契约 | ✅ `contracts/remote-api.md`（含幂等、504 状态不明、错误模型、端点绑定表） | 补 OpenAPI 生成（可选） | 完成 |
| 远端三层 | ✅ `remote.rs` + `transport-net` + `banking.rs`（`RoutedBanking`） | TLS、连接池 | 中 |
| 端点配置 | ✅ `endpoint_config.rs`（`TK_ENDPOINTS` / `endpoints.json` → Router），非法配置回退 mock | — | 完成 |
| 配置完整性 | ✅ `config_sig.rs`：Ed25519 分离签名，先验签后解析；配了公钥就必须签 | — | 完成 |
| 传输加密 | ⚠️ 失败关闭：拒绝 `https://`（缺 TLS 后端）、`TK_REQUIRE_TLS=1` 时拒绝明文 | 按 `changes/009` 引入 rustls | 高 |

## 8. 可检查的规则（要加进门禁）

| 规则 | 检查方式 |
|---|---|
| L1 不得出现命令名字符串 | ✅ 已实现：`src/**` 禁止 `@tauri-apps/api` 与 `invoke(`，只有 `platform/tauri.ts` 例外 |
| L2 不得直接调领域函数做编排 | ✅ 已实现：`check-boundaries` 禁止 `commands.rs` 出现 `sync::boot_load(`/`filter::filter_visible(`/`pkg::scan_dir(`/`run_once(`/`.ensure_allowed(` |
| L3 不得依赖具体适配器 | ⚠️ 待补：`app.rs` 目前仍直接调 `state.tx`/`state.devices`（它们是端口实例，可以接受，但需在评审时确认） |
| L4 不得引入 IO | 已有：core 不得依赖 tauri；扩展为禁止 `std::fs`/`std::net` |

## 9. 反模式（评审直接打回）

1. 在 `commands.rs` 里写 `if 断网 { ... } else { ... }` 之类的业务分支。
2. 前端组件里出现 `invoke("xxx")` 字面量。
3. 领域层读时钟或环境变量。
4. 为"将来可能换实现"提前抽接口，而没有第二个实现。← 与本文 §5 判据冲突时，以判据为准。
5. 端口 trait 里出现具体类型（`reqwest::Error`、`rusqlite::Connection`）。

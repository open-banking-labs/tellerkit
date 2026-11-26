# 安全评审 · 2026-11-25

> 评审对象：HEAD `155e0fa`（#30 README）。评审人视角：外部安全评审，只读代码与文档。
> 结论：威胁模型是对的，**但五个关键控制点在生产上不成立**，且都不在
> [production-readiness.md](../production-readiness.md) 的 B1–B8 里。
> 可执行待办见 [.specify/specs/040-security-hardening/](../../.specify/specs/040-security-hardening/spec.md)。

## 1. 一句话

签名、TLS、配置验签、原子替换、快照不落盘、拒绝离线挂起——这些"点"都做了；
缺的是**把它们连成"生产上真的生效"的链**：公钥是演示的、配置默认不签名、
锁屏不拦命令、包能力声明没人执行、审计不能当证据。

## 2. 评审范围

- 四层骨架：`apps/shell/src`（Vue 只渲染）、`src-tauri/{commands,app,lib}.rs`、`packages/core`。
- 适配器：`packages/{transport-net,journal-sqlite,fake-bank}`、`packages/biz-demo`。
- 契约与文档：`contracts/**`、`docs/{security-boundary,production-readiness,constitution}.md`、`changes/**`。
- 构建与门禁：`package.json`、`tools/**`、`.github/workflows/gate.yml`、`scripts/remote-build.sh`。

不在范围：远端主机/账务核心、行内 IAM、真实外设固件、Windows 部署与域策略。

## 3. 已经具备的控制（不要重复造）

| 控制 | 位置 | 评价 |
|---|---|---|
| 菜单包签名 + 拒绝尾随字节 | `packages/core/src/pack.rs` | 结构正确，Ed25519 over sha256(header‖payload) |
| 验签失败绝不落盘 + 原子替换 | `packages/core/src/store.rs` | tmp → fsync → rename，正确 |
| 端点配置先验签后解析 | `apps/shell/src-tauri/src/endpoint_config.rs` | 顺序正确，缺公钥即拒绝 |
| TLS：rustls + 内置根 + 主机名校验 | `packages/transport-net/src/lib.rs` | 正确，但只覆盖 http 端点、无吊销（B6 已知） |
| 权限快照不落盘 | `packages/core/src/perm.rs` | 正确 |
| 主机无幂等则拒绝离线挂起 | `app.rs::submit_transaction` | 正确，且与 lifecycle 不变量 7 一致 |
| 业务包 iframe：allow-scripts + CSP connect-src none | `apps/shell/src/platform/sandbox.ts` | 方向对，但见 P0-4 |
| Tauri capabilities 只给 core:default | `apps/shell/src-tauri/capabilities/default.json` | 正确，无 fs/shell 权限 |
| dev 命令 debug 门禁 | `commands.rs::dev_only` | 可用，但 release 里仍注册 |

## 4. P0：控制点在生产上不成立

### P0-1 签名公钥是演示公钥，且没有生产注入路径

- 证据：`apps/shell/src-tauri/src/lib.rs:222` 与 `app.rs:164` 都用
  `tellerkit_core::mock::demo_verifying_key()`；`DEMO_SEED` 在 `packages/core/src/mock.rs` 公开；
  `tools/mtree-tool` 的 `pubkey` 子命令直接打印它。全仓没有 `option_env!`/`env!` 取生产公钥。
  `lib.rs:221` 的注释"生产环境由构建配置注入"在代码里不存在。
- 攻击路径：任何人拿到仓库 → 用 demo 私钥签 `.mtree` / `.bizpkg` → 放进 appdata →
  菜单注入 + 包内任意 JS（沙箱内）+ 任意权限码入口。
- 影响：签名链在生产等于零。B8 只覆盖"配置签名私钥保管"，B3 只覆盖 IAM，都没有覆盖这条。
- 建议：`option_env!("TK_PACK_PUBKEY")` 构建期注入，缺失即拒绝启动；启动信息与审计记录公钥指纹；
  CI 门禁"release 构建不得包含 demo 公钥"。

### P0-2 端点配置默认不要求签名，失败还回退 mock（fail-open）

- 证据：`endpoint_config.rs:113` 的逻辑是"设了 `TK_CONFIG_PUBKEY` 才要求签名"；默认没有 → 不签名也接受。
  `apps/shell/src-tauri/src/lib.rs:146` 解析/验签失败时 `eprintln` 后回退 `MockRemote`，只加一条启动 note。
  `TK_REQUIRE_TLS` 默认关，`endpoint_config.rs:156` 只拦 `http://`。
  `MockRemote::supports_idempotency()` 返回 `true`（`ports.rs`），所以回退后离线挂起也被允许。
- 攻击路径：本机可写 `endpoints.json` 的人改地址或让其校验失败 → 客户端进入演示模式，
  界面一切正常、任意角色可登录、交易"成功"只落本地；或把流量指向自己的服务器拿走账号与要素。
- 影响：柜员可能在"假系统"上办真业务；这是可用性优先于真实性的默认值，必须反过来。
- 建议：生产 fail-closed（无签名配置/无效配置 = 拒绝启动）；TLS 强制；
  mock 只在显式 `TK_DEMO=1` 且界面有不可关闭标识时启用；所有降级必须上 UI，不能只 eprintln。

### P0-3 锁屏是展示层，命令层零校验，后台重放不看锁

- 证据：`app.rs::submit_transaction`（:357）、`print_voucher`（:447）、`request_reversal`（:325）
  都不检查 `snapshot` / `session.locked`；`commands.rs` 只加锁转发。
  `lib.rs:187` 的重放线程只查 `snapshot.is_none()`。
  锁由前端 1s tick + mousemove 驱动（`App.vue:409-430`），tick 异常被吞；`LockScreen.vue` 只是覆盖层，
  底层 iframe 仍在 DOM 里并继续接受 postMessage。
- 影响：离开工位后（或锁屏中）业务包/devtools/被注入脚本都能以柜员身份提交交易、打印凭证，
  审计 actor 仍是柜员。违反 `changes/007` 自己写的"锁屏状态必须由 Rust 持有"。
- 建议：core/app 层统一 `ensure_active_session()`（有快照 + 未锁），所有资金/打印/冲正入口强制走它；
  后台重放单独定策略（锁屏/登出即暂停并记审计）；补"锁屏后 submit 必须 Err"的负向测试。

### P0-4 业务包只受方法名白名单约束，能力声明没被执行

- 证据：`sandbox.ts:26` 白名单只有 `printVoucher/submitTransaction/log`，`dispatchCall`（:65）只看方法名；
  `PackageHost.vue` 把 `api.submitTransaction` 直接绑给包；Rust 侧不知道调用方是哪个包；
  `pkg.rs` 的 `TxMeta{code, idempotency, offline_capable, devices}` 没有任何地方读取。
- 影响：任何已安装包（今天 = 任何人用 demo 公钥签的包）都能静默提交任意 `tx_code`、任意金额、
  任意 `elements`，无确认、无限额，审计无法区分柜员操作与包操作。沙箱只防 DOM，不防业务。
- 建议：IPC 由主壳注入 `package_id/route`（不让前端传），core 校验 `tx_code ∈ 包声明`、
  按包声明降权 `offline_capable`、按权限码/限额放行；资金类动作要求柜员确认回执；
  审计记录 `package_id`。

## 5. P1：授权、账本、审计

### P1-5 冲正授权的决策信息是空的，未登录也能"自批"

`app.rs:325` 构造 `ReversalRequest{ tx_code: "", amount_cents: 0 }`；`DemoSupervisor` 无条件
`granted: true`；`reversal.rs` 的自批检查只比较字符串，未登录时 `actor()` 返回 `"unknown"`（`app.rs:635`），
与 `"T0101"` 不同 → 未登录也能完成"申请 + 授权 + 冲正"。

建议：要素从账本按幂等键取（交易码/金额/账号掩码/时间/结果）；授权端展示 + 二次确认；
initiator 必须来自有效会话；approver 必须是另一有效会话/凭证；"无会话"单独报错。

### P1-6 审计是"尽力而为"，不能作为证据

- 写失败只 `eprintln`（`app.rs:613`），交易照常成功。
- `audit_log` 只是类型上不提供 update/delete；DB 层无 hash 链、无 `BEFORE UPDATE/DELETE` 触发器。
- `at` 由实现用本机 `SystemTime` 生成，改钟即可伪造/重排。
- 登录失败不审计（`app.rs:205` 直接返回 Err）。
- 重放/冲正的审计 actor 是"当前登录人"，不是原交易柜员。

建议：`seq + prev_hash`（或 HMAC）；写失败按策略阻断资金类操作；可信时间；
登录失败/配置变更/包加载/锁屏解锁/快照变更入审计；审计与交易同事务或 outbox。

### P1-7 待冲正清单不落盘，对账入口是死代码

`ReversalBook` 纯内存（`packages/core/src/reversal.rs`）。重启后 `tx_exceptions()` 为空，
而 `tx_reconciliation` 只给聚合 + 12 字符截断的 key（`ReconcilePanel.vue`），凑不出完整 key 就无法冲正。
`App.vue:32` 的 `showReconcile` 从未被置 true——对账面板没有入口。

### P1-8 幂等与重放的语义缺口

幂等索引、重放 attempts/backoff 都在内存（`tx.rs`/`replay.rs`）→ 重启清零，等于可无限重试；
`MarkedForReversal` 刻意不写幂等索引，同一 key 可再次提交；达上限的交易只在重放结果里提一句。

### P1-9 传输与终端身份

- 无 mTLS/终端证书/请求签名：能连到主机的机器都能冒充柜面终端（叠加 P0-2 即完整冒用）。
- `TcpTransport` 完全绕过 TLS 门（`endpoint_config.rs:156` 只拦 `http://`）。
- `HttpTransport::read_to_end` 无响应体大小上限（TCP 有 16MB）；不按 `Content-Length` 截断。
- `build_request_head` 把 head 值原样拼进报文，未拒 CR/LF；`timeout_ms`/`width` 无上下界；IPv6 字面量解析错误。

## 6. P2：敏感数据与工程面

| 项 | 证据 | 说明 |
|---|---|---|
| 完整卡号进报文与凭证 | `TxPanel.vue:65` | `elements.account` 是原始输入，与 `journal-sqlite/AGENTS.md` "只放掩码"矛盾 |
| 密文可被降级为明文 | `cipher.rs:116` | 无 `v1:` 前缀一律当明文返回；AAD 为空 |
| 密钥创建有窗口期 | `cipher.rs:54` | 先 write 再 chmod 0600；Windows 无 ACL（B7 只提"同目录"） |
| 剪贴板/水印只是体验层 | `clipboard.ts`、`Watermark.vue` | 挡不住截图/拖拽；水印时间只算一次 |
| 供应链 | `gate.yml` | action 用 tag 引用、toolchain `stable` 不可复现；无产物签名/SBOM |
| 安全姿态靠环境变量 | `TK_*` | 本机改环境即可改安全姿态，且不受配置签名保护 |
| 包/模块面 | `pkg.rs`、`package_module` | `entry` 不校验存在；route 不校验归属；Blob URL 不 revoke；iframe 接受重复 init |
| 登录面 | `app.rs::login` | 无失败计数/锁定、无并发会话检测（lifecycle 列为 P3） |

## 7. 与 B1–B8 的关系

建议把上面五条 P0/P1 补进阻断清单（编号待负责人分配）：

| 建议编号 | 阻断项 | 为什么必须阻断 |
|---|---|---|
| B9 | 生产公钥注入，release 禁 demo 公钥 | 否则签名机制在生产为零 |
| B10 | 端点配置 fail-closed + TLS 强制 + 降级上 UI | 否则柜员会在"假系统"上办真业务 |
| B11 | 命令层统一会话/锁屏校验，重放受锁约束 | 锁屏目前是装饰 |
| B12 | 业务包能力执行（包身份 + tx_code 白名单 + 限额 + 审计归属） | 沙箱只防 DOM，不防越权交易 |
| B13 | 冲正授权携带原交易要素 + 有效会话双人 | 现在的授权是"空白支票" |
| B14 | 审计防篡改 + 写失败策略 + 可信时间 | 事故调查与合规底线 |

## 8. 需要实测确认的两点

1. 父级 CSP 是 `script-src 'self' blob:`，而沙箱文档是 `srcdoc`。按 CSP 规范 `srcdoc` 继承父策略，
   内联 bootstrap 可能被父级拦掉。若线上包加载正常，说明 Tauri 的 CSP 注入方式与预期不同——
   应写一条实测用例，否则"沙箱能跑"可能依赖误配。
2. `scripts/verify-p0.sh` 是否覆盖"锁屏后调 submit / request_reversal"与"删 endpoints.json 后的降级行为"。
   建议变成门禁里的负向用例，而不是人工验收项。

## 9. 评审方法说明

- 只读；未运行 `cargo`（遵守 `AGENTS.md` 编译纪律），未改动任何只读区。
- 结论以源码行号为证据；行号对应 HEAD `155e0fa`，后续改动可能漂移。
- 未做动态测试与渗透测试；P0-2/P0-3 的攻击路径建议在目标 Windows 机器上复现一次再定级。

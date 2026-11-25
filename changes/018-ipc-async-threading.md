# 018 · IPC 命令层异步化与线程模型

- 状态：提案（实现已随 P0-10 第 1 步落地，待负责人确认是否接受为正式线程模型）
- 基线：`155e0fa`
- 相关：架构评审 A-03 · 规模评审 P0-01 · `docs/project-backlog.md` P0-10 · `docs/rust-style.md` §4.3

## 现状

`apps/shell/src-tauri/src/commands.rs` 的 23 条命令全部是同步 `pub fn`，托管态是
`Mutex<AppState>`。Tauri 2 的同步命令在**主线程**执行，于是：

- `submit_transaction` / `request_reversal` / `replay_pending` 在持锁状态下打主机，
  默认 3s 超时、按优先级切端点，最坏接近 10 秒界面冻结；
- 前端每秒一次的 `sessionTick` 与交易抢同一把锁，交易卡住时锁屏判定一起停摆；
- 命令 panic 发生在主线程路径上，没有隔离层。

`docs/rust-style.md` §4.3 现有条款是「无 `async`（当前架构全同步，引入前须立 ADR）」。
本提案是那条 ADR 的前置说明：**只允许在 IPC 边界引入 async**，其余各层保持全同步。

## 建议

| 项 | 现在 | 改为 |
|---|---|---|
| 命令签名 | `pub fn` | `pub async fn` |
| 托管态 | `Mutex<AppState>` | `Arc<Mutex<AppState>>` |
| 阻塞工作 | 主线程直接跑 | `tauri::async_runtime::spawn_blocking` |
| 加锁位置 | 命令体第一行 | `with_state()` 闭包内部 |

统一交接点是 `with_state()`：克隆 `Arc` → 进阻塞池 → 加锁 → 调 `ShellApp` → 出锁 → 结果搬回。
`MutexGuard` 不允许跨 `await`（跨了就是编译错误），不靠人记。

## 为什么

- Tauri 同步命令跑主线程是框架事实，不是风格选择：界面线程被主机调用占住，
  柜员看到的是"卡死"，最容易被误操作成重复提交。
- 锁和阻塞池放在 `with_state()` 里，是为了让 23 条命令没有"忘记搬走"的余地。
  每条命令自己写 `spawn_blocking` 也能编译，但以后新增命令一定会有人抄错。
- panic 被阻塞池兜住，变成可读的「后台任务异常退出」而不是穿透 IPC。
- 契约不变：命令名、参数、DTO 全部不动；前端本来就 `await` 每次调用。

## 影响面

- `apps/shell/src-tauri/src/commands.rs`：23 条命令改为 `async` + `with_state()`；
  顺带删掉 `dev_set_offline` / `dev_set_tampered` / `dev_bump_server_version` 里
  重复了 5 次的 `dev_only()?`（历史遗留的重复校验，非功能改动）。
- `apps/shell/src-tauri/src/lib.rs`：`app.manage(Arc::new(Mutex::new(state)))`；
  后台重放线程按新托管类型取状态；合并重复的 `Arc` 导入。
- `tools/check-contracts.mjs`：命令扫描正则从 `pub fn` 放宽到 `pub (?:async )?fn`。
  **只放宽实现形态，不放松校验内容**：仍是"契约列的命令必须有 `#[tauri::command]` 实现，
  实现必须在 `generate_handler!` 注册"。放宽前后都用负向用例确认过会红。
- `docs/rust-style.md` §4.3：条款改为 IPC 边界可用 `async`，其余代码保持全同步。

## 第 2a 步（本分支追加）：会话拆独立锁

第 1 步只把阻塞工作搬走了，`session_tick` 仍要和交易抢同一把 `AppState` 锁——
一笔 3s 的主机调用照样能让锁屏判定停摆 3 秒。所以追加 2a：

| 项 | 现在 | 改为 |
|---|---|---|
| 会话状态 | `AppState.session: Option<SessionGuard>` | 独立托管的 `Arc<Mutex<SessionSlot>>` |
| `session_touch/tick/blur` | 锁 `AppState` | **只**锁 `SessionSlot` |
| 资金命令的会话判定 | 读 `AppState.session` 字段 | 短暂锁 `SessionSlot` 读判定 |
| 锁序 | 只有一把锁 | `AppState` → `SessionSlot`，禁止反向（反向必死锁） |

- `apps/shell/src-tauri/src/app.rs`：新增 `SessionSlot`（guard + 柜员号 + clock + 审计句柄）。
  会话锁中毒时按**已锁定**处理（`poisoned_session_view`），不放行。
  登录 `begin()`、登出 `end()`；锁屏事件仍写 `session_lock` 审计，字段与原来一致。
- `apps/shell/src-tauri/src/lib.rs`：`AppState` 持有 `SharedSession`，并把同一个 `Arc`
  **单独托管**一份，供 `session_*` 命令按类型取；`clock`/`audit` 与会话槽共用同一批 `Arc`，
  不再有两份时间/两份账本。

## 不做什么（这是第 1 步，不是 P0-10 的全部）

P0-10 的验收是「网络调用不持全局锁 / 锁屏 tick 不被交易阻塞 / 单飞防重复提交」。

1. ✅ 第 1 步（PR #44）：命令异步化 + 阻塞池。界面不再被主机调用冻住。
2. 🟡 第 2 步按风险拆成两半：
   - ✅ **2a**：会话拆独立锁，tick/touch/blur 完全不碰 `AppState`。
   - ✅ **2b（资金路径）**：`submit_transaction`（#47）、`request_reversal`（#49）、
     `replay_pending` + 后台 worker（#50/#51）的主机调用都已移出锁外。
     版本探测与启动兜底仍在锁内发请求，见下一节。
3. ✅ 第 3 步：`TxEngine` 拆成 `begin_submit` / `finish_submit` 两阶段，
   `begin` 里边占「在途」位、`finish` 释放，同一幂等键在结论回来前再次提交
   直接拒绝（"正在处理中"）。它必须在 2b **之前**落地：一旦把网络调用移出临界区，
   同一键的并发窗口才真正打开，没有在途去重就是拿"不持锁"换重复提交。

## 第 2b 步（本分支追加）：`submit_transaction` 的主机调用移出临界区

提交交易那条路径按「锁内校验 → **锁外打主机** → 重新加锁落结论」三段跑：

| 段 | 持锁 | 做什么 |
|---|---|---|
| 一 | ✅ | 会话闸门、写前审计、包能力/主机契约校验、写前意图落盘、占在途位 |
| 二 | ❌ | `PreparedSubmit::call_host()`——3s 超时的那个调用 |
| 三 | ✅ | `finish_submit` 落账、标待冲正、写结论审计 |

放锁期间状态可能被别人改，所以阶段一只把**值**交给阶段三（`SubmitContext`：
幂等键、交易码、金额、柜员号、包身份），绝不带 `&AppState` 引用——否则审计会记错人。
放锁期间的安全性由三件事兜住：

1. 幂等键仍占着「在途」位（第 3 步），同一笔再提交直接被拒；
2. 不同笔的并发结果由 `AppState` 锁在第 1/3 段串行化，账本写入本身是单事务；
3. 阶段三拿不到锁时返回「主机调用可能已经受理，请立即查账确认」，不假装成功也不假装失败。

已按同一三段式改造的路径：`submit_transaction`（#47）、`request_reversal`（#49）、
`replay_pending` 与后台重放 worker（#50 core 接缝 + #51 shell 接线）。

`check_update`（#53）也按三段式改造：探测 + 按需下载装包不持锁，
并用 `AppState.updating` 挡并发检查（两次并发会写同一个临时包文件）。

**`boot` 的本地无包兜底不拆**，理由是它没有可保护的并发：该路径只在首屏、
且界面此时正等 `boot` 返回，不存在"被挡住的其他命令"；拆它反而要给启动路径
（P0-5 fail-closed 语义所在）加中间态。这是有意取舍，不是遗漏。

重放的特殊之处：它是"多笔 × 3s 超时"，且网络调用夹在调度表推进中间，
所以 core 侧新增了 `plan_once`/`apply_once` 接缝——计划与回执落账分开，
取出队列即占位（两轮重放不会对同一笔打两次主机），崩了也不丢单。

因此本改动链**不能**把 P0-10 标完成。

## 风险

- 阻塞池线程数默认上限 512。前端 `setInterval` 每秒拍的 `session_tick` 在长交易期间会排队，
  这笔账由第 2 步（拆会话锁）还；当前队列深度等于交易秒数上限，不会无限增长。
- 两条并发 `submit_transaction` 现在会同时进入阻塞池，但仍串行在同一把锁上——
  并发窗口没有变大，只是"等待的位置"从主线程挪到了阻塞池。
  跨进程/跨命令的并发正确性仍由 P0-9 的落盘幂等索引与第 3 步单飞保证。
- 若负责人不接受 async 方向，本提案与对应 PR 需要整体回退；`rust-style.md` 的
  「全同步」条款恢复原样。

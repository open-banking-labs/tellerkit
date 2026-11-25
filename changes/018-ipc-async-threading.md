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

## 不做什么（这是第 1 步，不是 P0-10 的全部）

P0-10 的验收是「网络调用不持全局锁 / 锁屏 tick 不被交易阻塞 / 单飞防重复提交」。
本提案只解决第一条的前提——**把阻塞工作搬离主线程**：

1. 本轮：命令异步化 + 阻塞池。界面不再被主机调用冻住。
2. 待做：把 `session` 从 `AppState` 拆成独立锁，让 `session_tick`/`session_touch`/`session_blur`
   不再排在同一把锁后面；网络调用移出临界区。
3. 待做：`submit_transaction` 按幂等键做 in-flight 单飞去重。

因此本改动**不能**把 P0-10 标完成。

## 风险

- 阻塞池线程数默认上限 512。前端 `setInterval` 每秒拍的 `session_tick` 在长交易期间会排队，
  这笔账由第 2 步（拆会话锁）还；当前队列深度等于交易秒数上限，不会无限增长。
- 两条并发 `submit_transaction` 现在会同时进入阻塞池，但仍串行在同一把锁上——
  并发窗口没有变大，只是"等待的位置"从主线程挪到了阻塞池。
  跨进程/跨命令的并发正确性仍由 P0-9 的落盘幂等索引与第 3 步单飞保证。
- 若负责人不接受 async 方向，本提案与对应 PR 需要整体回退；`rust-style.md` 的
  「全同步」条款恢复原样。

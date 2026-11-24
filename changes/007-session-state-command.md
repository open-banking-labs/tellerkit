# 007 新增 `session_touch` / `session_tick` 命令

## 现状

离岗锁屏的判定在 `packages/core/src/session.rs`，但前端没有任何办法驱动它，
也不知道什么时候该盖锁屏。会话状态必须由 Rust 侧持有——否则业务包或
前端脚本可以直接把状态改成"已解锁"。

## 建议

新增两条命令，都返回同一个 `SessionState`：

| 命令 | 用途 | 何时调用 |
|---|---|---|
| `session_touch` | 记录一次操作 | 鼠标/键盘事件（节流到每秒最多一次） |
| `session_tick` | 周期性评估 | 每秒一次 |

`SessionState { locked, reason: string|null, idle_secs }`。

## 为什么锁屏状态不放在前端

前端持有"是否锁定"等于没有锁：任何能执行脚本的东西都能改。
锁定是授权状态的一部分，必须和权限快照一样只由 Rust 持有。

## 影响

- `contracts/ipc-commands.md`、`contracts/shell-api.md`、`contracts/fields.json`
- `apps/shell/src-tauri/src/{lib.rs,commands.rs}`
- `apps/shell/src/{api.ts,App.vue}`、`LockScreen.vue`、`Watermark.vue`

## 风险

`session_tick` 每秒一次 IPC。频率低、负载小，可接受；
若将来要 30 秒一次也可以——锁屏延迟本身由策略控制，不由 tick 频率决定。

# 安全加固 · 任务

> 机器可读格式。来源：[docs/reviews/2026-11-25-security-review.md](../../docs/reviews/2026-11-25-security-review.md)。
> `tools/check-tasks.mjs` 校验结构，`tools/run-task.mjs` 执行验收命令。
> 单元格内不得出现 `|`；多路径用英文逗号；验收命令只能有一条。

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | 提案：IPC 增加包身份上下文，冲正授权请求补原交易要素 | S-05, S-06 | `contract` | `changes/*.md` | `node tools/check-contracts.mjs` |
| T-02 | 在核心增加公钥十六进制解析与失败路径测试 | S-01 | `core` | `packages/core/src/pack.rs` | `cargo test -p tellerkit-core pack::` |
| T-03 | 菜单包与业务包公钥改为构建期注入，缺失即拒绝启动 | S-01 | `shell` | `apps/shell/src-tauri/src/lib.rs,apps/shell/src-tauri/src/app.rs` | `cargo test -p tellerkit` |
| T-04 | 端点配置失效时拒绝启动，演示模式需显式 TK_DEMO=1 | S-02 | `shell` | `apps/shell/src-tauri/src/lib.rs,scripts/verify-p0.sh` | `cargo test -p tellerkit` |
| T-05 | 为资金、打印、冲正入口加统一活动会话校验 | S-03 | `shell` | `apps/shell/src-tauri/src/app.rs` | `cargo test -p tellerkit` |
| T-06 | 后台重放在锁屏或登出时跳过并记审计 | S-04 | `shell` | `apps/shell/src-tauri/src/lib.rs` | `cargo test -p tellerkit` |
| T-07 | 业务包暴露交易能力查询并拒绝未声明交易 | S-05 | `core` | `packages/core/src/pkg.rs` | `cargo test -p tellerkit-core pkg::` |
| T-08 | 提交交易时按包身份校验交易码归属并写审计 | S-05 | `shell` | `apps/shell/src-tauri/src/app.rs,apps/shell/src-tauri/src/commands.rs` | `cargo test -p tellerkit` |
| T-09 | 冲正请求必须带原交易要素，无会话直接拒绝 | S-06, S-07 | `core` | `packages/core/src/reversal.rs` | `cargo test -p tellerkit-core reversal::` |
| T-10 | 按幂等键从账本补齐冲正要素，拒绝无会话发起 | S-06, S-07 | `shell` | `apps/shell/src-tauri/src/app.rs` | `cargo test -p tellerkit` |
| T-11 | 审计加序号与链式摘要，提供篡改检测 | S-08 | `core` | `packages/journal-sqlite/src/lib.rs` | `cargo test -p tellerkit-journal-sqlite` |
| T-12 | 审计写入失败时资金类操作返回错误而不是放行 | S-09 | `shell` | `apps/shell/src-tauri/src/app.rs` | `cargo test -p tellerkit` |
| T-13 | 登录失败与安全事件写入审计 | S-15 | `shell` | `apps/shell/src-tauri/src/app.rs` | `cargo test -p tellerkit` |
| T-14 | 异常标记与冲正墓碑落盘，重启后清单不丢 | S-10 | `core` | `packages/core/src/reversal.rs,packages/journal-sqlite/src/lib.rs` | `cargo test -p tellerkit-journal-sqlite` |
| T-15 | 对账与待冲正清单接到可点击入口 | S-11 | `ui` | `apps/shell/src/App.vue,apps/shell/src/components/DevPanel.vue` | `npm run build` |
| T-16 | 幂等索引落盘，重启后拒绝同键异内容 | S-12 | `core` | `packages/core/src/tx.rs,packages/journal-sqlite/src/lib.rs` | `cargo test -p tellerkit-core tx::` |
| T-17 | 重放次数与放弃状态落盘，重启后不再无限重试 | S-12 | `core` | `packages/core/src/replay.rs,packages/journal-sqlite/src/lib.rs` | `cargo test -p tellerkit-core replay::` |
| T-18 | 传输层补响应体大小上限并按长度截断 | S-13 | `core` | `packages/transport-net/src/lib.rs` | `cargo test -p tellerkit-transport-net` |
| T-19 | tcp 端点纳入 TLS 门，未强制时拒绝连接 | S-13 | `shell` | `apps/shell/src-tauri/src/endpoint_config.rs` | `cargo test -p tellerkit` |
| T-20 | 交易要素与凭证只发送掩码 | S-14 | `ui` | `apps/shell/src/components/TxPanel.vue` | `npm run build` |

## 执行顺序

1. **T-01 先行**：它是 `contract` 层，只产出 `changes/` 提案，由人评审后合并；T-08 依赖它的包身份字段。
2. **启动链**：T-02 → T-03 → T-04。T-04 会改变演示路径，必须同时更新 `scripts/verify-p0.sh`，否则 P0 验收脚本会失败。
3. **命令链**：T-05 → T-06 → T-12 → T-13（都在 `app.rs`/`lib.rs`，串行做，避免同文件冲突）。
4. **业务包链**：T-07 → T-08。
5. **冲正链**：T-09 → T-10 → T-14。
6. **账本链**：T-11 → T-16 → T-17（T-16/T-17 都要改 `journal-sqlite`，串行）。
7. **传输链**：T-18、T-19 可并行。
8. **界面**：T-15、T-20 可并行，各自只跑 `npm run build`。

## 完成定义

- 验收命令 PASS，且改动全部落在「允许改动」内。
- 每个负向场景至少一条断言"主机调用次数为 0"，不能只断言返回 Err。
- 涉及落盘的任务（T-14、T-16、T-17）必须带"重启后"用例：同进程内通过不算。
- 契约层任务（T-01）的产出是 `changes/` 提案，AI 不得直接改 `contracts/**`。
- 完成后更新 `docs/production-readiness.md` 的阻断清单（建议编号 B9–B14），并把本文件对应任务标记为已完成。

# 020 · 诊断包导出与更新策略下发

- 状态：提案（**未动 contracts/**，等负责人批准后实现）
- 基线：`155e0fa`
- 相关：架构评审 B-08 · 规模评审 P1-03 / P1-09 · `docs/project-backlog.md` P1-3

## 现状（本提案之前已落地什么）

- 结构化日志 + 大小轮转 + 脱敏 + panic 落盘：`apps/shell/src-tauri/src/logging.rs`（PR #56）。
- 命令层 `request_id` 贯穿，失败/慢调用留痕（PR #57）。
- 灰度判定已有稳定分桶与比例，并新增急停开关（`packages/core/src/update.rs`，PR #58）。

## 缺口一：没有"一键带走现场"的通道

现场排障现在要么让用户拷整个数据目录（**里面有 `tx.db` 与 `ledger.key`，绝对不能外发**），
要么继续截图。缺一个只有白名单内容、默认脱敏、可安全外发的诊断包。

### 建议：新增 IPC 命令 `export_diagnostics`

```
export_diagnostics(scope: "startup" | "full") -> DiagnosticsReport
DiagnosticsReport { path: string, bytes: number, files: string[] }
```

落盘到 `<appdata>/diagnostics/diag-<yyyyMMdd-HHmmss>/`，**目录而不是 zip**：
本项目没有压缩依赖，引 `zip` 要走供应链评审；需要单文件时由运维用系统工具压缩。

**白名单内容（少一个不如多一个都不行）**：

| 文件 | 内容 | 脱敏 |
|---|---|---|
| `meta.json` | 版本、构建标识、公钥指纹、OS/Arch、实例锁/账本状态、启动致命原因 | 无敏感字段 |
| `logs/` | `client.log*` 最近 ≤2MiB | 已在写入期脱敏（≥12 位数字 → `***`） |
| `audit_tail.json` | 最近 200 条审计（actor/action/target/result/at） | `detail` 里的 `amount_cents` 一律掩码；长数字 → `***` |
| `counts.json` | 账本各表行数、待重放/待冲正笔数、菜单版本 | 只有计数 |
| `README.txt` | 包内文件说明与"上交流程" | — |

**严禁进入诊断包**：`tx.db`、`ledger.key`、任何私钥材料、菜单包原文、客户要素、
卡号、金额明文。用"文件白名单 + 写前扫描"双保险；测试用假卡号/假金额断言零命中。

### 为什么 scope 要分两档

- `startup`：启动失败时用（此时账本可能打不开），只含 meta + 日志尾部 + 启动原因。
- `full`：可正常启动时用，多含审计尾部与计数。

### 实现影响

`contracts/ipc-commands.md` +1 命令、`contracts/shell-api.md` +1 DTO、
`contracts/fields.json` +1 生成类型、前端 `generated/` 同步；
`commands.rs` +1、`app.rs` 增收集逻辑（纯函数可测）、`logging.rs` 暴露日志目录路径。

## 缺口二：更新策略没落到传输层

`UpdateCandidate`（版本/通道/灰度/急停）目前**只由测试构造**，服务端下发链路未接（B5）。
所以 #58 的急停只是"判定层就绪"，还不能真的停住任何终端。

### 建议：更新策略进契约并由更新器执行

服务端在更新策略里下发：

| 字段 | 类型 | 含义 |
|---|---|---|
| `latest` | 版本 | 已有 |
| `channel` | `stable` / `canary` | 终端所属通道 |
| `rollout_percent` | 0–100 | 灰度比例（网点稳定分桶） |
| `kill_switch` | bool | **急停**：为真时一律 `Skip` |
| `min_shell` / `max_shell` | 版本 | 已有兼容区间 |

客户端保证：

1. `decision = update::decide(current, candidate, channel, outlet, local_hour)` 是**纯函数**，
   同样的输入永远同样的结论（桶算法已钉黄金值）。
2. `UpdateAction::Skip` 不下载；`DownloadOnly` 后台下载不提示；`DownloadAndNotify` 提示下次启动生效。
3. 急停优先于一切（已实现并测试）。
4. `outlet` 取自当前权限快照的网点号；未登录时**不评估更新**（没有身份就不该被灰度）。

### 不做什么

- 差分升级（bsdiff 等）：需要构建管线产出差分包 + 客户端安装器，属交付链 P1-1，单列。
- 崩溃上报 / 心跳 / 错误码远端上报：需要服务端接口与出网策略，属 P1-16 与 P1-10。

## 验收对应（批准后实现时照此验收）

- 现场模拟故障 → 导出目录能在离线机器上定位原因（含版本、公钥指纹、错误码、
  最近失败命令与耗时、审计链尾部是否连续）。
- 导出内容用测试卡号/测试金额扫描，零命中。
- 服务端置 `kill_switch=true` → 终端 `decide=Skip` 且原因写明"急停"；
  同网点同版本连续多次检查，桶不横跳。

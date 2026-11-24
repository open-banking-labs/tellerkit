# src-tauri/AGENTS.md

## 职责

应用装配与 IPC 边界：`AppState` 持有唯一真相源，命令层只做「加锁 → 调核心 → 计时 → 翻译错误」。

## 允许依赖

`tauri` · `serde` · `serde_json` · `tellerkit-core`

## 禁止

- `unwrap()` / `expect()`（panic 会穿透 IPC，客户端直接白屏）
- 在命令里写业务逻辑（决策属于 `tellerkit-core::sync` / `filter`）
- 前端直连文件系统或数据库（必须经命令）
- 把权限快照写入磁盘（ADR-0005）
- 记录客户敏感明文到日志

## 对外契约

命令清单见 `contracts/ipc-commands.md`，DTO 见 `contracts/shell-api.md`。
新增命令必须同时改契约、前端 `src/contracts.ts`、`check-contracts` 门禁。

## 测试

```bash
cargo build -p tellerkit            # 日常
cargo test --workspace              # 逻辑回归
```

`commands.rs` 里不写单测：决策已下沉到 core，这一层只验证能编译、能跑通启动。

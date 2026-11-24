# <能力名> · 任务

> 每项必须引用 spec 场景 ID 与层次。AI 只在 `layer` 指定的目录内改。

| ID | 任务 | 场景 | layer | 允许改动 | 验收命令 |
|---|---|---|---|---|---|
| T-01 | | S-01 | `core` | `crates/tellerkit-core/src/*` | `cargo test -p tellerkit-core` |
| T-02 | | S-02 | `ui` | `src/**` | `npm run build` |

## 层次含义

- `contract`：只改 `contracts/`，必须走 `changes/` 提案，**AI 不得直接做**
- `core`：纯逻辑，无 GUI 依赖
- `shell`：`src-tauri/`，只装配与转发
- `ui`：`src/`，只渲染

## 完成定义

- 对应场景有测试或用例可复现
- `npm run gate` 绿
- 没有超出 `允许改动` 的文件

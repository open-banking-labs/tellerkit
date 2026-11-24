# AGENTS.md

人 + AI 共同维护的柜面终端仓库。**进场先读本文件，再读离目标目录最近的 AGENTS.md。**
完整规则见 [docs/constitution.md](docs/constitution.md)——那是正文，这里只放禁令和导航。

## 只读区（AI 不得修改）

`contracts/**`（唯一事实源）· `docs/adr/**` · `docs/constitution.md` · `changes/**`（只能新增提案）

## 全局禁令

1. 不得为实现改契约。契约缺失 → 写 `changes/` 提案，等人批准。
2. 不得跨层 import；前端不得判断权限（见 `tools/check-boundaries.mjs`）。
3. 不得声称"测试已通过"——以 `npm run gate` 的 exit code 为准。
4. 一次任务一个文件/一个 trait/一个页面；单次新增 >300 行退回重拆。
5. 不得自主扩范围；新需求写 TODO。
6. 命令层不得 `unwrap()`；错误必须翻译成人能看懂的文案。
7. 权限快照不落盘；菜单隐藏 ≠ 安全。
8. 端侧 AI 只预留契约位置，不写实现。

## 入口导航

| 我要改 | 去哪 |
|---|---|
| 接口 / 字段 / 错误码 | `contracts/`（只读） |
| 行为规格 | `.specify/specs/<capability>/spec.md` |
| 核心逻辑（菜单/验签/存储/同步） | `crates/tellerkit-core/` |
| 应用装配与命令 | `src-tauri/` |
| 界面（只渲染） | `src/` |
| 门禁 | `tools/` |

## 命令

```bash
npm run gate                  # 三道门禁，唯一裁决入口
cargo test --workspace        # 核心逻辑
npm run build                 # 前端类型检查 + 打包
bash scripts/verify-p0.sh     # 菜单包端到端验收
```

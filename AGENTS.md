# AGENTS.md

人 + AI 共同维护的柜面终端平台。**进场先读本文件**（README 是给人看的，别当规则）。
阅读顺序逐级下发：[docs/constitution.md](docs/constitution.md)（完整规则）→ [docs/team.md](docs/team.md)（角色分工）→ 目标目录最近的 `AGENTS.md` → 该目录 `README.md` → 任务所在 `.specify/specs/<id>/`。

## 只读区（AI 不得修改）

`contracts/**` · `docs/adr/**` · `docs/constitution.md` · `.agents/presets/**` · `changes/**`（只能新增提案）

## 全局禁令

1. 不得为实现改契约。契约缺失 → 写 `changes/` 提案，等人批准。
2. 不得跨层 import；前端不得判断权限（`tools/check-boundaries.mjs`）。
3. 不得声称"测试已通过"——以 `npm run gate` 的 exit code 为准。
4. 一次任务一个文件/一个 trait/一个页面；单次新增 >300 行退回重拆。
5. 不得自主扩范围；新需求写 TODO。
6. 命令层不得 `unwrap()`；错误必须翻译成人能看懂的文案。
7. 权限快照不落盘；菜单隐藏 ≠ 安全。
8. 端侧 AI 只预留契约位置，不写实现。
9. 提交前跑 `node tools/check-bank-names.mjs`；不得出现具体银行名称/缩写。
10. 不得跨模块直接读写对方结构体字段；不得把内部模块全部 `pub`（宪法第 11 条、`tools/check-module-depth.mjs`）。

## 编译纪律

**不在本机跑 cargo。** 编译一律走 `scripts/remote-build.sh`（经 Tailscale 到首尔构建机），Windows / Ubuntu / macOS 产物都在那台机器上验证；本地只跑 `npm run gate` 的 node 部分。

## 分支纪律

`main` 受保护：**改动必须走 PR，且 `gate` 检查通过才能合并**；禁止直推/force push。分支：`feat/` `fix/` `chore/` `docs/`。

## 入口导航

| 我要改 | 去哪 |
|---|---|
| 接口 / 字段 / 错误码 | `contracts/`（只读） |
| 行为规格 | `.specify/specs/<capability>/` |
| 核心引擎（菜单/验签/存储/同步） | `packages/core/` |
| 主壳装配与命令 | `apps/shell/src-tauri/` |
| 界面（只渲染） | `apps/shell/src/` |
| 业务包 | `packages/biz-*/` |
| Agent 预设 | `.agents/presets/` |
| 门禁 / 模块化 / 路径发现 | `tools/` · `docs/architecture.md` §2.1 |
| Rust 写法 | `docs/rust-style.md`（Google Rust Style Guide 落地版） |
| 边界 / 上生产 | `docs/desktop-scope.md` · `docs/production-readiness.md` |

## 命令

```bash
npm run gate                  # 全部门禁，唯一裁决入口
npm run bindings              # 由 Rust 重新生成前端类型（改了 Rust 必须跑）
npm run test                  # cargo test --workspace
npm run build                 # 前端类型检查 + 打包（apps/shell）
bash scripts/verify-p0.sh     # 菜单包端到端验收
npm run task -- --tasks .specify/specs/<id>/tasks.md --id T-01   # 跑一个任务并裁决
npm run dispatch -- --tasks .specify/specs/<id>/tasks.md --id T-01 --record   # 编译任务单并留痕
npm run metrics               # 派发指标与判断（含覆盖率，别把有记录当全覆盖）
npm run stats                 # 规模数字（能力/任务/场景/契约），不要手抄
npm run dev                   # 浏览器预览（无需 Tauri）
```

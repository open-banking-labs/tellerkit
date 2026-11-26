# 018 合并裁决口径：gate 是静态预检，CI required checks 才是裁决

## 现状

宪法第 5 条写「`npm run gate` 是唯一入口，CI 与本地同一条命令」。实际：

- `npm run gate` 只跑八道 node 静态检查 + `cargo fmt --check`，不跑 `cargo test`、
  `clippy`、`npm run build`；exit 0 不代表编译或测试通过。
- CI（`.github/workflows/gate.yml`）另外跑 test / clippy / bindings diff / build，
  还新增 `contracts/**` 变更必须带 `changes/*.md` 提案的检查。
- 本机编译纪律禁止跑 cargo，本地无法给出完整裁决。

`changes/015-gate-adjudication.md` 已提出过同一问题，但宪法第 5 条仍未修订；本提案按当前
门禁形态（含 strict 模块深度与契约保护）重新给出可直接替换的条文。

## 建议

将宪法第 5 条的检查条款改为：

> 检查：`npm run gate` 是本地静态预检，exit 0 只表示静态检查通过，不代表编译或测试通过。
> 合并裁决以分支保护要求的 CI checks 为准（test / clippy / bindings / build / gate 全绿）。
> CI 另检查 `contracts/**` 的变更是否附带新增 `changes/*.md` 提案。
> 禁止用本地 gate 的 exit 0 代替测试结论。

同步把附则中「以 `npm run gate` 的 exit code 为准」改为「以 CI required checks 为准」。

## 影响

- `docs/constitution.md`：第 5 条与附则措辞（只读文件，需负责人修改；本提案不直接改）。
- `AGENTS.md`：全局禁令与命令区措辞（需负责人修改）。
- 已同步：`README.md`、`docs/production-readiness.md`、`docs/architecture.md`、
  `tools/stats.mjs`、`.github/workflows/gate.yml` 注释。
- 不改运行时代码与契约内容。

## 风险

若仓库管理员没有把 CI job 设为分支保护 required check，裁决仍会退化成「谁本地绿谁合并」；
需要管理员在 PR 合入前确认 required checks 清单。

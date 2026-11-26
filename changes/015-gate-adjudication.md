# 015 合并裁决以 CI 为准，`npm run gate` 定位为静态预检

## 现状

宪法第 5 条写的是：

> 检查：`npm run gate` 是唯一入口，CI 与本地同一条命令。

实际不是：

- `npm run gate` 只跑 6 个 node 检查和 `cargo fmt --check`，不跑 `cargo test`、`clippy`、`npm run build`；
- CI（`.github/workflows/gate.yml`）在同一 job 里另外跑 test / clippy / bindings diff / build / metrics；
- 编译纪律又规定本机不跑 cargo，本地根本不可能执行完整裁决。

结果是"gate 全绿"可以被当成"测试已通过"，而这两件事不是一回事。架构评审 C-01 指的就是这个。

## 建议

1. 宪法第 5 条改为：**合并裁决以分支保护要求的 CI 检查为准**（test / clippy / bindings / build / gate 全绿）；`npm run gate` 是本地静态预检，exit 0 只代表静态检查通过，不代表编译与测试通过。
2. `npm run verify` 补上 `cargo clippy --workspace --all-targets --locked -- -D warnings`，作为"有编译环境时的本地全量"；本机仍按编译纪律走 `scripts/remote-build.sh`。
3. README、AGENTS.md、docs/team.md 以及 `gate.yml` 顶部注释同步措辞；删除"CI 与本地同一条命令"的表述。
4. 禁止在任何汇报里用 `npm run gate` 的 exit 0 代替测试结论；测试结论只认 CI 的 required check。

## 影响

- `docs/constitution.md`：第 5 条措辞（只读区，需负责人修改）
- `README.md`、`AGENTS.md`、`docs/team.md`：命令说明
- `package.json`：`verify` 增加 clippy
- `.github/workflows/gate.yml`：注释；步骤不变
- 不改任何运行时代码

## 风险

- 本地没有编译环境时，只能等 CI 才能得到完整结论；这比"本地 gate 绿了就宣称通过"慢，但结论可信。
- 若分支保护没有把 CI job 设为 required，本提案等于没有落地；需要仓库管理员确认 required checks。

# 变更记录

版本唯一来源：`Cargo.toml` 的 `[workspace.package].version`。
`package.json`、`apps/shell/package.json`、`apps/shell/src-tauri/tauri.conf.json`
三处由 `npm run check:version`（已进 `npm run gate`）校验一致，漂移即失败。

## 0.1.0（未发布 · 试点准备）

项目级整改按 `docs/project-backlog.md` 的优先级推进，本版本记录控制点变更，
不列日常文档微调。完整细项见 `docs/reviews/` 与三份 backlog。

### 信任与裁决

- 全仓禁止具体银行名称/缩写；`npm run gate` 与 pre-commit 均含银行名检查。
- 构建可复现：补齐 `Cargo.lock` 并对 test/clippy/remote-build 全部加 `--locked`。
- 轨迹门禁改为真裁决：提示词/输入指纹不符、记录无法解析一律失败；
  作废记录须在 `.agents/dispatch-superseded.json` 登记原因。
- 提示词指纹跨平台归一化为 LF，新增 `.gitattributes`。
- 合并裁决以 CI required checks 为准；`npm run gate` 定位为本地静态预检
  （见 `changes/015-gate-adjudication.md`）。
- 文档与事实对齐：README/PLAN/team/security-boundary 不再声称未实现的能力；
  删除标题/短语存在性这类伪门禁；新增版本一致性门禁。

### 启动与会话

- 菜单验签公钥由 `TK_MENU_PUBKEY_HEX` 注入；release 缺公钥或非法值构建/启动均失败。
- 启动配置 fail-closed：release 缺端点、配置非法、未签名或明文端点一律拒绝启动；
  非法开关值不静默当作关闭；debug 才回退演示远端且必须带提示。
- 资金、打印、冲正、重放、业务包入口统一经 Rust 侧 `require_unlocked` 闸门；
  锁定/未登录返回可读错误、不触达主机、拒绝写审计；后台重放同门。

### 仍在进行

- 批次 2 的资金状态可恢复项（业务包能力、写前意图、幂等落盘、审计链）。
- 批次 3 起的交付链、单实例、可观测性与主壳产品化，按台账顺序推进。

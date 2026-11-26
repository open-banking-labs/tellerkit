# 016 · 启动配置 fail-closed

- 状态：提案（实现已随 P0-5 落地，待负责人确认是否接受为正式行为）
- 基线：`155e0fa`（`changes/008-endpoint-config.md` 第 34 行定义了旧行为）
- 相关：A-06 · 安全评审 P0-2 · 规模评审 P0-09 · `docs/project-backlog.md` P0-5

## 要改什么

`changes/008` 规定：端点配置缺失或解析失败 → 回退内置 mock，并在 notes 里写原因。
这条在演示环境成立，在柜面不成立——**它把"配置错了"变成"看起来在办业务"**。

本次改动把启动行为按构建类型分开：

| 情形 | debug 构建 | release 构建 |
|---|---|---|
| 没有 `endpoints.json` | 回退演示远端 + note | **拒绝启动** |
| 配置非法（JSON/字段/端点） | 回退演示远端 + note | **拒绝启动** |
| 配置未签名或缺签名公钥 | 按策略 | **拒绝启动**（强制签名） |
| 明文 `http://` 端点 | 按策略 | **拒绝启动**（强制 TLS） |
| 开关值写错（如 `TK_REQUIRE_TLS=maybe`） | 报错，不当作关 | 报错，不当作关 |

「拒绝启动」不是进程直接消失，而是：进程起来 → `boot` 返回
`启动被拒绝：<原因>` → 前端停在错误页。这样运维能看到原因，而不是黑屏。

## 为什么

- 柜面的第一原则是"看到的和发生的是同一件事"。演示远端静默接管，
  等于让柜员在假界面上办真业务。
- release 里"能不能关掉验签/TLS"本身是漏洞：出事时第一个被改的就是这两个开关。
  所以 release 不提供关闭入口，debug 才能显式打开。
- 启动失败必须可见。Tauri 的 setup 失败在 Windows 上没有控制台，
  等于静默退出；因此把失败承载到 `AppState.startup_fatal`，
  由唯一的加锁入口 `lock()` 拦下所有命令。

## 影响面

- `apps/shell/src-tauri/src/bootstrap.rs`（新增）：策略与判定，纯函数可测。
- `apps/shell/src-tauri/src/lib.rs`：`build_remote` 改为调用 bootstrap；
  `AppState` 增加 `startup_fatal`。
- `apps/shell/src-tauri/src/commands.rs`：`lock()` 统一拦截。
- 契约不变（命令与 DTO 未变）。
- 运维影响：release 部署必须先下发 `endpoints.json` + `.sig`，
  并注入 `TK_CONFIG_PUBKEY`；否则起不来——这是有意为之。

## 未覆盖（后续任务）

- `dev_*` 命令仍在 release 产物中注册，应按构建裁剪。
- 前端只在登录页显示错误；独立「启动失败」页面随界面批次做。
- TLS 目前只校验 scheme，不做证书固定 / mTLS（属 P1-4）。

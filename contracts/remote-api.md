# 远端 API 契约（边界 ③）

> 桌面应用 ↔ 银行后台。与本地 IPC 契约（`ipc-commands.md`）**严格分开**：
> 前者是 HTTP/REST，跨网络、有超时与重试；后者是本机进程间调用。
>
> 状态：骨架。当前由 `apps/shell/src-tauri/src/ports.rs::MockRemote` 实现；
> `HttpRemote` 按本文实现即可替换，应用层不改。

## 0. 客户端对服务端的三个硬要求

这三条不是"建议"，客户端的所有安全假设都建立在其上：

1. **交易接口按 `idempotency_key` 幂等**。同键重复提交只入账一次，且返回**同一个** `reference`。
   客户端会用同一个键重试（网络恢复后）与重放（离线队列），服务端不去重就会重复扣账。
   见 `changes/001`。
2. **鉴权独立于客户端**。菜单/按钮的可见性只是体验层；每个接口都要独立校验权限。
3. **菜单包签名私钥只存在于服务端构建环节**。客户端只内置公钥。

## 1. 通用约定

| 项 | 约定 |
|---|---|
| 版本 | 路径前缀 `/v1`；不兼容变更升版本，不原地改语义 |
| 认证 | `Authorization: Bearer <token>`；token 只含 `userId` + `permVersion` |
| 内容类型 | JSON（菜单包除外，是二进制） |
| 时间 | UTC ISO-8601 字符串；客户端不依赖服务端时区 |
| 重试 | 客户端只对 `5xx` 与网络错误重试；`4xx` 视为最终结论 |

### 错误模型

```jsonc
{
  "code": "TX_DUPLICATE_KEY",      // 稳定的机器可读码
  "message": "该幂等键已用于另一笔交易",  // 可直接展示给柜员（中文）
  "retryable": false
}
```

`code` 是契约的一部分，客户端按它分支；`message` 允许服务端调整文案。

## 2. 接口

### GET /v1/menu/version

启动后的第一个后台请求，必须极小（< 1KB）。

**响应**

```jsonc
{ "latest": 7, "shell_compat": { "min": 1, "max": 1 } }
```

客户端行为：`latest` 与本地版本一致 → 什么都不做；不一致 → 后台下载。

### GET /v1/menu/pack

**查询**：`?version=7`
**响应**：`application/octet-stream`，即 `.mtree` 文件字节（客户端负责验签）。
**错误**：`404` 表示该版本不存在，客户端回退到当前版本。

### POST /v1/auth/login

**请求**

```jsonc
{ "role_id": "teller", "client": { "shell_version": "0.1.0" } }
```

**响应**：`PermSnapshot`（字段见 `contracts/menu-biz-schema.md`）。
**错误**：`401` 凭据无效；`403` 该角色被禁用。

> 生产环境这里应换成行内统一认证，`role_id` 只是 P0 的 mock 形态。

### POST /v1/tx/submit

**请求**

```jsonc
{
  "idempotency_key": "b6f1…",     // 客户端生成，必填
  "tx_code": "account.deposit",
  "amount_cents": 100050,
  "elements": { "account": "6222****0123" },
  "offline_capable": false
}
```

**响应 200**

```jsonc
{ "reference": "TX00000042", "accepted_at": "2026-11-25T00:00:00Z" }
```

**错误**

| HTTP | code | 客户端处置 |
|---|---|---|
| 409 | `TX_DUPLICATE_KEY` | 同键不同载荷：拒绝，提示换键 |
| 422 | `TX_REJECTED` | 业务拒绝（余额不足等），不重试 |
| 503 | `SERVICE_UNAVAILABLE` | 可重试；`offline_capable` 则挂起 |
| 504 | `GATEWAY_TIMEOUT` | **状态不明**：标记为待冲正，不得当失败 |

`504` 这一行是整份契约里最关键的：超时不等于失败。

### GET /v1/packages

业务包清单。P2 阶段客户端只读本地目录，本接口留待网络下发。

## 3. 客户端侧的端口

```rust
// apps/shell/src-tauri/src/ports.rs
pub trait RemoteBanking: TxHost + Send + Sync {
    fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError>;
    fn download_menu(&self) -> Result<Vec<u8>, RemoteError>;
    fn login(&self, role_id: &str, perm_version: u64) -> Result<PermSnapshot, RemoteError>;
}
```

`TxHost` 由领域定义（状态机需要它），`RemoteBanking` 覆盖其余。

`RemoteError` 刻意只有三种：`Unavailable`（可重试）/ `Rejected`（不必重试）/ `Unauthorized`（重新登录）。
HTTP 状态码、header、序列化细节都在 `HttpRemote` 里消化，**不得泄漏到应用层**。

## 4. 与本地 IPC 契约的区别（别混）

| 维度 | 本地 IPC（②） | 远端 API（③） |
|---|---|---|
| 传输 | Tauri invoke | HTTP/JSON |
| 延迟 | 微秒级 | 毫秒到秒级 |
| 失败 | 基本不失败，失败即 bug | 常态，必须设计重试与幂等 |
| 版本 | 与客户端同包发布 | 独立演进，需兼容窗口 |
| 契约文件 | `contracts/ipc-commands.md` | 本文 |
| 生成物 | ts-rs 生成 TS 类型 | 手工维护（待定：OpenAPI 生成） |

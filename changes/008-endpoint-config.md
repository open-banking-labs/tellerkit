# 008 远端端点配置

## 现状

远端抽象已经有三层（Router / Codec / Transport）和 `RoutedBanking` 适配器，
但**没有配置入口**：主壳固定用 `MockRemote`，真实端点无法接入。

## 建议

新增网点级配置 `endpoints.json`，由运维下发，放在应用数据目录；
也可用环境变量 `TK_ENDPOINTS` 指向别的路径（演示与联调用）。

```jsonc
{
  "services": {
    "menu.core": [
      { "transport": "http", "codec": "json", "url": "http://menu.internal/v1",
        "priority": 0, "timeout_ms": 3000 }
    ],
    "acct.core": [
      { "transport": "http", "codec": "json", "url": "http://acct.internal/v1",
        "priority": 0, "timeout_ms": 5000 },
      { "transport": "tcp", "codec": "json", "host": "10.0.8.11", "port": 7001,
        "priority": 1, "timeout_ms": 3000 }
    ]
  }
}
```

规则：

1. 同一服务多条端点按 `priority` 升序尝试，只在**可重试**失败时切换。
2. `transport` 可选 `http` / `tcp`；`codec` 可选 `json` / `raw`。
3. 配置缺失或解析失败 → **回退到内置 mock**，并在启动 notes 里明确写出原因。
   柜面宁可"跑在演示模式"也不要"打不开"。
4. 配置里**不允许**出现凭据（token/密码）；凭据走登录流程。

## 为什么是文件而不是配置中心

P3 阶段没有配置中心。文件能让网点的运维手工改、能随安装包分发、
也能被备份系统带走。等接入配置中心时，这个文件只需变成"缓存"。

## 影响

- `apps/shell/src-tauri/src/endpoint_config.rs`（新增）
- `AppState.remote` 从 `Arc<MockRemote>` 改为 `Arc<dyn RemoteBanking>`
- `sync::{boot_load, check_and_update, reload}` 参数从 `&MockHost` 改为 `&dyn MenuSource`
- `contracts/remote-api.md` 增加配置示例

## 风险

1. **配置写错就打到错的地址**：解析失败一律回退 mock，不猜。
2. **明文配置能被改**：端点指向恶意服务器可造成数据泄露。这是运维层面的
   供应链问题，客户端只能做"配置签名校验"（P4），当前未做。
3. **不校验 TLS**：`HttpTransport` 只支持 `http://`，生产必须补。

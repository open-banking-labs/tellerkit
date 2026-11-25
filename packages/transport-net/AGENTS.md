# transport-net/AGENTS.md

## 职责

远端**传输**适配器：把 `tellerkit_core::remote::Transport` 端口落到真实 socket。
只负责"把字节送到、把字节取回"。

## 允许依赖

`tellerkit-core`（仅 `remote` 模块的类型）· 标准库 `std::net` / `std::io`

## 禁止

- 做重试、主备切换、超时策略（属于 `Router`）
- 做业务语义（报文含义由 `Codec` 决定）
- 直接读写磁盘、依赖 tauri
- 引入 HTTP 客户端库：当前实现只需要定长 POST；上 TLS 前先立 ADR

## 已知缺口（不要当成已完成）

- `HttpTransport` 不支持 TLS、重定向、分块编码、连接复用
- `TcpTransport` 不做长连接与心跳
- 没有连接池

## 测试

```bash
cargo test -p tellerkit-transport-net
```

用例会起本地 `TcpListener` 做真实收发，不依赖外网。

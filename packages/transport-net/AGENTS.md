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
- 引入 HTTP 客户端库（连接池/重定向）：当前只需要定长 POST；TLS 已按 `changes/009` 用 rustls 实现

## 已知缺口（不要当成已完成）

- `HttpTransport` 已支持 TLS（rustls + 内置根证书），但仍不支持重定向、分块编码、连接复用
- 不读系统证书库：企业自签 CA 需要显式 `with_extra_roots`（生产未启用）
- `TcpTransport` 不做长连接与心跳
- 没有连接池

## 测试

```bash
cargo test -p tellerkit-transport-net
```

用例会起本地 `TcpListener` 做真实收发，不依赖外网。

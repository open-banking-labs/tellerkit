# tellerkit-transport-net

远端传输适配器：TCP 短连接与最小 HTTP/1.1。

## 为什么单独一个 crate

传输要碰 `std::net`，而 `tellerkit-core` 必须保持纯逻辑（可脱离客户端测试）。
把它放 core 会让"核心能不能 IO"这条边界失效。

## 两个实现

| 类型 | 分帧 | 适用 |
|---|---|---|
| `TcpTransport` | `[u32 大端长度][载荷]`，短连接 | 自定义报文协议 |
| `HttpTransport` | HTTP/1.1 POST + `Content-Length`，支持 `http://` 与 `https://`（rustls） | REST 后台 |

两者都只实现 `Transport::{name, exchange}`，编排（主备、重试、编解码）在
`tellerkit-core::remote::Router`。

## 用法

```rust
use tellerkit_core::remote::{Endpoint, EndpointAddr, FixedWidthCodec, Router};
use tellerkit_transport_net::TcpTransport;

let router = Router::new(vec![Endpoint {
    service: "card.auth".into(),
    addr: EndpointAddr::Tcp { host: "10.0.9.20".into(), port: 7002 },
    transport: std::sync::Arc::new(TcpTransport::default()),
    codec: std::sync::Arc::new(FixedWidthCodec::new(layout)),
    priority: 0,
    timeout_ms: 3000,
}]);
```

## TLS

`https://` 走 `rustls`，根证书来自 `webpki-roots`（内置，不读系统证书库），
保证三平台行为一致。测试用 `rcgen` 起本地 TLS 服务器，覆盖握手与不受信证书拒绝。

`with_extra_roots()` 可以追加根证书，**只给测试与内网自签场景用**。

## 未做（生产前必须补）

连接池、重定向、代理、DNS 失败的重试策略、证书吊销（OCSP/CRL）、企业 CA 支持。

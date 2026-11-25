# tellerkit-fake-bank

本地假后台：一个**真的 TCP 服务端**，用于验证"路由 + 编解码 + 传输"整条链路。

## 为什么不叫 mock

mock 传输验证不了分帧、连接生命周期、响应解析这些**只有真 socket 才会出错**的地方。
Windows 上的 RST 就是在真 socket 上才暴露的。

## 协议

与 `TcpTransport` 对齐：`[u32 大端长度][载荷]`，请求与响应同构。

- 载荷以 `{` 开头 → 按 JSON 处理：`{"op":..., "payload":...}`，响应 `{"payload":...}`
- 否则 → 二进制下载：原样返回 `pack_bytes`

## 支持的操作

| op | 响应 |
|---|---|
| `probe_menu` | `{latest, shell_compat_min, shell_compat_max}` |
| `login` | `PermSnapshot` |
| `submit_tx` | `{reference}`，或 `{error:{code,message,retryable}}` |
| 任意二进制 | `pack_bytes`（模拟签名菜单包） |

## 只在测试里用

它不是产品代码：没有鉴权、没有持久化、不校验报文。

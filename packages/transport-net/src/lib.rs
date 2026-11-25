//! 传输适配器：把 [`tellerkit_core::remote::Transport`] 落到真实 socket 上。
//!
//! 两个实现覆盖当前两种形态：
//! - [`TcpTransport`]：**短连接** + 长度前缀分帧，适合自定义报文协议；
//! - [`HttpTransport`]：最小 HTTP/1.1（POST + Content-Length），适合 REST 后台。
//!
//! 刻意不引入 HTTP 客户端库：这里只需要"发一段字节、收一段字节"，
//! 引入 reqwest 会把 TLS、连接池、重定向等策略带进来，而那些还没定。
//! 真要上生产 HTTPS 时，替换 `HttpTransport` 的内部实现即可，端口不变。
//!
//! 两者都**不做重试与主备**——那是 `Router` 的职责。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

use tellerkit_core::remote::{EndpointAddr, Transport, TransportError, WireMessage};

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TcpStream, TransportError> {
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| TransportError::Unreachable(format!("{host}:{port} 解析失败：{e}")))?
        .collect();
    if addrs.is_empty() {
        return Err(TransportError::Unreachable(format!(
            "{host}:{port} 无可用地址"
        )));
    }

    let mut last = None;
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(stream) => return Ok(stream),
            Err(e) => last = Some(e),
        }
    }
    Err(TransportError::Unreachable(format!(
        "{host}:{port} 连接失败：{}",
        last.map(|e| e.to_string()).unwrap_or_default()
    )))
}

fn io_err(context: &str, e: std::io::Error) -> TransportError {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => TransportError::Timeout,
        _ => TransportError::Unreachable(format!("{context}：{e}")),
    }
}

/// TCP 短连接传输。
///
/// 分帧：`[u32 大端长度][载荷]`，请求与响应同构。
/// 每次调用**新建连接、写完关闭**——柜面交易是稀疏请求，
/// 长连接要处理心跳/重连/半开，收益不抵复杂度。
#[derive(Debug, Clone)]
pub struct TcpTransport {
    pub timeout: Duration,
}

impl Default for TcpTransport {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
        }
    }
}

impl Transport for TcpTransport {
    fn name(&self) -> &'static str {
        "tcp"
    }

    fn exchange(
        &self,
        addr: &EndpointAddr,
        request: &WireMessage,
        timeout: Duration,
    ) -> Result<WireMessage, TransportError> {
        let timeout = timeout.min(self.timeout);
        let EndpointAddr::Tcp { host, port } = addr else {
            return Err(TransportError::Protocol(
                "TcpTransport 只能处理 Tcp 地址".into(),
            ));
        };

        let mut stream = connect(host, *port, timeout)?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| io_err("设置读超时失败", e))?;
        stream
            .set_write_timeout(Some(timeout))
            .map_err(|e| io_err("设置写超时失败", e))?;

        let len = u32::try_from(request.body.len())
            .map_err(|_| TransportError::Protocol("请求体超过 4GB".into()))?;
        stream
            .write_all(&len.to_be_bytes())
            .and_then(|_| stream.write_all(&request.body))
            .and_then(|_| stream.flush())
            .map_err(|e| io_err("发送失败", e))?;

        let mut len_buf = [0u8; 4];
        stream
            .read_exact(&mut len_buf)
            .map_err(|e| io_err("读响应长度失败", e))?;
        let n = u32::from_be_bytes(len_buf) as usize;
        if n > 16 * 1024 * 1024 {
            return Err(TransportError::Protocol(format!("响应体过大：{n} 字节")));
        }
        let mut body = vec![0u8; n];
        stream
            .read_exact(&mut body)
            .map_err(|e| io_err("读响应体失败", e))?;

        Ok(WireMessage {
            head: Default::default(),
            body,
        })
    }
}

/// 最小 HTTP/1.1 传输，支持 `http://` 与 `https://`。
///
/// - 明文：直接 TCP + 手写报文头；
/// - TLS：`rustls` + 内置根证书（`webpki-roots`），**不读系统证书库**，
///   保证 Windows / macOS / Linux 行为一致。
///
/// 不处理：重定向、分块编码、连接复用、客户端证书。这些在接入真实后台前按需补。
#[derive(Clone)]
pub struct HttpTransport {
    pub timeout: Duration,
    client: Arc<ClientConfig>,
}

impl std::fmt::Debug for HttpTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpTransport")
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        Self {
            timeout: Duration::from_secs(5),
            client: Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ),
        }
    }
}

impl HttpTransport {
    /// 额外信任若干根证书。**只给测试与内网自签场景用**——
    /// 生产环境用 [`HttpTransport::default`]，只信公共根。
    pub fn with_extra_roots(extra: Vec<CertificateDer<'static>>) -> Self {
        let mut roots = RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for cert in extra {
            let _ = roots.add(cert);
        }
        Self {
            timeout: Duration::from_secs(5),
            client: Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(roots)
                    .with_no_client_auth(),
            ),
        }
    }
}

#[derive(Debug, Clone)]
struct Target {
    host: String,
    port: u16,
    path: String,
    tls: bool,
}

fn split_host_port(base_url: &str) -> Result<Target, TransportError> {
    let (tls, without_scheme) = if let Some(rest) = base_url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = base_url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(TransportError::Protocol(format!(
            "只支持 http:// 或 https://：{base_url}"
        )));
    };

    let (authority, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, ""),
    };
    let default_port = if tls { 443 } else { 80 };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| TransportError::Protocol(format!("端口非法：{p}")))?,
        ),
        None => (authority.to_string(), default_port),
    };
    Ok(Target {
        host,
        port,
        path: path.to_string(),
        tls,
    })
}

fn build_request_head(target: &Target, request: &WireMessage) -> String {
    let host_header = if target.port == 80 || target.port == 443 {
        target.host.clone()
    } else {
        format!("{}:{}", target.host, target.port)
    };
    let mut head = format!(
        "POST {} HTTP/1.1\r\nHost: {host_header}\r\nContent-Length: {}\r\nConnection: close\r\n",
        target.path,
        request.body.len()
    );
    for (k, v) in &request.head {
        if k == "x-op" {
            continue;
        }
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    head
}

fn write_and_read<S: Read + Write>(
    stream: &mut S,
    head: &str,
    body: &[u8],
) -> Result<Vec<u8>, TransportError> {
    stream
        .write_all(head.as_bytes())
        .and_then(|_| stream.write_all(body))
        .and_then(|_| stream.flush())
        .map_err(|e| io_err("发送失败", e))?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| io_err("读响应失败", e))?;
    Ok(raw)
}

impl Transport for HttpTransport {
    fn name(&self) -> &'static str {
        "http"
    }

    fn exchange(
        &self,
        addr: &EndpointAddr,
        request: &WireMessage,
        timeout: Duration,
    ) -> Result<WireMessage, TransportError> {
        let timeout = timeout.min(self.timeout);
        let EndpointAddr::Http { base_url } = addr else {
            return Err(TransportError::Protocol(
                "HttpTransport 只能处理 Http 地址".into(),
            ));
        };
        let op = request.head.get("x-op").cloned().unwrap_or_default();
        let mut target = split_host_port(base_url)?;
        target.path = format!("{}/{}", target.path.trim_end_matches('/'), op);

        let tcp = connect(&target.host, target.port, timeout)?;
        tcp.set_read_timeout(Some(timeout))
            .map_err(|e| io_err("设置读超时失败", e))?;

        let head = build_request_head(&target, request);

        let raw = if target.tls {
            let server_name = ServerName::try_from(target.host.clone())
                .map_err(|e| TransportError::Protocol(format!("服务器名非法：{e}")))?;
            let conn = ClientConnection::new(Arc::clone(&self.client), server_name)
                .map_err(|e| TransportError::Protocol(format!("TLS 初始化失败：{e}")))?;
            let mut stream = StreamOwned::new(conn, tcp);
            write_and_read(&mut stream, &head, &request.body)?
        } else {
            let mut stream = tcp;
            write_and_read(&mut stream, &head, &request.body)?
        };

        parse_http_response(&raw)
    }
}

/// 解析 HTTP 响应：状态行 + 头部 + 体（只支持 `Content-Length`）。
fn parse_http_response(raw: &[u8]) -> Result<WireMessage, TransportError> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or_else(|| TransportError::Protocol("响应缺少头部结束标记".into()))?;
    let head_text = String::from_utf8_lossy(&raw[..split]);
    let body = &raw[split + 4..];

    let mut lines = head_text.lines();
    let status_line = lines
        .next()
        .ok_or_else(|| TransportError::Protocol("响应为空".into()))?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| TransportError::Protocol(format!("状态行非法：{status_line}")))?;

    if !(200..300).contains(&status) {
        return Err(TransportError::Protocol(format!(
            "HTTP {status}：{}",
            String::from_utf8_lossy(body)
                .chars()
                .take(120)
                .collect::<String>()
        )));
    }

    let mut head = std::collections::BTreeMap::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            head.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }

    Ok(WireMessage {
        head,
        body: body.to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    /// 起一个只服务一次的假服务器，返回 (端口, JoinHandle)。
    fn serve_once(
        handler: impl FnOnce(TcpStream) + Send + 'static,
    ) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                handler(stream);
            }
        });
        (port, handle)
    }

    /// 完整读一个 HTTP 请求：读到头部结束，再按 Content-Length 读满 body。
    ///
    /// 只读一次就回包会让客户端在 Windows 上收到 RST（还有未读数据），
    /// 表现是 "读响应失败" 而不是预期的状态码——测试必须把请求读完。
    fn read_http_request<S: Read + Write>(stream: &mut S) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                return buf;
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                let head_end = pos + 4;
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let content_len = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Content-Length: "))
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(0);
                while buf.len() < head_end + content_len {
                    let n = stream.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                return buf;
            }
        }
    }

    fn tcp_addr(port: u16) -> EndpointAddr {
        EndpointAddr::Tcp {
            host: "127.0.0.1".into(),
            port,
        }
    }

    #[test]
    fn tcp_transport_frames_and_reads_reply() {
        let (port, handle) = serve_once(|mut stream| {
            let mut len = [0u8; 4];
            stream.read_exact(&mut len).unwrap();
            let mut body = vec![0u8; u32::from_be_bytes(len) as usize];
            stream.read_exact(&mut body).unwrap();
            // 回声：把收到的原样加前缀发回
            let mut reply = b"ACK:".to_vec();
            reply.extend_from_slice(&body);
            let n = reply.len() as u32;
            stream.write_all(&n.to_be_bytes()).unwrap();
            stream.write_all(&reply).unwrap();
        });

        let transport = TcpTransport::default();
        let out = transport
            .exchange(
                &tcp_addr(port),
                &WireMessage {
                    head: Default::default(),
                    body: b"HELLO".to_vec(),
                },
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(out.body, b"ACK:HELLO");
        handle.join().unwrap();
    }

    #[test]
    fn tcp_transport_reports_connection_refused() {
        // 绑定后立刻释放，端口大概率无人监听
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let transport = TcpTransport {
            timeout: Duration::from_millis(500),
        };
        let err = transport
            .exchange(
                &tcp_addr(port),
                &WireMessage {
                    head: Default::default(),
                    body: vec![],
                },
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(matches!(err, TransportError::Unreachable(_)));
        assert!(err.retryable(), "连接失败应可重试（换主备）");
    }

    #[test]
    fn exchange_honours_the_timeout_argument() {
        let (port, handle) = serve_once(|mut stream| {
            // 故意比客户端超时慢：如果 timeout 没传下去，这个用例会挂 300ms 以上
            thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(&0u32.to_be_bytes());
        });

        let started = std::time::Instant::now();
        let err = TcpTransport::default()
            .exchange(
                &tcp_addr(port),
                &WireMessage {
                    head: Default::default(),
                    body: vec![],
                },
                Duration::from_millis(60),
            )
            .unwrap_err();
        let elapsed = started.elapsed();

        assert!(
            matches!(
                err,
                TransportError::Timeout | TransportError::Unreachable(_)
            ),
            "实际：{err:?}"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "应按传入的 60ms 超时返回，而不是等服务器 300ms（实际 {elapsed:?}）"
        );
        handle.join().unwrap();
    }

    #[test]
    fn tcp_transport_rejects_http_address() {
        let err = TcpTransport::default()
            .exchange(
                &EndpointAddr::Http {
                    base_url: "http://x".into(),
                },
                &WireMessage::default(),
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
    }

    #[test]
    fn http_transport_sends_post_and_parses_json_reply() {
        let (port, handle) = serve_once(|mut stream| {
            let request = read_http_request(&mut stream);
            assert!(
                String::from_utf8_lossy(&request).starts_with("POST /v1/ping HTTP/1.1"),
                "应把 op 拼进路径"
            );
            let reply = b"{\"payload\":{\"ok\":true}}";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                reply.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(reply).unwrap();
        });

        let mut head = std::collections::BTreeMap::new();
        head.insert("x-op".to_string(), "ping".to_string());
        head.insert("content-type".to_string(), "application/json".to_string());

        let out = HttpTransport::default()
            .exchange(
                &EndpointAddr::Http {
                    base_url: format!("http://127.0.0.1:{port}/v1"),
                },
                &WireMessage {
                    head,
                    body: b"{}".to_vec(),
                },
                Duration::from_secs(5),
            )
            .unwrap();
        assert_eq!(out.body, b"{\"payload\":{\"ok\":true}}");
        assert_eq!(
            out.head.get("content-type").map(String::as_str),
            Some("application/json")
        );
        handle.join().unwrap();
    }

    #[test]
    fn http_transport_surfaces_error_status() {
        let (port, handle) = serve_once(|mut stream| {
            let _ = read_http_request(&mut stream);
            let reply = b"{\"code\":\"TX_REJECTED\"}";
            let head = format!(
                "HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: {}\r\n\r\n",
                reply.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(reply).unwrap();
        });

        let err = HttpTransport::default()
            .exchange(
                &EndpointAddr::Http {
                    base_url: format!("http://127.0.0.1:{port}"),
                },
                &WireMessage {
                    head: Default::default(),
                    body: b"{}".to_vec(),
                },
                Duration::from_secs(5),
            )
            .unwrap_err();
        assert!(err.to_string().contains("422"));
        assert!(!err.retryable(), "4xx 不该触发主备切换");
        handle.join().unwrap();
    }

    #[test]
    fn split_host_port_handles_schemes_and_default_ports() {
        let http = split_host_port("http://localhost/api").unwrap();
        assert_eq!(http.host, "localhost");
        assert_eq!(http.port, 80);
        assert_eq!(http.path, "/api");
        assert!(!http.tls);

        let https = split_host_port("https://bank.internal/v1").unwrap();
        assert_eq!(https.port, 443, "https 默认 443");
        assert!(https.tls);

        let explicit = split_host_port("http://10.0.0.1:8080").unwrap();
        assert_eq!(explicit.port, 8080);
        assert_eq!(explicit.path, "");
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        let err = split_host_port("ftp://x").unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
        assert!(err.to_string().contains("http://"));
    }

    /// 起一个只服务一次的 TLS 服务器，返回 (端口, 证书 DER)。
    fn serve_once_tls(
        handler: impl FnOnce(&mut StreamOwned<rustls::ServerConnection, TcpStream>) + Send + 'static,
    ) -> (u16, CertificateDer<'static>, thread::JoinHandle<()>) {
        let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("生成自签证书");
        let cert_der = certified.cert.der().clone();
        let key_der =
            rustls::pki_types::PrivateKeyDer::Pkcs8(certified.key_pair.serialize_der().into());
        let config = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert_der.clone()], key_der)
            .expect("服务器 TLS 配置");

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            if let Ok((tcp, _)) = listener.accept() {
                let conn = rustls::ServerConnection::new(Arc::new(config)).expect("服务端连接");
                let mut stream = StreamOwned::new(conn, tcp);
                handler(&mut stream);
            }
        });
        (port, cert_der, handle)
    }

    #[test]
    fn https_handshake_succeeds_with_trusted_root() {
        let (port, cert, handle) = serve_once_tls(|stream| {
            let request = read_http_request(stream);
            assert!(
                String::from_utf8_lossy(&request).starts_with("POST /v1/ping HTTP/1.1"),
                "TLS 上的报文内容应与明文一致"
            );
            let reply = b"{\"payload\":{\"ok\":true}}";
            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", reply.len());
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(reply).unwrap();
            // 必须发 close_notify：没有它，客户端会把 EOF 当成被截断（rustls 的正确行为）
            stream.conn.send_close_notify();
            stream.flush().unwrap();
        });

        let transport = HttpTransport::with_extra_roots(vec![cert]);
        let mut head = std::collections::BTreeMap::new();
        head.insert("x-op".to_string(), "ping".to_string());

        let out = transport
            .exchange(
                &EndpointAddr::Http {
                    base_url: format!("https://localhost:{port}/v1"),
                },
                &WireMessage {
                    head,
                    body: b"{}".to_vec(),
                },
                Duration::from_secs(5),
            )
            .expect("TLS 握手与请求应成功");
        assert_eq!(out.body, b"{\"payload\":{\"ok\":true}}");
        handle.join().unwrap();
    }

    #[test]
    fn https_rejects_untrusted_certificate() {
        let (port, _cert, handle) = serve_once_tls(|stream| {
            // 客户端会先握手失败，这里读到的是错误或 EOF，忽略即可
            let _ = read_http_request(stream);
        });

        // 不把自签证书加进信任库：必须失败，而不是"连上就行"
        let err = HttpTransport::default()
            .exchange(
                &EndpointAddr::Http {
                    base_url: format!("https://localhost:{port}"),
                },
                &WireMessage {
                    head: Default::default(),
                    body: b"{}".to_vec(),
                },
                Duration::from_secs(5),
            )
            .unwrap_err();
        let text = err.to_string();
        assert!(
            matches!(
                err,
                TransportError::Unreachable(_) | TransportError::Protocol(_)
            ),
            "实际：{text}"
        );
        handle.join().unwrap();
    }
}

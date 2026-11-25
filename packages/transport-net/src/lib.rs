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
use std::time::Duration;

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
    ) -> Result<WireMessage, TransportError> {
        let EndpointAddr::Tcp { host, port } = addr else {
            return Err(TransportError::Protocol(
                "TcpTransport 只能处理 Tcp 地址".into(),
            ));
        };

        let mut stream = connect(host, *port, self.timeout)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| io_err("设置读超时失败", e))?;
        stream
            .set_write_timeout(Some(self.timeout))
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

/// 最小 HTTP/1.1 传输：`POST {base_url}/{op}`，`Content-Length` 定长。
///
/// 不处理：TLS、重定向、分块编码、连接复用。
/// 这些在接入真实后台前必须补（或换成成熟客户端），已经在文档里记为 TODO。
#[derive(Debug, Clone)]
pub struct HttpTransport {
    pub timeout: Duration,
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(5),
        }
    }
}

fn split_host_port(base_url: &str) -> Result<(String, u16, String), TransportError> {
    let without_scheme = base_url
        .strip_prefix("http://")
        .ok_or_else(|| TransportError::Protocol(format!("只支持 http://：{base_url}")))?;
    let (authority, path) = match without_scheme.find('/') {
        Some(i) => (&without_scheme[..i], &without_scheme[i..]),
        None => (without_scheme, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| TransportError::Protocol(format!("端口非法：{p}")))?,
        ),
        None => (authority.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}

impl Transport for HttpTransport {
    fn name(&self) -> &'static str {
        "http"
    }

    fn exchange(
        &self,
        addr: &EndpointAddr,
        request: &WireMessage,
    ) -> Result<WireMessage, TransportError> {
        let EndpointAddr::Http { base_url } = addr else {
            return Err(TransportError::Protocol(
                "HttpTransport 只能处理 Http 地址".into(),
            ));
        };
        let (host, port, base_path) = split_host_port(base_url)?;
        let op = request.head.get("x-op").cloned().unwrap_or_default();
        let path = format!("{base_path}/{op}");

        let mut stream = connect(&host, port, self.timeout)?;
        stream
            .set_read_timeout(Some(self.timeout))
            .map_err(|e| io_err("设置读超时失败", e))?;

        let mut head = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n",
            request.body.len()
        );
        for (k, v) in &request.head {
            if k == "x-op" {
                continue;
            }
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        head.push_str("\r\n");

        stream
            .write_all(head.as_bytes())
            .and_then(|_| stream.write_all(&request.body))
            .and_then(|_| stream.flush())
            .map_err(|e| io_err("发送失败", e))?;

        let mut raw = Vec::new();
        stream
            .read_to_end(&mut raw)
            .map_err(|e| io_err("读响应失败", e))?;

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
            )
            .unwrap_err();
        assert!(matches!(err, TransportError::Unreachable(_)));
        assert!(err.retryable(), "连接失败应可重试（换主备）");
    }

    #[test]
    fn tcp_transport_rejects_http_address() {
        let err = TcpTransport::default()
            .exchange(
                &EndpointAddr::Http {
                    base_url: "http://x".into(),
                },
                &WireMessage::default(),
            )
            .unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
    }

    #[test]
    fn http_transport_sends_post_and_parses_json_reply() {
        let (port, handle) = serve_once(|mut stream| {
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            // 读到头部结束
            loop {
                let n = stream.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let content_len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    while buf.len() < head_end + content_len {
                        let n = stream.read(&mut chunk).unwrap();
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                    }
                    break;
                }
            }
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
            let mut buf = [0u8; 512];
            let _ = stream.read(&mut buf);
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
            )
            .unwrap_err();
        assert!(err.to_string().contains("422"));
        assert!(!err.retryable(), "4xx 不该触发主备切换");
        handle.join().unwrap();
    }

    #[test]
    fn split_host_port_handles_default_and_explicit_ports() {
        assert_eq!(
            split_host_port("http://localhost/api").unwrap(),
            ("localhost".to_string(), 80, "/api".to_string())
        );
        assert_eq!(
            split_host_port("http://10.0.0.1:8080").unwrap(),
            ("10.0.0.1".to_string(), 8080, "".to_string())
        );
    }
}

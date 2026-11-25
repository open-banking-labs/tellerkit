//! 远端通信抽象：**传输 / 编解码 / 路由**三层分离。
//!
//! 为什么分三层而不是一个 "RemoteClient"：
//!
//! | 变化的东西 | 抽象成 | 例子 |
//! |---|---|---|
//! | 怎么把字节送过去 | [`Transport`] | HTTP/1.1、TCP 短连接、进程内 mock |
//! | 报文长什么样 | [`Codec`] | JSON REST、定长自定义报文、TLV |
//! | 哪个服务找哪个端、失败找谁 | [`Router`] | 一个业务模块对应多个服务端（主备/分片） |
//!
//! 三者独立演进：换成 TCP 短连接不动编解码；改报文格式不动传输；
//! 加一个备用服务端不动前两者。
//!
//! 本模块**不做 IO**：`Transport` 是端口，具体实现在 `packages/transport-net`。
//! 这样路由、主备、错误聚合这些"会出错的地方"可以用 mock 传输精确测试。

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::Value;

/// 线上报文：头部键值 + 字节体。
///
/// 同时能装下 HTTP（head 就是请求头）与自定义报文（head 是报文头字段），
/// 所以传输层不需要知道业务语义。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WireMessage {
    pub head: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

/// 应用层载荷。三种形态覆盖现有与预期的报文：
/// - `Json`：REST 与大多数后台
/// - `Binary`：定长/二进制自定义报文
/// - `Text`：行协议、调试
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Json(Value),
    Binary(Vec<u8>),
    Text(String),
}

impl Body {
    pub fn as_json(&self) -> Option<&Value> {
        match self {
            Body::Json(v) => Some(v),
            _ => None,
        }
    }

    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Body::Json(v) => serde_json::to_vec(v).unwrap_or_default(),
            Body::Binary(b) => b.clone(),
            Body::Text(t) => t.as_bytes().to_vec(),
        }
    }
}

/// 一次调用请求。`service` 是路由键，`op` 是服务内操作名。
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub service: String,
    pub op: String,
    pub body: Body,
}

/// 传输层能连到哪里。由实现解释（HTTP 只看 base_url，TCP 只看 host/port）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointAddr {
    Http { base_url: String },
    Tcp { host: String, port: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TransportError {
    #[error("无法连接：{0}")]
    Unreachable(String),
    #[error("超时")]
    Timeout,
    #[error("协议错误：{0}")]
    Protocol(String),
}

impl TransportError {
    /// 只有"连接不上/超时/协议暂时对不上"值得换下一个端点；
    /// 明确的服务端拒绝不该触发主备切换（会把请求打到不该去的地方）。
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            TransportError::Unreachable(_) | TransportError::Timeout
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CodecError {
    #[error("无法编码：{0}")]
    Encode(String),
    #[error("无法解码：{0}")]
    Decode(String),
}

/// 传输端口：把一段字节送到地址，拿回一段字节。
///
/// 不负责重试、主备、编解码——那些在 [`Router`] 与 [`Codec`] 里。
pub trait Transport: Send + Sync {
    /// 用于日志与排障的名字。
    fn name(&self) -> &'static str;
    /// `timeout` 来自端点配置（`Endpoint::timeout_ms`）。
    /// 由调用方传入而不是传输自己保存：同一种传输可能服务多个端点，
    /// 每个端点的超时策略不同。
    fn exchange(
        &self,
        addr: &EndpointAddr,
        request: &WireMessage,
        timeout: std::time::Duration,
    ) -> Result<WireMessage, TransportError>;
}

/// 编解码端口：领域载荷 ↔ 线上报文。
pub trait Codec: Send + Sync {
    fn name(&self) -> &'static str;
    /// 把一次调用编成线上报文。`path` 由端点决定（HTTP 用，TCP 忽略）。
    fn encode(&self, request: &Request) -> Result<WireMessage, CodecError>;
    fn decode(&self, wire: &WireMessage) -> Result<Body, CodecError>;
}

/// 一个端点的完整绑定：地址 + 传输 + 编解码 + 策略。
///
/// "同一服务多个端点"就是多条 `Endpoint`，用 `priority` 排序（0 最高）。
pub struct Endpoint {
    pub service: String,
    pub addr: EndpointAddr,
    pub transport: Arc<dyn Transport>,
    pub codec: Arc<dyn Codec>,
    pub priority: u8,
    pub timeout_ms: u64,
}

impl std::fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Endpoint")
            .field("service", &self.service)
            .field("addr", &self.addr)
            .field("transport", &self.transport.name())
            .field("codec", &self.codec.name())
            .field("priority", &self.priority)
            .field("timeout_ms", &self.timeout_ms)
            .finish()
    }
}

/// 路由失败：保留每个端点的失败原因，排障时能看出"主备都试过，各自为什么失败"。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("服务 {service} 调用失败（尝试 {attempts} 个端点）：{detail}")]
pub struct RouteError {
    pub service: String,
    pub attempts: usize,
    pub detail: String,
    /// 是否属于"换个端点可能成功"。全为不可重试错误时为 false。
    pub retryable: bool,
}

/// 按服务名路由，按优先级主备切换。
#[derive(Default)]
pub struct Router {
    /// 已按 (service, priority) 排好序
    endpoints: Vec<Endpoint>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("endpoints", &self.endpoints)
            .finish()
    }
}

impl Router {
    /// `endpoints` 可以乱序传入；内部按服务与优先级排序，保证顺序可复现。
    pub fn new(mut endpoints: Vec<Endpoint>) -> Self {
        endpoints.sort_by(|a, b| {
            a.service
                .cmp(&b.service)
                .then(a.priority.cmp(&b.priority))
                .then(a.transport.name().cmp(b.transport.name()))
        });
        Self { endpoints }
    }

    /// 已登记的服务名（去重、有序）。
    pub fn services(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.endpoints.iter().map(|e| e.service.as_str()).collect();
        names.dedup();
        names
    }

    pub fn endpoints_of(&self, service: &str) -> Vec<&Endpoint> {
        self.endpoints
            .iter()
            .filter(|e| e.service == service)
            .collect()
    }

    /// 发起调用：按优先级逐个尝试，可重试的失败才切换下一个端点。
    ///
    /// 为什么"不可重试"就停：主备切换的前提是"这个端点坏了"。
    /// 如果服务端明确拒绝了业务（比如余额不足），换台机器照样拒绝，
    /// 切换只会把同一笔请求打到不该去的地方。
    pub fn call(&self, request: &Request) -> Result<Body, RouteError> {
        let candidates: Vec<&Endpoint> = self
            .endpoints
            .iter()
            .filter(|e| e.service == request.service)
            .collect();

        if candidates.is_empty() {
            return Err(RouteError {
                service: request.service.clone(),
                attempts: 0,
                detail: "没有为该服务登记任何端点".to_string(),
                retryable: false,
            });
        }

        let mut failures = Vec::new();
        let mut all_retryable = true;
        for endpoint in &candidates {
            let wire = match endpoint.codec.encode(request) {
                Ok(w) => w,
                Err(e) => {
                    // 编码错误与端点无关，换端点也不会好，直接返回
                    return Err(RouteError {
                        service: request.service.clone(),
                        attempts: failures.len() + 1,
                        detail: format!("{} 编码失败：{e}", endpoint.codec.name()),
                        retryable: false,
                    });
                }
            };

            let timeout = std::time::Duration::from_millis(endpoint.timeout_ms.max(1));
            match endpoint.transport.exchange(&endpoint.addr, &wire, timeout) {
                Ok(response) => match endpoint.codec.decode(&response) {
                    Ok(body) => return Ok(body),
                    Err(e) => {
                        failures.push(format!(
                            "{}@{} 解码失败：{e}",
                            endpoint.transport.name(),
                            endpoint.priority
                        ));
                        continue;
                    }
                },
                Err(e) if e.retryable() => {
                    failures.push(format!(
                        "{}@{} {}",
                        endpoint.transport.name(),
                        endpoint.priority,
                        e
                    ));
                    continue;
                }
                Err(e) => {
                    all_retryable = false;
                    failures.push(format!(
                        "{}@{} {}",
                        endpoint.transport.name(),
                        endpoint.priority,
                        e
                    ));
                    break;
                }
            }
        }

        Err(RouteError {
            service: request.service.clone(),
            attempts: failures.len(),
            detail: failures.join("；"),
            retryable: all_retryable,
        })
    }
}

// ---------------------------------------------------------------------------
// 编解码实现
// ---------------------------------------------------------------------------

/// JSON 编解码：`Content-Type: application/json`，body 是 `{"op":..., "payload":...}`。
///
/// 对应 REST 后台。`op` 放在 body 里而不是 URL，是因为很多网关会改写 URL。
#[derive(Debug, Default, Clone)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    fn name(&self) -> &'static str {
        "json"
    }

    fn encode(&self, request: &Request) -> Result<WireMessage, CodecError> {
        let payload = match &request.body {
            Body::Json(v) => v.clone(),
            Body::Text(t) => Value::String(t.clone()),
            Body::Binary(_) => {
                return Err(CodecError::Encode(
                    "JSON 编解码不支持二进制载荷（请用定长编解码）".into(),
                ))
            }
        };
        let envelope = serde_json::json!({ "op": request.op, "payload": payload });
        let body = serde_json::to_vec(&envelope)
            .map_err(|e| CodecError::Encode(format!("序列化失败：{e}")))?;
        let mut head = BTreeMap::new();
        head.insert("content-type".to_string(), "application/json".to_string());
        head.insert("x-op".to_string(), request.op.clone());
        Ok(WireMessage { head, body })
    }

    fn decode(&self, wire: &WireMessage) -> Result<Body, CodecError> {
        let value: Value = serde_json::from_slice(&wire.body)
            .map_err(|e| CodecError::Decode(format!("JSON 解析失败：{e}")))?;
        // 后台可能返回 { "payload": ... } 或裸对象，两种都接受
        Ok(Body::Json(value.get("payload").cloned().unwrap_or(value)))
    }
}

/// 定长报文字段定义（ASCII，不足补空格/前导零）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldSpec {
    pub name: String,
    pub width: usize,
    /// true = 右对齐补零（金额、序号）；false = 左对齐补空格（名称）
    pub numeric: bool,
}

impl FieldSpec {
    pub fn text(name: &str, width: usize) -> Self {
        Self {
            name: name.to_string(),
            width,
            numeric: false,
        }
    }

    pub fn numeric(name: &str, width: usize) -> Self {
        Self {
            name: name.to_string(),
            width,
            numeric: true,
        }
    }
}

/// 定长自定义报文编解码。
///
/// 报文形如：`[4 字节大端长度][按 layout 拼接的 ASCII 字段]`。
/// 这是很多行内老系统的实际形态（不 XML、不 JSON），所以必须能进这套抽象。
#[derive(Debug, Clone)]
pub struct FixedWidthCodec {
    pub layout: Vec<FieldSpec>,
}

impl FixedWidthCodec {
    pub fn new(layout: Vec<FieldSpec>) -> Self {
        Self { layout }
    }

    fn total_width(&self) -> usize {
        self.layout.iter().map(|f| f.width).sum()
    }
}

impl Codec for FixedWidthCodec {
    fn name(&self) -> &'static str {
        "fixed-width"
    }

    fn encode(&self, request: &Request) -> Result<WireMessage, CodecError> {
        let Body::Json(value) = &request.body else {
            return Err(CodecError::Encode("定长编解码需要 JSON 载荷".into()));
        };
        let mut out = String::with_capacity(self.total_width());
        for field in &self.layout {
            let raw = match value.get(&field.name) {
                Some(Value::String(s)) => s.clone(),
                Some(Value::Number(n)) => n.to_string(),
                Some(Value::Bool(b)) => b.to_string(),
                Some(Value::Null) | None => String::new(),
                Some(other) => {
                    return Err(CodecError::Encode(format!(
                        "字段 {} 不是标量：{other}",
                        field.name
                    )))
                }
            };
            if raw.len() > field.width {
                return Err(CodecError::Encode(format!(
                    "字段 {} 超出 {} 字节（实际 {}）",
                    field.name,
                    field.width,
                    raw.len()
                )));
            }
            // 定长报文按**字节**对齐：format! 的 width 是字符数，CJK 会算错
            let pad = field.width - raw.len();
            if field.numeric {
                out.push_str(&"0".repeat(pad));
                out.push_str(&raw);
            } else {
                out.push_str(&raw);
                out.push_str(&" ".repeat(pad));
            }
        }

        let mut body = (out.len() as u32).to_be_bytes().to_vec();
        body.extend_from_slice(out.as_bytes());

        let mut head = BTreeMap::new();
        head.insert("x-codec".to_string(), "fixed-width".to_string());
        head.insert("x-op".to_string(), request.op.clone());
        Ok(WireMessage { head, body })
    }

    fn decode(&self, wire: &WireMessage) -> Result<Body, CodecError> {
        if wire.body.len() < 4 {
            return Err(CodecError::Decode("报文不足 4 字节长度头".into()));
        }
        let len =
            u32::from_be_bytes([wire.body[0], wire.body[1], wire.body[2], wire.body[3]]) as usize;
        let payload = &wire.body[4..];
        if payload.len() < len {
            return Err(CodecError::Decode(format!(
                "长度头声明 {len} 字节，实际 {}",
                payload.len()
            )));
        }
        if len != self.total_width() {
            return Err(CodecError::Decode(format!(
                "报文长度 {len} 与布局 {} 不符",
                self.total_width()
            )));
        }

        let mut map = serde_json::Map::new();
        let mut offset = 0;
        for field in &self.layout {
            // 按字节切：定长布局是字节偏移，切完再用 lossy 转字符串，
            // 这样即使字段里混入非 ASCII 也不会 panic（会显示替换字符）
            let chunk = &payload[offset..offset + field.width];
            offset += field.width;
            let slice = String::from_utf8_lossy(chunk);
            let slice = slice.as_ref();
            let trimmed = if field.numeric {
                slice.trim_start_matches('0')
            } else {
                slice.trim_end()
            };
            let value = if field.numeric && trimmed.is_empty() {
                Value::Number(0.into())
            } else if field.numeric {
                trimmed
                    .parse::<i64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::String(trimmed.to_string()))
            } else {
                Value::String(trimmed.to_string())
            };
            map.insert(field.name.clone(), value);
        }
        Ok(Body::Json(Value::Object(map)))
    }
}

/// 原样透传编解码：请求体即线上体，响应当二进制返回。
///
/// 用于"下载签名菜单包"这类**非结构化**载荷。和 `JsonCodec` 放同一个抽象里，
/// 是为了让同一套路由既能处理 JSON 接口、又能处理二进制下载。
#[derive(Debug, Default, Clone)]
pub struct RawCodec;

impl Codec for RawCodec {
    fn name(&self) -> &'static str {
        "raw"
    }

    fn encode(&self, request: &Request) -> Result<WireMessage, CodecError> {
        match &request.body {
            Body::Binary(b) => {
                let mut head = BTreeMap::new();
                head.insert("x-op".to_string(), request.op.clone());
                Ok(WireMessage {
                    head,
                    body: b.clone(),
                })
            }
            _ => Err(CodecError::Encode("raw 编解码只接受二进制载荷".to_string())),
        }
    }

    fn decode(&self, wire: &WireMessage) -> Result<Body, CodecError> {
        Ok(Body::Binary(wire.body.clone()))
    }
}

// ---------------------------------------------------------------------------
// 测试用传输
// ---------------------------------------------------------------------------

/// 脚本化传输：按顺序返回预置结果，并记录收到的请求。
///
/// 路由的主备切换、错误聚合都靠它测——不需要真网络。
pub struct MockTransport {
    name: &'static str,
    script: std::sync::Mutex<std::collections::VecDeque<Result<WireMessage, TransportError>>>,
    seen: std::sync::Mutex<Vec<WireMessage>>,
}

impl MockTransport {
    pub fn new(name: &'static str, script: Vec<Result<WireMessage, TransportError>>) -> Self {
        Self {
            name,
            script: std::sync::Mutex::new(script.into()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 永远成功的回声传输：把请求体原样返回，便于测编解码往返。
    pub fn echoing(name: &'static str) -> Self {
        Self {
            name,
            script: std::sync::Mutex::new(std::collections::VecDeque::new()),
            seen: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn calls(&self) -> usize {
        self.seen.lock().map(|v| v.len()).unwrap_or(0)
    }
}

impl Transport for MockTransport {
    fn name(&self) -> &'static str {
        self.name
    }

    fn exchange(
        &self,
        _addr: &EndpointAddr,
        request: &WireMessage,
        _timeout: std::time::Duration,
    ) -> Result<WireMessage, TransportError> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(request.clone());
        }
        let next = self.script.lock().ok().and_then(|mut s| s.pop_front());
        match next {
            Some(result) => result,
            None => Ok(request.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn endpoint(
        service: &str,
        priority: u8,
        transport: Arc<dyn Transport>,
        codec: Arc<dyn Codec>,
    ) -> Endpoint {
        Endpoint {
            service: service.to_string(),
            addr: EndpointAddr::Tcp {
                host: "127.0.0.1".into(),
                port: 1,
            },
            transport,
            codec,
            priority,
            timeout_ms: 1000,
        }
    }

    fn request(service: &str, op: &str, body: Value) -> Request {
        Request {
            service: service.to_string(),
            op: op.to_string(),
            body: Body::Json(body),
        }
    }

    #[test]
    fn primary_failure_falls_back_to_secondary() {
        let primary = Arc::new(MockTransport::new(
            "tcp",
            vec![Err(TransportError::Unreachable("连接被拒".into()))],
        ));
        let secondary = Arc::new(MockTransport::echoing("http"));
        let json = Arc::new(JsonCodec);

        let router = Router::new(vec![
            endpoint("acct.core", 0, primary.clone(), json.clone()),
            endpoint("acct.core", 1, secondary.clone(), json.clone()),
        ]);

        let out = router
            .call(&request("acct.core", "ping", json!({"a": 1})))
            .expect("备用端点应接管");
        // JsonCodec 解码后已剥掉 {"op","payload"} 外壳，返回的就是载荷
        assert_eq!(out.as_json().unwrap(), &json!({"a": 1}));
        assert_eq!(primary.calls(), 1);
        assert_eq!(secondary.calls(), 1);
    }

    #[test]
    fn non_retryable_error_does_not_switch_endpoint() {
        let primary = Arc::new(MockTransport::new(
            "tcp",
            vec![Err(TransportError::Protocol("报文头非法".into()))],
        ));
        let secondary = Arc::new(MockTransport::echoing("http"));

        let router = Router::new(vec![
            endpoint("acct.core", 0, primary.clone(), Arc::new(JsonCodec)),
            endpoint("acct.core", 1, secondary.clone(), Arc::new(JsonCodec)),
        ]);

        let err = router
            .call(&request("acct.core", "ping", json!({})))
            .unwrap_err();
        assert_eq!(err.attempts, 1, "协议错误不该触发主备切换");
        assert!(err.detail.contains("报文头非法"));
        assert_eq!(secondary.calls(), 0);
    }

    #[test]
    fn unknown_service_reports_clearly() {
        let router = Router::new(vec![]);
        let err = router.call(&request("nope", "op", json!({}))).unwrap_err();
        assert_eq!(err.attempts, 0);
        assert!(err.detail.contains("没有为该服务登记"));
    }

    #[test]
    fn different_services_can_use_different_transports_and_codecs() {
        let acct = Arc::new(MockTransport::echoing("http"));
        let card = Arc::new(MockTransport::echoing("tcp"));
        let fixed = Arc::new(FixedWidthCodec::new(vec![
            FieldSpec::numeric("amount", 10),
            FieldSpec::text("memo", 6),
        ]));

        let router = Router::new(vec![
            endpoint("acct.core", 0, acct.clone(), Arc::new(JsonCodec)),
            endpoint("card.auth", 0, card.clone(), fixed.clone()),
        ]);

        // 两个服务各走各的传输与编解码
        let a = router
            .call(&request("acct.core", "ping", json!({"x": 1})))
            .unwrap();
        assert!(a.as_json().is_some());

        let c = router
            .call(&request(
                "card.auth",
                "authorize",
                json!({"amount": 100, "memo": "ok"}),
            ))
            .unwrap();
        // 定长回程被解回 JSON
        assert_eq!(c.as_json().unwrap()["amount"], json!(100));
        assert_eq!(c.as_json().unwrap()["memo"], json!("ok"));
    }

    #[test]
    fn fixed_width_roundtrip_pads_numeric_and_text() {
        let codec = FixedWidthCodec::new(vec![
            FieldSpec::numeric("seq", 6),
            FieldSpec::text("name", 8),
        ]);
        let req = request("s", "op", json!({"seq": 42, "name": "柜面"}));

        let wire = codec.encode(&req).unwrap();
        // 4 字节长度头 + 14 **字节**定长体（"柜面" 占 6 字节，补 2 个空格）
        assert_eq!(wire.body.len(), 4 + 14);
        let text = std::str::from_utf8(&wire.body[4..]).unwrap();
        assert_eq!(text, "000042柜面  ");

        let back = codec.decode(&wire).unwrap();
        assert_eq!(back.as_json().unwrap()["seq"], json!(42));
        assert_eq!(back.as_json().unwrap()["name"], json!("柜面"));
    }

    #[test]
    fn fixed_width_rejects_overflow_instead_of_truncating() {
        let codec = FixedWidthCodec::new(vec![FieldSpec::text("name", 3)]);
        let err = codec
            .encode(&request("s", "op", json!({"name": "太长太长"})))
            .unwrap_err();
        assert!(
            err.to_string().contains("超出"),
            "截断会静默改数据，必须报错"
        );
    }

    #[test]
    fn json_codec_rejects_binary_payload() {
        let err = JsonCodec
            .encode(&Request {
                service: "s".into(),
                op: "op".into(),
                body: Body::Binary(vec![1, 2, 3]),
            })
            .unwrap_err();
        assert!(err.to_string().contains("二进制"));
    }

    #[test]
    fn router_orders_endpoints_by_priority_regardless_of_input_order() {
        let a = Arc::new(MockTransport::new(
            "tcp",
            vec![Err(TransportError::Timeout)],
        ));
        let b = Arc::new(MockTransport::echoing("http"));
        // 故意乱序传：priority 1 在前
        let router = Router::new(vec![
            endpoint("svc", 1, b.clone(), Arc::new(JsonCodec)),
            endpoint("svc", 0, a.clone(), Arc::new(JsonCodec)),
        ]);
        router.call(&request("svc", "op", json!({}))).unwrap();
        assert_eq!(a.calls(), 1, "priority 0 必须先尝试");
        assert_eq!(b.calls(), 1);
    }
}

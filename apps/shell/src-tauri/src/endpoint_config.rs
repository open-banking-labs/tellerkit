//! 端点配置：把网点下发的 `endpoints.json` 变成 [`Router`]。
//!
//! 契约见 `changes/008-endpoint-config.md`，启动时的 fail-closed 行为见
//! `changes/016-fail-closed-startup.md` 与 `bootstrap.rs`。要点：
//! - 一个服务可有多条端点，按 `priority` 升序主备；
//! - 不同服务可以用不同传输与编解码；
//! - 本模块只负责"解析得对不对"；**配置不成立时是拒绝启动还是回退演示，
//!   由 `bootstrap::resolve_remote` 按构建类型决定**，不在这里偷偷降级。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use tellerkit_core::remote::{
    Codec, Endpoint, EndpointAddr, FieldSpec, FixedWidthCodec, JsonCodec, RawCodec, Router,
    Transport,
};
use tellerkit_transport_net::{HttpTransport, TcpTransport};

/// 配置文件的根结构。
#[derive(Debug, Deserialize)]
pub struct EndpointConfig {
    pub services: BTreeMap<String, Vec<EndpointSpec>>,
    /// 交易服务是否按幂等键去重（`changes/001`）。
    ///
    /// **默认 false**：没写就是没有。写错这一位会让离线挂起变成重复扣账。
    #[serde(default)]
    pub idempotent_tx: bool,
}

/// 加载结果：路由 + 交易服务是否声明幂等。
#[derive(Debug)]
pub struct LoadedEndpoints {
    pub router: Router,
    pub idempotent_tx: bool,
}

#[derive(Debug, Deserialize)]
pub struct EndpointSpec {
    /// `http` | `tcp`
    transport: String,
    /// `json` | `raw` | `fixed-width`
    codec: String,
    /// HTTP 用
    #[serde(default)]
    url: Option<String>,
    /// TCP 用
    #[serde(default)]
    host: Option<String>,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    priority: u8,
    #[serde(default = "default_timeout")]
    timeout_ms: u64,
    /// 定长报文的字段布局（仅 codec = fixed-width 时必填）
    #[serde(default)]
    layout: Option<Vec<FieldLayoutSpec>>,
}

impl EndpointSpec {
    pub fn transport(&self) -> &str {
        &self.transport
    }

    pub fn codec(&self) -> &str {
        &self.codec
    }

    pub fn url(&self) -> Option<&String> {
        self.url.as_ref()
    }

    pub fn host(&self) -> Option<&String> {
        self.host.as_ref()
    }

    pub fn port(&self) -> Option<u16> {
        self.port
    }

    pub fn priority(&self) -> u8 {
        self.priority
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn layout(&self) -> Option<&[FieldLayoutSpec]> {
        self.layout.as_deref()
    }
}

#[derive(Debug, Deserialize)]
pub struct FieldLayoutSpec {
    name: String,
    width: usize,
    #[serde(default)]
    numeric: bool,
}

impl FieldLayoutSpec {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn numeric(&self) -> bool {
        self.numeric
    }
}

fn default_timeout() -> u64 {
    3000
}

/// 解析并构建路由。任何一条端点非法都整体失败——不猜、不跳过。
///
/// 顺序：先验签（如果要求），再解析。**先验签后解析**的原因：
/// 未签名的配置可能被改成任意内容，解析它等于信任它。
pub fn load_router(path: &Path) -> Result<LoadedEndpoints, String> {
    load_router_with(path, &LoadPolicy::from_env())
}

/// 加载策略。抽出来是为了能测——直接读环境变量的函数在并行测试里不可靠。
#[derive(Debug, Clone, Default)]
pub struct LoadPolicy {
    /// 配置公钥（hex）。`None` 表示未配置。
    pub pubkey: Option<String>,
    /// 是否强制要求签名。
    pub require_signed: bool,
    /// 是否禁止明文端点（生产应为 true）。
    pub require_tls: bool,
}

impl LoadPolicy {
    pub fn from_env() -> Self {
        Self {
            pubkey: crate::config_sig::config_pubkey(),
            require_signed: crate::config_sig::require_signed(),
            require_tls: std::env::var("TK_REQUIRE_TLS")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}

pub fn load_router_with(path: &Path, policy: &LoadPolicy) -> Result<LoadedEndpoints, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("读取端点配置失败（{}）：{e}", path.display()))?;
    let raw = String::from_utf8(bytes.clone()).map_err(|e| format!("端点配置不是 UTF-8：{e}"))?;

    if policy.require_signed {
        let pubkey = policy
            .pubkey
            .as_deref()
            .ok_or_else(|| "要求配置签名，但未设置 TK_CONFIG_PUBKEY".to_string())?;
        let sig_path = signature_path(path);
        let sig = std::fs::read_to_string(&sig_path)
            .map_err(|_| format!("缺少签名文件 {}", sig_path.display()))?;
        crate::config_sig::verify_detached(&bytes, &sig, pubkey)
            .map_err(|e| format!("端点配置验签失败：{e}"))?;
    }

    parse_full(&raw, policy.require_tls)
}

/// `endpoints.json` → `endpoints.json.sig`
fn signature_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".sig");
    path.with_file_name(name)
}

pub fn parse(raw: &str) -> Result<Router, String> {
    parse_full(raw, false).map(|loaded| loaded.router)
}

/// `require_tls` 为真时拒绝一切 `http://` 端点（release 构建强制为真）。
pub fn parse_with(raw: &str, require_tls: bool) -> Result<Router, String> {
    parse_full(raw, require_tls).map(|loaded| loaded.router)
}

pub fn parse_full(raw: &str, require_tls: bool) -> Result<LoadedEndpoints, String> {
    let config: EndpointConfig =
        serde_json::from_str(raw).map_err(|e| format!("端点配置不是合法 JSON：{e}"))?;

    if config.services.is_empty() {
        return Err("端点配置里没有任何服务".to_string());
    }

    let mut endpoints = Vec::new();
    for (service, specs) in &config.services {
        if specs.is_empty() {
            return Err(format!("服务 {service} 没有配置任何端点"));
        }
        for spec in specs {
            if require_tls {
                if let Some(url) = spec.url() {
                    if url.starts_with("http://") {
                        return Err(format!(
                            "{service}: 已要求 TLS（TK_REQUIRE_TLS=1），拒绝明文端点 {url}"
                        ));
                    }
                }
            }
            endpoints.push(build(service, spec)?);
        }
    }
    Ok(LoadedEndpoints {
        router: Router::new(endpoints),
        idempotent_tx: config.idempotent_tx,
    })
}

fn build(service: &str, spec: &EndpointSpec) -> Result<Endpoint, String> {
    let addr = match spec.transport() {
        "http" => {
            let url = spec
                .url()
                .cloned()
                .ok_or_else(|| format!("{service}: http 端点必须提供 url"))?;
            if !url.starts_with("http://") && !url.starts_with("https://") {
                return Err(format!("{service}: url 必须以 http:// 或 https:// 开头"));
            }
            EndpointAddr::Http { base_url: url }
        }
        "tcp" => EndpointAddr::Tcp {
            host: spec
                .host()
                .cloned()
                .ok_or_else(|| format!("{service}: tcp 端点必须提供 host"))?,
            port: spec
                .port()
                .ok_or_else(|| format!("{service}: tcp 端点必须提供 port"))?,
        },
        other => {
            return Err(format!(
                "{service}: 不支持的 transport `{other}`（可选 http / tcp）"
            ))
        }
    };

    let transport: Arc<dyn Transport> = match spec.transport() {
        "http" => Arc::new(HttpTransport::default()),
        _ => Arc::new(TcpTransport::default()),
    };

    let codec: Arc<dyn Codec> = match spec.codec() {
        "json" => Arc::new(JsonCodec),
        "raw" => Arc::new(RawCodec),
        "fixed-width" => {
            let layout = spec
                .layout()
                .ok_or_else(|| format!("{service}: fixed-width 端点必须提供 layout"))?;
            if layout.is_empty() {
                return Err(format!("{service}: fixed-width 的 layout 不能为空"));
            }
            Arc::new(FixedWidthCodec::new(
                layout
                    .iter()
                    .map(|f| {
                        if f.numeric() {
                            FieldSpec::numeric(f.name(), f.width())
                        } else {
                            FieldSpec::text(f.name(), f.width())
                        }
                    })
                    .collect(),
            ))
        }
        other => {
            return Err(format!(
                "{service}: 不支持的 codec `{other}`（可选 json / raw / fixed-width）"
            ))
        }
    };

    Ok(Endpoint::new(
        service,
        addr,
        transport,
        codec,
        spec.priority(),
        spec.timeout_ms(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = r#"{
      "services": {
        "menu.core": [
          { "transport": "http", "codec": "json", "url": "http://menu.internal/v1" }
        ],
        "acct.core": [
          { "transport": "http", "codec": "json", "url": "http://acct.internal/v1" },
          { "transport": "tcp", "codec": "json", "host": "10.0.8.11", "port": 7001, "priority": 1 }
        ],
        "card.auth": [
          { "transport": "tcp", "codec": "fixed-width", "host": "10.0.9.20", "port": 7002,
            "layout": [
              { "name": "amount", "width": 12, "numeric": true },
              { "name": "memo", "width": 20 }
            ] }
        ]
      }
    }"#;

    #[test]
    fn builds_router_with_multiple_endpoints_per_service() {
        let router = parse(OK).expect("配置合法");
        let mut services = router.services();
        services.sort();
        assert_eq!(services, vec!["acct.core", "card.auth", "menu.core"]);

        let acct = router.endpoints_of("acct.core");
        assert_eq!(acct.len(), 2, "同一服务两条端点");
        assert_eq!(acct[0].priority(), 0);
        assert_eq!(acct[1].priority(), 1);
        assert_eq!(acct[0].transport().name(), "http");
        assert_eq!(acct[1].transport().name(), "tcp");
    }

    #[test]
    fn different_services_can_use_different_codecs() {
        let router = parse(OK).expect("配置合法");
        assert_eq!(
            router.endpoints_of("card.auth")[0].codec().name(),
            "fixed-width"
        );
        assert_eq!(router.endpoints_of("menu.core")[0].codec().name(), "json");
    }

    #[test]
    fn idempotent_tx_defaults_to_false_and_can_be_enabled() {
        let without = parse_full(OK, false).expect("合法");
        assert!(!without.idempotent_tx, "没写就是没有");

        let with = OK.replace(
            "{\n      \"services\"",
            "{\n      \"idempotent_tx\": true,\n      \"services\"",
        );
        assert!(parse_full(&with, false).expect("合法").idempotent_tx);
    }

    #[test]
    fn unknown_transport_is_rejected_not_skipped() {
        let bad = r#"{ "services": { "x": [
            { "transport": "grpc", "codec": "json", "url": "http://x" } ] } }"#;
        let err = parse(bad).unwrap_err();
        assert!(err.contains("不支持的 transport"), "实际：{err}");
    }

    #[test]
    fn tcp_without_port_is_rejected() {
        let bad = r#"{ "services": { "x": [
            { "transport": "tcp", "codec": "json", "host": "1.2.3.4" } ] } }"#;
        assert!(parse(bad).unwrap_err().contains("port"));
    }

    #[test]
    fn fixed_width_without_layout_is_rejected() {
        let bad = r#"{ "services": { "x": [
            { "transport": "tcp", "codec": "fixed-width", "host": "1.2.3.4", "port": 1 } ] } }"#;
        assert!(parse(bad).unwrap_err().contains("layout"));
    }

    #[test]
    fn non_http_scheme_is_rejected() {
        let cfg = r#"{ "services": { "x": [
            { "transport": "http", "codec": "json", "url": "ftp://bank.internal" } ] } }"#;
        assert!(parse(cfg).unwrap_err().contains("http://"));
    }

    #[test]
    fn signed_config_is_verified_before_parse() {
        use ed25519_dalek::{Signer, SigningKey};
        use rand::rngs::OsRng;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("endpoints.json");
        let payload = OK.as_bytes();
        std::fs::write(&path, payload).unwrap();

        let key = SigningKey::generate(&mut OsRng);
        let pubkey = tellerkit_core::pack::pubkey_to_hex(&key.verifying_key());
        let sig_path = dir.path().join("endpoints.json.sig");
        std::fs::write(&sig_path, hex::encode(key.sign(payload).to_bytes())).unwrap();

        let policy = LoadPolicy {
            pubkey: Some(pubkey),
            require_signed: true,
            require_tls: false,
        };
        assert!(load_router_with(&path, &policy).is_ok(), "合法签名应通过");

        // 篡改一个字节：必须失败，而不是回退到"没签名也能用"
        let mut tampered = payload.to_vec();
        tampered.push(b' ');
        std::fs::write(&path, &tampered).unwrap();
        let err = load_router_with(&path, &policy).unwrap_err();
        assert!(err.contains("验签失败"), "实际：{err}");
    }

    #[test]
    fn missing_signature_is_refused_when_required() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("endpoints.json");
        std::fs::write(&path, OK).unwrap();
        let policy = LoadPolicy {
            pubkey: Some("00".repeat(32)),
            require_signed: true,
            require_tls: false,
        };
        let err = load_router_with(&path, &policy).unwrap_err();
        assert!(err.contains("缺少签名文件"), "实际：{err}");
    }

    #[test]
    fn empty_services_is_rejected() {
        assert!(parse(r#"{ "services": {} }"#)
            .unwrap_err()
            .contains("没有任何服务"));
    }
}

//! 端点配置：把网点下发的 `endpoints.json` 变成 [`Router`]。
//!
//! 契约见 `changes/008-endpoint-config.md`。要点：
//! - 一个服务可有多条端点，按 `priority` 升序主备；
//! - 不同服务可以用不同传输与编解码；
//! - 配置缺失/非法 → 回退 mock，并在启动 notes 里写明原因（柜面宁可降级也不白屏）。

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
}

#[derive(Debug, Deserialize)]
pub struct EndpointSpec {
    /// `http` | `tcp`
    pub transport: String,
    /// `json` | `raw` | `fixed-width`
    pub codec: String,
    /// HTTP 用
    #[serde(default)]
    pub url: Option<String>,
    /// TCP 用
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub priority: u8,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    /// 定长报文的字段布局（仅 codec = fixed-width 时必填）
    #[serde(default)]
    pub layout: Option<Vec<FieldLayoutSpec>>,
}

#[derive(Debug, Deserialize)]
pub struct FieldLayoutSpec {
    pub name: String,
    pub width: usize,
    #[serde(default)]
    pub numeric: bool,
}

fn default_timeout() -> u64 {
    3000
}

/// 解析并构建路由。任何一条端点非法都整体失败——不猜、不跳过。
pub fn load_router(path: &Path) -> Result<Router, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("读取端点配置失败（{}）：{e}", path.display()))?;
    parse(&raw)
}

pub fn parse(raw: &str) -> Result<Router, String> {
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
            endpoints.push(build(service, spec)?);
        }
    }
    Ok(Router::new(endpoints))
}

fn build(service: &str, spec: &EndpointSpec) -> Result<Endpoint, String> {
    let addr = match spec.transport.as_str() {
        "http" => EndpointAddr::Http {
            base_url: spec
                .url
                .clone()
                .ok_or_else(|| format!("{service}: http 端点必须提供 url"))?,
        },
        "tcp" => EndpointAddr::Tcp {
            host: spec
                .host
                .clone()
                .ok_or_else(|| format!("{service}: tcp 端点必须提供 host"))?,
            port: spec
                .port
                .ok_or_else(|| format!("{service}: tcp 端点必须提供 port"))?,
        },
        other => {
            return Err(format!(
                "{service}: 不支持的 transport `{other}`（可选 http / tcp）"
            ))
        }
    };

    let transport: Arc<dyn Transport> = match spec.transport.as_str() {
        "http" => Arc::new(HttpTransport::default()),
        _ => Arc::new(TcpTransport::default()),
    };

    let codec: Arc<dyn Codec> = match spec.codec.as_str() {
        "json" => Arc::new(JsonCodec),
        "raw" => Arc::new(RawCodec),
        "fixed-width" => {
            let layout = spec
                .layout
                .as_ref()
                .ok_or_else(|| format!("{service}: fixed-width 端点必须提供 layout"))?;
            if layout.is_empty() {
                return Err(format!("{service}: fixed-width 的 layout 不能为空"));
            }
            Arc::new(FixedWidthCodec::new(
                layout
                    .iter()
                    .map(|f| {
                        if f.numeric {
                            FieldSpec::numeric(&f.name, f.width)
                        } else {
                            FieldSpec::text(&f.name, f.width)
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

    Ok(Endpoint {
        service: service.to_string(),
        addr,
        transport,
        codec,
        priority: spec.priority,
        timeout_ms: spec.timeout_ms,
    })
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
        assert_eq!(acct[0].priority, 0);
        assert_eq!(acct[1].priority, 1);
        assert_eq!(acct[0].transport.name(), "http");
        assert_eq!(acct[1].transport.name(), "tcp");
    }

    #[test]
    fn different_services_can_use_different_codecs() {
        let router = parse(OK).expect("配置合法");
        assert_eq!(
            router.endpoints_of("card.auth")[0].codec.name(),
            "fixed-width"
        );
        assert_eq!(router.endpoints_of("menu.core")[0].codec.name(), "json");
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
    fn empty_services_is_rejected() {
        assert!(parse(r#"{ "services": {} }"#)
            .unwrap_err()
            .contains("没有任何服务"));
    }
}

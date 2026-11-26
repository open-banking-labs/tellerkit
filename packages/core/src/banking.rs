//! 远端银行端口与"路由化实现"。
//!
//! 这里是边界 ③ 的**端口定义** + 一个通用适配器：
//! - [`RemoteBanking`]：应用层看到的"银行后台"，只有三个能力；
//! - [`RoutedBanking`]：用 [`Router`] 把这三个能力落到具体端点，
//!   从而支持"不同业务模块走不同后台、同一模块多端点主备"。
//!
//! 端口放在 core 而不是主壳：这样 fake 后台与适配器可以脱离 Tauri 测试。

use serde::{Deserialize, Serialize};

use crate::perm::PermSnapshot;
use crate::remote::{Body, Request, RouteError, Router};
use crate::tx::{HostError, TxHost, TxRequest};

/// 契约响应里的 shell 兼容范围（contracts/remote-api.md）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct ShellCompat {
    min: u16,
    max: u16,
}

/// 菜单版本探测结果。体积必须很小：它是启动后的第一个后台请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MenuVersionProbe {
    latest: u64,
    shell_compat: ShellCompat,
}

/// 契约形状是嵌套 `shell_compat`；旧后台的扁平字段仍可读。
impl<'de> serde::Deserialize<'de> for MenuVersionProbe {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Wire {
            latest: u64,
            #[serde(default)]
            shell_compat: Option<ShellCompat>,
            #[serde(default, alias = "shell_compat_min")]
            legacy_min: Option<u16>,
            #[serde(default, alias = "shell_compat_max")]
            legacy_max: Option<u16>,
        }

        let wire = Wire::deserialize(deserializer)?;
        let shell_compat = match (wire.shell_compat, wire.legacy_min, wire.legacy_max) {
            (Some(compat), _, _) => compat,
            (None, Some(min), Some(max)) => ShellCompat { min, max },
            _ => {
                return Err(serde::de::Error::custom(
                    "探测响应缺少 shell_compat 兼容范围",
                ))
            }
        };
        Ok(Self {
            latest: wire.latest,
            shell_compat,
        })
    }
}

impl MenuVersionProbe {
    pub fn new(latest: u64, shell_compat_min: u16, shell_compat_max: u16) -> Self {
        Self {
            latest,
            shell_compat: ShellCompat {
                min: shell_compat_min,
                max: shell_compat_max,
            },
        }
    }

    pub fn latest(&self) -> u64 {
        self.latest
    }

    pub fn shell_compat_min(&self) -> u16 {
        self.shell_compat.min
    }

    pub fn shell_compat_max(&self) -> u16 {
        self.shell_compat.max
    }
}

/// 远端调用的错误。**不携带 HTTP 细节**：应用层只需要知道处置方式。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("服务端不可达：{0}")]
    Unavailable(String),
    #[error("服务端超时，状态不明：{0}")]
    Timeout(String),
    #[error("服务端拒绝：{0}")]
    Rejected(String),
    #[error("登录已失效，请重新登录")]
    Unauthorized,
}

/// 菜单来源：启动与版本探测只依赖这两件事。
///
/// 单独拆出来是因为 `sync` 只需要它——不必把整个 `RemoteBanking` 传进去。
pub trait MenuSource: Send + Sync {
    fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError>;
    fn download_menu(&self) -> Result<Vec<u8>, RemoteError>;
}

/// ③ 边界：桌面应用看到的"银行后台"。
///
/// 继承 [`TxHost`]：交易提交与菜单/登录走同一套远端，
/// 分成两个对象只会让应用层多持有一个引用，没有收益。
pub trait RemoteBanking: MenuSource + TxHost + Send + Sync {
    fn login(&self, role_id: &str, perm_version: u64) -> Result<PermSnapshot, RemoteError>;

    /// 请求主机关闭一笔状态不明的交易，返回主机流水号。
    ///
    /// 这是**账务操作**：客户端只发请求，冲正是否成立由主机决定。
    /// 返回的流水号必须写回 `ReversalBook::confirm`，本地才算处置完成。
    fn reverse(&self, idempotency_key: &str, reason: &str) -> Result<String, RemoteError>;
}

/// 服务名。路由键是服务而不是模块：同一个模块可以调多个服务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceNames {
    menu: String,
    pack: String,
    auth: String,
    tx: String,
}

impl ServiceNames {
    pub fn new(menu: &str, pack: &str, auth: &str, tx: &str) -> Self {
        Self {
            menu: menu.to_string(),
            pack: pack.to_string(),
            auth: auth.to_string(),
            tx: tx.to_string(),
        }
    }

    pub fn menu(&self) -> &str {
        &self.menu
    }

    pub fn pack(&self) -> &str {
        &self.pack
    }

    pub fn auth(&self) -> &str {
        &self.auth
    }

    pub fn tx(&self) -> &str {
        &self.tx
    }
}

impl Default for ServiceNames {
    fn default() -> Self {
        Self {
            menu: "menu.core".to_string(),
            pack: "menu.pack".to_string(),
            auth: "auth.core".to_string(),
            tx: "acct.core".to_string(),
        }
    }
}

/// 用 [`Router`] 实现的 `RemoteBanking`。
///
/// 应用层只依赖 `RemoteBanking`；换成 mock/HTTP/TCP 都不需要改用例代码。
pub struct RoutedBanking {
    router: Router,
    services: ServiceNames,
    /// 交易服务是否声明了幂等。未声明 → 客户端不挂起离线交易。
    idempotent: bool,
}

impl RoutedBanking {
    /// 默认**不**假定幂等：接真实后台时必须显式声明。
    pub fn new(router: Router) -> Self {
        Self {
            router,
            services: ServiceNames::default(),
            idempotent: false,
        }
    }

    pub fn with_services(router: Router, services: ServiceNames) -> Self {
        Self {
            router,
            services,
            idempotent: false,
        }
    }

    /// 声明交易服务按幂等键去重（服务端契约要求，见 `changes/001`）。
    pub fn with_idempotency(mut self, idempotent: bool) -> Self {
        self.idempotent = idempotent;
        self
    }

    fn call_json(
        &self,
        service: &str,
        op: &str,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, RemoteError> {
        let body = self
            .router
            .call(&Request::new(service, op, Body::Json(payload)))
            .map_err(remote_err)?;
        let value = body
            .as_json()
            .cloned()
            .ok_or_else(|| RemoteError::Rejected(format!("{service}/{op} 返回的不是 JSON")))?;
        // 服务端用 `{"error": {"code","message","retryable"}}` 表达业务失败
        if let Some(err) = value.get("error") {
            return Err(classify_envelope(err).into_remote());
        }
        Ok(value)
    }
}

/// 错误信封的处置分类。`RemoteError` 与 `HostError` 两个出口共用，避免同一错误码
/// 在一处是拒绝、另一处是超时。
enum EnvelopeFailure {
    Rejected(String),
    Timeout(String),
    Unavailable(String),
}

impl EnvelopeFailure {
    fn into_remote(self) -> RemoteError {
        match self {
            Self::Rejected(msg) => RemoteError::Rejected(msg),
            Self::Timeout(msg) => RemoteError::Timeout(msg),
            Self::Unavailable(msg) => RemoteError::Unavailable(msg),
        }
    }

    fn into_host(self) -> HostError {
        match self {
            Self::Rejected(msg) => HostError::Rejected(msg),
            Self::Timeout(_) => HostError::Timeout,
            Self::Unavailable(_) => HostError::Unavailable,
        }
    }
}

/// 契约按 `code` 分支（contracts/remote-api.md）；`retryable` 只作兜底。
/// 未知码保守按拒绝处理，但把码带进文案，排障时才看得出是哪个新码。
fn classify_envelope(err: &serde_json::Value) -> EnvelopeFailure {
    let code = err.get("code").and_then(|v| v.as_str()).unwrap_or("");
    let msg = error_message(err);
    match code {
        "TX_DUPLICATE_KEY" | "TX_REJECTED" => EnvelopeFailure::Rejected(msg),
        "GATEWAY_TIMEOUT" => EnvelopeFailure::Timeout(msg),
        "SERVICE_UNAVAILABLE" => EnvelopeFailure::Unavailable(msg),
        _ if err
            .get("retryable")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false) =>
        {
            EnvelopeFailure::Unavailable(msg)
        }
        _ => EnvelopeFailure::Rejected(format!("{msg}（未知错误码：{code}）")),
    }
}

/// 从错误信封里取出人可读文案；兼容只剩字符串的旧形态。
fn error_message(err: &serde_json::Value) -> String {
    err.get("message")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| err.as_str().map(str::to_string))
        .unwrap_or_else(|| err.to_string())
}

fn remote_err(e: RouteError) -> RemoteError {
    if e.timed_out() {
        RemoteError::Timeout(e.detail().to_string())
    } else if e.retryable() {
        RemoteError::Unavailable(e.detail().to_string())
    } else {
        RemoteError::Rejected(e.detail().to_string())
    }
}

impl MenuSource for RoutedBanking {
    fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError> {
        let value = self.call_json(&self.services.menu, "probe_menu", serde_json::json!({}))?;
        serde_json::from_value(value)
            .map_err(|e| RemoteError::Rejected(format!("菜单版本响应不合法：{e}")))
    }

    fn download_menu(&self) -> Result<Vec<u8>, RemoteError> {
        let body = self
            .router
            .call(&Request::new(
                &self.services.pack,
                "download",
                Body::Binary(Vec::new()),
            ))
            .map_err(remote_err)?;
        match body {
            Body::Binary(bytes) => Ok(bytes),
            _ => Err(RemoteError::Rejected("菜单包响应不是二进制".to_string())),
        }
    }
}

impl RemoteBanking for RoutedBanking {
    fn login(&self, role_id: &str, perm_version: u64) -> Result<PermSnapshot, RemoteError> {
        let value = self.call_json(
            &self.services.auth,
            "login",
            serde_json::json!({ "role_id": role_id, "perm_version": perm_version }),
        )?;
        serde_json::from_value(value)
            .map_err(|e| RemoteError::Rejected(format!("登录响应不合法：{e}")))
    }

    fn reverse(&self, idempotency_key: &str, reason: &str) -> Result<String, RemoteError> {
        let value = self.call_json(
            &self.services.tx,
            "reverse",
            serde_json::json!({ "idempotency_key": idempotency_key, "reason": reason }),
        )?;
        let reference = value
            .get("reference")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RemoteError::Rejected("冲正响应缺少 reference".to_string()))?;
        if reference.trim().is_empty() {
            return Err(RemoteError::Rejected(
                "冲正响应流水号为空：不能确认冲正，请重试或查账".to_string(),
            ));
        }
        Ok(reference.to_string())
    }
}

impl TxHost for RoutedBanking {
    fn supports_idempotency(&self) -> bool {
        self.idempotent
    }

    fn submit(&self, request: &TxRequest) -> Result<String, HostError> {
        let payload = serde_json::to_value(request)
            .map_err(|e| HostError::Rejected(format!("请求无法序列化：{e}")))?;
        let result = self.router.call(&Request::new(
            &self.services.tx,
            "submit_tx",
            Body::Json(payload),
        ));

        match result {
            Ok(body) => {
                let Some(value) = body.as_json() else {
                    return Err(HostError::Rejected("主机响应不是 JSON".to_string()));
                };
                // 先看错误信封，再看成功字段：否则业务拒绝会被误报成"缺 reference"
                if let Some(err) = value.get("error") {
                    return Err(classify_envelope(err).into_host());
                }
                match value.get("reference") {
                    Some(serde_json::Value::String(r)) => Ok(r.clone()),
                    _ => Err(HostError::Rejected("主机响应缺少 reference".to_string())),
                }
            }
            Err(e) if e.timed_out() => Err(HostError::Timeout),
            Err(e) if e.retryable() => Err(HostError::Unavailable),
            Err(e) => Err(HostError::Rejected(e.detail().to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{Codec, Endpoint, EndpointAddr, JsonCodec, MockTransport, Transport};
    use crate::tx::{TxEngine, TxStatus};
    use std::sync::Arc;

    fn endpoint(service: &str, transport: Arc<dyn Transport>, codec: Arc<dyn Codec>) -> Endpoint {
        Endpoint::new(
            service,
            EndpointAddr::Tcp {
                host: "127.0.0.1".into(),
                port: 1,
            },
            transport,
            codec,
            0,
            1000,
        )
    }

    /// 把 JSON 响应包成 WireMessage（模拟后台返回）。
    fn reply(value: serde_json::Value) -> crate::remote::WireMessage {
        crate::remote::WireMessage::new(Default::default(), serde_json::to_vec(&value).unwrap())
    }

    fn banking_with(transport: Arc<dyn Transport>) -> RoutedBanking {
        RoutedBanking::new(Router::new(vec![endpoint(
            "acct.core",
            transport,
            Arc::new(JsonCodec),
        )]))
    }

    fn status(code: u16) -> crate::remote::TransportError {
        crate::remote::TransportError::Status {
            code,
            body: format!("HTTP {code}"),
        }
    }

    #[test]
    fn probe_parses_contract_shape_and_legacy_flat_shape() {
        let contract: MenuVersionProbe =
            serde_json::from_str(r#"{"latest":7,"shell_compat":{"min":1,"max":3}}"#).unwrap();
        assert_eq!(contract.latest(), 7);
        assert_eq!(contract.shell_compat_min(), 1);
        assert_eq!(contract.shell_compat_max(), 3);

        let legacy: MenuVersionProbe =
            serde_json::from_str(r#"{"latest":7,"shell_compat_min":1,"shell_compat_max":3}"#)
                .unwrap();
        assert_eq!(legacy, contract);
        assert_eq!(
            serde_json::to_value(&contract).unwrap(),
            serde_json::json!({"latest":7,"shell_compat":{"min":1,"max":3}})
        );
    }

    #[test]
    fn routes_each_capability_to_its_own_service() {
        let menu = Arc::new(MockTransport::new(
            "menu",
            vec![Ok(reply(serde_json::json!({
                "payload": { "latest": 9, "shell_compat": { "min": 1, "max": 1 } }
            })))],
        ));
        let auth = Arc::new(MockTransport::new(
            "auth",
            vec![Ok(reply(serde_json::json!({
                "payload": {
                    "user_id": "T01", "teller_no": "T01", "outlet_code": "01023",
                    "perm_version": 901, "menu_codes": ["a"], "action_codes": []
                }
            })))],
        ));
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(
                serde_json::json!({ "payload": { "reference": "TX42" } }),
            ))],
        ));

        let router = Router::new(vec![
            endpoint("menu.core", menu.clone(), Arc::new(JsonCodec)),
            endpoint("auth.core", auth.clone(), Arc::new(JsonCodec)),
            endpoint("acct.core", acct.clone(), Arc::new(JsonCodec)),
        ]);
        let banking = RoutedBanking::new(router);

        assert_eq!(banking.probe_menu().unwrap().latest(), 9);
        assert_eq!(banking.login("teller", 901).unwrap().teller_no(), "T01");

        let mut req = TxRequest::new("account.deposit", "k1");
        req.set_amount_cents(100);
        assert_eq!(banking.submit(&req).unwrap(), "TX42");

        // 三个能力确实打到了三个不同的服务
        assert_eq!(menu.calls(), 1);
        assert_eq!(auth.calls(), 1);
        assert_eq!(acct.calls(), 1);
    }

    #[test]
    fn unreachable_backend_becomes_host_unavailable_not_rejected() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Err(crate::remote::TransportError::Unreachable(
                "连接被拒".into(),
            ))],
        ));
        let router = Router::new(vec![endpoint("acct.core", acct, Arc::new(JsonCodec))]);
        let banking = RoutedBanking::new(router);

        let req = TxRequest::new("account.deposit", "k1");
        assert_eq!(
            banking.submit(&req),
            Err(HostError::Unavailable),
            "连不上要报 Unavailable，否则会当成业务拒绝而不重试"
        );
    }

    #[test]
    fn protocol_error_becomes_rejected_with_detail() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Err(crate::remote::TransportError::Protocol(
                "HTTP 422 余额不足".into(),
            ))],
        ));
        let router = Router::new(vec![endpoint("acct.core", acct, Arc::new(JsonCodec))]);
        let banking = RoutedBanking::new(router);

        let err = banking
            .submit(&TxRequest::new("account.deposit", "k1"))
            .unwrap_err();
        match err {
            HostError::Rejected(msg) => assert!(msg.contains("余额不足")),
            other => panic!("应映射为 Rejected，实际 {other:?}"),
        }
    }

    #[test]
    fn reverse_routes_to_tx_service_and_requires_reference() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(serde_json::json!({
                "payload": { "reference": "REV-9" }
            })))],
        ));
        let router = Router::new(vec![endpoint(
            "acct.core",
            acct.clone(),
            Arc::new(JsonCodec),
        )]);
        let banking = RoutedBanking::new(router);
        assert_eq!(banking.reverse("k1", "状态不明").unwrap(), "REV-9");
        assert_eq!(acct.calls(), 1);

        // 主机没回 reference：必须报错，不能假装成功
        let acct2 = Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(serde_json::json!({ "payload": { "ok": true } })))],
        ));
        let banking2 = RoutedBanking::new(Router::new(vec![endpoint(
            "acct.core",
            acct2,
            Arc::new(JsonCodec),
        )]));
        let err = banking2.reverse("k1", "x").unwrap_err();
        assert!(err.to_string().contains("reference"));

        // 主机回了空白流水号同样不是回执：不能让上层写进 confirm 后清掉异常清单。
        for blank in ["", "   "] {
            let acct3 = Arc::new(MockTransport::new(
                "acct",
                vec![Ok(reply(serde_json::json!({
                    "payload": { "reference": blank }
                })))],
            ));
            let banking3 = RoutedBanking::new(Router::new(vec![endpoint(
                "acct.core",
                acct3,
                Arc::new(JsonCodec),
            )]));
            let err = banking3.reverse("k1", "x").unwrap_err();
            assert!(
                err.to_string().contains("流水号为空"),
                "空白流水号 {blank:?} 必须报错，实际：{err}"
            );
        }
    }

    #[test]
    fn idempotency_is_opt_in_not_assumed() {
        assert!(
            !RoutedBanking::new(Router::new(vec![])).supports_idempotency(),
            "未声明时必须视为不支持"
        );
        assert!(RoutedBanking::new(Router::new(vec![]))
            .with_idempotency(true)
            .supports_idempotency());
    }

    #[test]
    fn business_error_envelope_becomes_rejected_with_message() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(serde_json::json!({
                "payload": { "error": { "code": "TX_REJECTED", "message": "余额不足" } }
            })))],
        ));
        let router = Router::new(vec![endpoint("acct.core", acct, Arc::new(JsonCodec))]);
        let banking = RoutedBanking::new(router);

        let err = banking
            .submit(&TxRequest::new("account.deposit", "k1"))
            .unwrap_err();
        match err {
            HostError::Rejected(msg) => assert_eq!(msg, "余额不足", "应透传服务端文案"),
            other => panic!("应映射为 Rejected，实际 {other:?}"),
        }
    }

    #[test]
    fn missing_reference_is_reported_not_silently_successful() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(serde_json::json!({ "payload": { "ok": true } })))],
        ));
        let router = Router::new(vec![endpoint("acct.core", acct, Arc::new(JsonCodec))]);
        let banking = RoutedBanking::new(router);

        let err = banking
            .submit(&TxRequest::new("account.deposit", "k1"))
            .unwrap_err();
        assert!(err.to_string().contains("reference"));
    }

    #[test]
    fn status_503_becomes_host_unavailable_not_rejected() {
        let banking = banking_with(Arc::new(MockTransport::new("acct", vec![Err(status(503))])));

        assert_eq!(
            banking.submit(&TxRequest::new("account.deposit", "k-503")),
            Err(HostError::Unavailable)
        );
    }

    #[test]
    fn status_504_becomes_marked_for_reversal_not_rejected() {
        let banking = banking_with(Arc::new(MockTransport::new("acct", vec![Err(status(504))])));
        let mut engine = TxEngine::new();

        let result = engine.submit(&banking, TxRequest::new("account.deposit", "k-504"));

        assert_eq!(result.status(), TxStatus::MarkedForReversal);
        assert!(result.message().contains("待冲正"));
    }

    #[test]
    fn status_409_is_rejected_without_switching_endpoint() {
        let primary = Arc::new(MockTransport::new("http", vec![Err(status(409))]));
        let secondary = Arc::new(MockTransport::echoing("http"));
        let banking = RoutedBanking::new(Router::new(vec![
            endpoint("acct.core", primary, Arc::new(JsonCodec)),
            endpoint("acct.core", secondary.clone(), Arc::new(JsonCodec)),
        ]));

        let err = banking
            .submit(&TxRequest::new("account.deposit", "k-409"))
            .unwrap_err();
        match err {
            HostError::Rejected(msg) => assert!(msg.contains("409")),
            other => panic!("应映射为 Rejected，实际 {other:?}"),
        }
        assert_eq!(secondary.calls(), 0, "4xx 不得切端点");
    }

    #[test]
    fn gateway_timeout_envelope_is_marked_for_reversal() {
        let banking = banking_with(Arc::new(MockTransport::new(
            "acct",
            vec![Ok(reply(serde_json::json!({
                "payload": { "error": {
                    "code": "GATEWAY_TIMEOUT", "message": "上游超时" }
                }
            })))],
        )));
        let mut engine = TxEngine::new();

        let result = engine.submit(&banking, TxRequest::new("account.deposit", "k-gt"));

        assert_eq!(result.status(), TxStatus::MarkedForReversal);
    }

    #[test]
    fn error_envelope_codes_follow_the_contract() {
        let cases = [
            (
                "TX_DUPLICATE_KEY",
                "重复键",
                HostError::Rejected("重复键".into()),
            ),
            (
                "TX_REJECTED",
                "余额不足",
                HostError::Rejected("余额不足".into()),
            ),
            ("GATEWAY_TIMEOUT", "上游超时", HostError::Timeout),
            ("SERVICE_UNAVAILABLE", "稍后再试", HostError::Unavailable),
            (
                "FUTURE_CODE",
                "新错误",
                HostError::Rejected("新错误（未知错误码：FUTURE_CODE）".into()),
            ),
        ];

        for (code, message, expected) in cases {
            let banking = banking_with(Arc::new(MockTransport::new(
                "acct",
                vec![Ok(reply(serde_json::json!({
                    "payload": { "error": { "code": code, "message": message } }
                })))],
            )));
            let err = banking
                .submit(&TxRequest::new("account.deposit", "k-codes"))
                .unwrap_err();
            assert_eq!(err, expected, "code={code}");
        }
    }

    #[test]
    fn retry_after_504_reaches_host_with_the_same_key() {
        let acct = Arc::new(MockTransport::new(
            "acct",
            vec![
                Err(status(504)),
                Ok(reply(serde_json::json!({
                    "payload": { "reference": "TX-RECOVERED" }
                }))),
            ],
        ));
        let banking = banking_with(acct.clone());
        let mut engine = TxEngine::new();
        let request = TxRequest::new("account.deposit", "k-retry-504");

        let first = engine.submit(&banking, request.clone());
        assert_eq!(first.status(), TxStatus::MarkedForReversal);

        let second = engine.submit(&banking, request);
        assert_eq!(second.status(), TxStatus::Committed);
        assert_eq!(second.reference(), Some("TX-RECOVERED"));
        assert_eq!(acct.calls(), 2, "504 后同键重试必须再次打到主机");
    }
}

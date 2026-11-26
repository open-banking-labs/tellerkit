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

/// 菜单版本探测结果。体积必须很小：它是启动后的第一个后台请求。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuVersionProbe {
    latest: u64,
    shell_compat_min: u16,
    shell_compat_max: u16,
}

impl MenuVersionProbe {
    pub fn new(latest: u64, shell_compat_min: u16, shell_compat_max: u16) -> Self {
        Self {
            latest,
            shell_compat_min,
            shell_compat_max,
        }
    }

    pub fn latest(&self) -> u64 {
        self.latest
    }

    pub fn shell_compat_min(&self) -> u16 {
        self.shell_compat_min
    }

    pub fn shell_compat_max(&self) -> u16 {
        self.shell_compat_max
    }
}

/// 远端调用的错误。**不携带 HTTP 细节**：应用层只需要知道处置方式。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    #[error("服务端不可达：{0}")]
    Unavailable(String),
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
            return Err(RemoteError::Rejected(error_message(err)));
        }
        Ok(value)
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
    if e.retryable() {
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
        value
            .get("reference")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .ok_or_else(|| RemoteError::Rejected("冲正响应缺少 reference".to_string()))
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
                    return Err(HostError::Rejected(error_message(err)));
                }
                match value.get("reference") {
                    Some(serde_json::Value::String(r)) => Ok(r.clone()),
                    _ => Err(HostError::Rejected("主机响应缺少 reference".to_string())),
                }
            }
            Err(e) if e.retryable() => Err(HostError::Unavailable),
            Err(e) => Err(HostError::Rejected(e.detail().to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::{Codec, Endpoint, EndpointAddr, JsonCodec, MockTransport, Transport};
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

    #[test]
    fn routes_each_capability_to_its_own_service() {
        let menu = Arc::new(MockTransport::new(
            "menu",
            vec![Ok(reply(serde_json::json!({
                "payload": { "latest": 9, "shell_compat_min": 1, "shell_compat_max": 1 }
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
}

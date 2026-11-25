//! 端到端：真 socket 上的 路由 + 编解码 + TCP 传输 + 银行端口。
//!
//! 这条链路就是客户端实际会走的：UI → IPC → 用例 → RoutedBanking → Router
//! → Transport → 假后台。测试里从 `RoutedBanking` 开始，省掉上面的 UI 层。

use std::sync::Arc;

use tellerkit_core::banking::{MenuSource, RemoteBanking, RoutedBanking};
use tellerkit_core::remote::{
    Codec, Endpoint, EndpointAddr, JsonCodec, RawCodec, Router, Transport,
};
use tellerkit_core::tx::{TxHost, TxRequest};
use tellerkit_fake_bank::{FakeBank, FakeBankConfig};
use tellerkit_transport_net::TcpTransport;

fn router_for(bank: &FakeBank) -> Router {
    let addr = EndpointAddr::Tcp {
        host: bank.host(),
        port: bank.port(),
    };
    let make = |service: &str, codec: Arc<dyn Codec>, priority: u8| Endpoint {
        service: service.to_string(),
        addr: addr.clone(),
        transport: Arc::new(TcpTransport::default()) as Arc<dyn Transport>,
        codec,
        priority,
        timeout_ms: 3000,
    };

    Router::new(vec![
        make("menu.core", Arc::new(JsonCodec), 0),
        make("auth.core", Arc::new(JsonCodec), 0),
        make("acct.core", Arc::new(JsonCodec), 0),
        make("menu.pack", Arc::new(RawCodec), 0),
    ])
}

#[test]
fn full_chain_reaches_a_real_backend_over_tcp() {
    let bank = FakeBank::start(FakeBankConfig {
        menu_version: 11,
        ..Default::default()
    });
    let banking = RoutedBanking::new(router_for(&bank));

    let probe = banking.probe_menu().expect("探测菜单版本");
    assert_eq!(probe.latest, 11);

    let snapshot = banking.login("teller", 1101).expect("登录");
    assert_eq!(snapshot.teller_no, "T0231");
    assert_eq!(snapshot.perm_version, 1101);

    let mut req = TxRequest::new("account.deposit", "key-001");
    req.amount_cents = 100_000;
    assert_eq!(banking.submit(&req).expect("提交"), "FAKE-key-001");

    // 二进制下载走的是另一个服务与另一个编解码
    let pack = banking.download_menu().expect("下载菜单包");
    assert_eq!(pack, b"FAKE-MTREE-PACK");

    // 证明请求真的到了后台（不是本地假装成功）
    let ops = bank.ops();
    assert!(ops.contains(&"probe_menu".to_string()));
    assert!(ops.contains(&"login".to_string()));
    assert!(ops.contains(&"submit_tx".to_string()));
    assert!(ops.contains(&"download".to_string()));
}

#[test]
fn business_rejection_surfaces_as_rejected_not_unavailable() {
    let bank = FakeBank::start(FakeBankConfig {
        reject_reason: Some("余额不足".into()),
        ..Default::default()
    });
    let banking = RoutedBanking::new(router_for(&bank));

    let mut req = TxRequest::new("account.deposit", "key-002");
    req.amount_cents = 1;
    let err = banking.submit(&req).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("余额不足"), "实际：{text}");
    // 业务拒绝映射为 Rejected（不触发主备切换）
    assert!(matches!(err, tellerkit_core::tx::HostError::Rejected(_)));
}

#[test]
fn backend_down_falls_back_to_secondary_endpoint() {
    // 主端点指向一个没人监听的端口，备用端点指向真后台
    let bank = FakeBank::start(FakeBankConfig::default());

    let dead = EndpointAddr::Tcp {
        host: "127.0.0.1".into(),
        port: 1, // 特权端口，普通用户无法监听
    };
    let alive = EndpointAddr::Tcp {
        host: bank.host(),
        port: bank.port(),
    };

    let router = Router::new(vec![
        Endpoint {
            service: "menu.core".into(),
            addr: dead,
            transport: Arc::new(TcpTransport::default()),
            codec: Arc::new(JsonCodec),
            priority: 0,
            timeout_ms: 500,
        },
        Endpoint {
            service: "menu.core".into(),
            addr: alive,
            transport: Arc::new(TcpTransport::default()),
            codec: Arc::new(JsonCodec),
            priority: 1,
            timeout_ms: 3000,
        },
    ]);

    let probe = router
        .call(&tellerkit_core::remote::Request {
            service: "menu.core".into(),
            op: "probe_menu".into(),
            body: tellerkit_core::remote::Body::Json(serde_json::json!({})),
        })
        .expect("备用端点应接管");

    assert_eq!(
        probe
            .as_json()
            .and_then(|v| v.get("latest"))
            .and_then(|v| v.as_u64()),
        Some(7)
    );
}

/// 契约 lifecycle.md 不变量 6：同键重复提交只入账一次，且返回同一流水号。
///
/// 这是客户端所有重试/重放策略的前提。用真 socket 证明它，而不是写在文档里。
#[test]
fn same_idempotency_key_is_charged_once() {
    let bank = FakeBank::start(FakeBankConfig::default());
    let banking = RoutedBanking::new(router_for(&bank)).with_idempotency(true);

    let mut req = TxRequest::new("account.deposit", "dup-key-1");
    req.amount_cents = 50_000;

    let first = banking.submit(&req).expect("首次提交");
    let second = banking.submit(&req).expect("重试");
    let third = banking.submit(&req).expect("再重试");

    assert_eq!(first, second, "重试必须返回同一流水号");
    assert_eq!(second, third);
    assert_eq!(*bank.config().charged.lock().unwrap(), 1, "只应入账一次");
    assert_eq!(
        bank.ops().iter().filter(|op| *op == "submit_tx").count(),
        3,
        "请求确实发了三次，是服务端去重而不是客户端缓存"
    );
}

/// 未声明幂等的主机：每次提交都入账。
///
/// 这个用例是"为什么客户端必须拦住离线挂起"的证据——
/// 不拦的话，断网期间的交易在恢复后重放就会重复扣账。
#[test]
fn non_idempotent_backend_charges_every_attempt() {
    let bank = FakeBank::start(FakeBankConfig {
        idempotent: false,
        ..Default::default()
    });
    let banking = RoutedBanking::new(router_for(&bank));

    assert!(!banking.supports_idempotency());
    let req = TxRequest::new("account.deposit", "dup-key-2");
    let _ = banking.submit(&req);
    let _ = banking.submit(&req);

    assert_eq!(
        *bank.config().charged.lock().unwrap(),
        2,
        "主机不去重时，两次提交就是两次扣账"
    );
}

//! 启动策略：决定"能不能起来"，而不是"起来以后怎么降级"。
//!
//! 对应 A-06 / P0-5。旧行为"配置有问题就回退内置 mock"在柜面意味着
//! 客户以为在办业务、实际什么都没发生。因此按构建类型分开：
//! - release：缺端点 / 配置非法 / 没签名 / 明文端点，一律拒绝进入业务界面，
//!   原因写进 [`StartupFatal`]，由 `boot` 命令转成前端可读的错误页；
//! - debug：允许回退演示远端，但必须带显式 note，界面据此标"演示模式"。
//!
//! 判断只看构建类型与配置内容——环境变量谁都能改，构建类型改不了。

use std::path::Path;
use std::sync::Arc;

use tellerkit_core::banking::RoutedBanking;

use crate::endpoint_config::{self, LoadPolicy};
use crate::ports::{MockRemote, RemoteChoice};

/// 一次启动解析的结果：可以用的远端 + 要给人看的说明。
pub struct RemoteSetup {
    choice: RemoteChoice,
    /// 启动说明。debug 构建回退演示远端时**必须**非空，界面用它标演示模式。
    note: Option<String>,
}

impl RemoteSetup {
    pub fn new(choice: RemoteChoice, note: Option<String>) -> Self {
        Self { choice, note }
    }

    pub fn choice(&self) -> &RemoteChoice {
        &self.choice
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_deref()
    }

    /// 装配层原样取走两部分，避免为了私有化而复制 `Arc` 与说明文本。
    pub fn into_parts(self) -> (RemoteChoice, Option<String>) {
        (self.choice, self.note)
    }
}

/// 启动被拒绝的原因。作为 `AppState` 字段，由唯一的加锁入口统一拦截。
#[derive(Debug, Clone, Default)]
pub struct StartupFatal(pub Option<String>);

impl StartupFatal {
    pub fn check(&self) -> Result<(), String> {
        match &self.0 {
            Some(reason) => Err(format!("启动被拒绝：{reason}")),
            None => Ok(()),
        }
    }
}

/// 三态开关：未设置 / 开 / 关。
///
/// 写了但认不出的值**不能**当成"关"——那正好是 fail-open 的典型形态。
fn env_flag(name: &str, raw: Option<&str>) -> Result<Option<bool>, String> {
    let Some(value) = raw.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    match value {
        "1" => Ok(Some(true)),
        "0" => Ok(Some(false)),
        _ if value.eq_ignore_ascii_case("true") => Ok(Some(true)),
        _ if value.eq_ignore_ascii_case("false") => Ok(Some(false)),
        other => Err(format!(
            "{name} 的值 `{other}` 无法识别：只接受 1 / 0 / true / false"
        )),
    }
}

/// 端点策略：release 一律强制签名与 TLS，且不给"关掉"的开关——
/// 这两项一旦可关，出问题时第一个被改的就是它们。
pub fn policy(
    release: bool,
    require_signed: Option<bool>,
    require_tls: Option<bool>,
) -> LoadPolicy {
    LoadPolicy::new(
        crate::config_sig::config_pubkey(),
        release || require_signed.unwrap_or(false),
        release || require_tls.unwrap_or(false),
    )
}

/// 从环境变量组装策略。非法取值直接报错，不静默取默认。
pub fn policy_from_env(release: bool) -> Result<LoadPolicy, String> {
    let signed = env_flag(
        "TK_REQUIRE_SIGNED_CONFIG",
        std::env::var("TK_REQUIRE_SIGNED_CONFIG").ok().as_deref(),
    )?;
    let tls = env_flag(
        "TK_REQUIRE_TLS",
        std::env::var("TK_REQUIRE_TLS").ok().as_deref(),
    )?;
    let policy = policy(release, signed, tls);
    if release && policy.require_signed() && policy.pubkey().is_none() {
        return Err("release 构建必须设置 TK_CONFIG_PUBKEY：拒绝加载未验签的端点".into());
    }
    Ok(policy)
}

/// 解析远端：要么拿到能用的真实路由，要么拿到"拒绝启动"的理由。
pub fn resolve_remote(
    path: &Path,
    release: bool,
    policy: &LoadPolicy,
) -> Result<RemoteSetup, String> {
    if !path.exists() {
        return if release {
            Err(format!(
                "未找到端点配置 {}：发布构建拒绝以演示远端启动，请下发并签名 endpoints.json",
                path.display()
            ))
        } else {
            Ok(demo_setup(format!(
                "未找到端点配置 {}，使用内置演示远端（仅调试构建）",
                path.display()
            )))
        };
    }

    match endpoint_config::load_router_with(path, policy) {
        Ok(loaded) => Ok(RemoteSetup::new(
            RemoteChoice::new(
                Arc::new({
                    let (router, idempotent_tx) = loaded.into_parts();
                    RoutedBanking::new(router).with_idempotency(idempotent_tx)
                }),
                None,
            ),
            Some(format!(
                "已按 {} 接入真实端点{}",
                path.display(),
                if policy.require_tls() {
                    ""
                } else {
                    "（未强制 TLS：仅调试构建允许）"
                }
            )),
        )),
        Err(e) => {
            if release {
                Err(format!("端点配置不可用：{e}"))
            } else {
                Ok(demo_setup(format!(
                    "端点配置不可用（{e}），使用内置演示远端"
                )))
            }
        }
    }
}

fn demo_setup(note: impl Into<String>) -> RemoteSetup {
    let mock = Arc::new(MockRemote::default());
    RemoteSetup::new(
        RemoteChoice::new(
            Arc::clone(&mock) as Arc<dyn tellerkit_core::banking::RemoteBanking>,
            Some(mock),
        ),
        Some(note.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAINTEXT: &str = r#"{ "services": { "menu.core": [
          { "transport": "http", "codec": "json", "url": "http://menu.internal/v1" } ] } }"#;

    fn write(dir: &Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("endpoints.json");
        std::fs::write(&path, body).expect("写测试配置");
        path
    }

    fn strict(release: bool) -> LoadPolicy {
        policy(release, None, None)
    }

    /// 构造测试策略：pubkey 为真给一把占位公钥，否则 None。
    fn lp(pubkey: bool, signed: bool, tls: bool) -> LoadPolicy {
        LoadPolicy::new(pubkey.then(|| "00".repeat(32)), signed, tls)
    }

    #[test]
    fn release_forces_signed_and_tls_even_if_asked_not_to() {
        let p = policy(true, Some(false), Some(false));
        assert!(p.require_signed(), "release 不允许关掉配置验签");
        assert!(p.require_tls(), "release 不允许关掉 TLS");
    }

    #[test]
    fn debug_defaults_to_loose_but_can_be_tightened() {
        let loose = policy(false, None, None);
        assert!(
            !loose.require_signed() && !loose.require_tls(),
            "调试构建不强制"
        );

        let tight = policy(false, Some(true), Some(true));
        assert!(
            tight.require_signed() && tight.require_tls(),
            "调试构建可显式收紧"
        );
    }

    #[test]
    fn unrecognised_flag_value_is_rejected_not_treated_as_off() {
        let err = env_flag("TK_REQUIRE_TLS", Some("maybe")).unwrap_err();
        assert!(err.contains("TK_REQUIRE_TLS"), "错误要指向变量名：{err}");
        assert!(err.contains("无法识别"), "要说清是取值问题：{err}");

        assert_eq!(env_flag("TK_REQUIRE_TLS", None).unwrap(), None);
        assert_eq!(env_flag("TK_REQUIRE_TLS", Some(" ")).unwrap(), None);
        assert_eq!(
            env_flag("TK_REQUIRE_TLS", Some("TRUE")).unwrap(),
            Some(true)
        );
        assert_eq!(env_flag("TK_REQUIRE_TLS", Some("0")).unwrap(), Some(false));
    }

    #[test]
    fn release_without_endpoint_file_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let err = resolve_remote(&path, true, &strict(true))
            .err()
            .expect("release 缺配置应拒绝启动");
        assert!(err.contains("发布构建"), "要说明是发布构建：{err}");
    }

    #[test]
    fn debug_without_endpoint_file_reports_demo_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.json");
        let setup = resolve_remote(&path, false, &strict(false)).unwrap();
        assert!(setup.choice().mock().is_some(), "调试构建回退演示远端");
        let note = setup.note().expect("回退必须留 note");
        assert!(note.contains("演示"), "note 要能让界面标出演示模式：{note}");
    }

    #[test]
    fn release_with_invalid_config_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "{ \"services\": {} }");
        let err = resolve_remote(&path, true, &lp(true, false, true))
            .err()
            .expect("release 配置非法应拒绝启动");
        assert!(err.contains("端点配置不可用"), "实际：{err}");
    }

    #[test]
    fn release_with_plaintext_endpoint_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), PLAINTEXT);
        let err = resolve_remote(&path, true, &lp(true, false, true))
            .err()
            .expect("release 明文端点应拒绝启动");
        assert!(err.contains("明文"), "要指出是明文端点：{err}");
    }

    #[test]
    fn release_without_signing_key_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), PLAINTEXT);
        let err = resolve_remote(&path, true, &lp(false, true, false))
            .err()
            .expect("release 缺签名公钥应拒绝启动");
        assert!(err.contains("TK_CONFIG_PUBKEY"), "要指出缺哪个配置：{err}");
    }

    #[test]
    fn debug_with_invalid_config_still_reports_why() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(dir.path(), "{ \"services\": {} }");
        let setup = resolve_remote(&path, false, &strict(false)).unwrap();
        assert!(setup.choice().mock().is_some());
        let note = setup.note().expect("降级必须留 note");
        assert!(
            note.contains("没有任何服务"),
            "note 要带原始原因，否则没人知道配置错在哪：{note}"
        );
    }

    #[test]
    fn valid_config_yields_real_route_without_mock() {
        let dir = tempfile::tempdir().unwrap();
        let path = write(
            dir.path(),
            r#"{ "services": { "menu.core": [
              { "transport": "http", "codec": "json", "url": "https://menu.internal/v1" } ] } }"#,
        );
        let setup = resolve_remote(&path, true, &lp(true, false, true)).unwrap();
        assert!(setup.choice().mock().is_none(), "真实端点不该留 mock 句柄");
        assert!(
            setup.note().expect("接入说明").contains("真实端点"),
            "要能看出接的是真实端点"
        );
    }

    #[test]
    fn startup_fatal_is_silent_only_when_absent() {
        assert!(StartupFatal::default().check().is_ok());
        let fatal = StartupFatal(Some("端点配置不可用".into()));
        let err = fatal.check().unwrap_err();
        assert!(err.contains("启动被拒绝"), "前端要看到明确拒绝：{err}");
        assert!(err.contains("端点配置不可用"), "要带原始原因：{err}");
    }
}

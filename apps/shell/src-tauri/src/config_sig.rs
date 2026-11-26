//! 配置签名：给"端点配置"加一条**完整性**保证。
//!
//! 为什么配置也要签名：端点配置决定了客户的交易数据发到哪个 IP。
//! 能改这个文件的人，就能把交易导到自己的服务器——这比改菜单包更危险，
//! 因为菜单包至少还有验签，而配置以前是明文的。
//!
//! 形态：`endpoints.json` + 同名 `endpoints.json.sig`（对**文件原始字节**的 Ed25519 签名，hex）。
//! 公钥由 `TK_CONFIG_PUBKEY` 注入（32 字节 hex）。密钥与菜单包公钥分开：
//! 两者轮换节奏不同，共用一把会让菜单包轮换时配置全部失效。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use tellerkit_core::pubkey_from_hex;

/// 校验失败的原因。文案要能直接给运维看。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigSigError {
    #[error("缺少配置公钥：请设置 TK_CONFIG_PUBKEY（32 字节 hex）")]
    MissingKey,
    #[error("配置公钥非法：{0}")]
    BadKey(String),
    #[error("缺少签名文件 {0}：配置未签名，拒绝加载")]
    MissingSignature(String),
    #[error("签名格式非法：{0}")]
    BadSignature(String),
    #[error("配置签名不匹配：文件已被修改或签名不属于这把公钥")]
    Mismatch,
}

/// 校验 `payload` 的分离式签名。
///
/// `payload` 必须是**文件原始字节**（不是解析后再序列化的结果）——
/// 否则字段顺序、空白、编码差异都会让合法签名失效。
pub fn verify_detached(
    payload: &[u8],
    signature_hex: &str,
    pubkey_hex: &str,
) -> Result<(), ConfigSigError> {
    let key: VerifyingKey =
        pubkey_from_hex(pubkey_hex).map_err(|e| ConfigSigError::BadKey(e.to_string()))?;
    let raw = hex::decode(signature_hex.trim())
        .map_err(|e| ConfigSigError::BadSignature(e.to_string()))?;
    let bytes: [u8; 64] = raw
        .as_slice()
        .try_into()
        .map_err(|_| ConfigSigError::BadSignature("签名必须是 64 字节".into()))?;
    let signature = Signature::from_bytes(&bytes);
    key.verify(payload, &signature)
        .map_err(|_| ConfigSigError::Mismatch)
}

/// 当前生效的配置公钥（来自环境变量）。
///
/// 返回 `None` 表示未配置。**未配置时不信任任何签名**，而不是退化成"跳过校验"。
pub fn config_pubkey() -> Option<String> {
    std::env::var("TK_CONFIG_PUBKEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 是否强制要求配置签名。
///
/// 生产环境必须为真。默认取 `TK_REQUIRE_SIGNED_CONFIG`，
/// 未设置时按"有公钥就必须签"处理——避免出现"配了公钥却忘了强制"的漏洞。
pub fn require_signed() -> bool {
    require_signed_for(
        config_pubkey().as_deref(),
        std::env::var("TK_REQUIRE_SIGNED_CONFIG").ok().as_deref(),
    )
}

/// 纯函数版本，便于测试（不读环境变量）。
///
/// 语义：显式开关优先；没设开关时"配了公钥就必须签"——
/// 避免"配了公钥却忘了强制"这种半吊子配置。
pub fn require_signed_for(pubkey: Option<&str>, flag: Option<&str>) -> bool {
    match flag {
        Some(v) => v == "1" || v.eq_ignore_ascii_case("true"),
        None => pubkey.map(|k| !k.trim().is_empty()).unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use tellerkit_core::pubkey_to_hex;

    fn keypair() -> (SigningKey, String) {
        let key = SigningKey::generate(&mut OsRng);
        (key.clone(), pubkey_to_hex(&key.verifying_key()))
    }

    fn sign(key: &SigningKey, payload: &[u8]) -> String {
        hex::encode(key.sign(payload).to_bytes())
    }

    #[test]
    fn valid_signature_passes() {
        let (key, pubkey) = keypair();
        let payload = br#"{"services":{}}"#;
        assert_eq!(
            verify_detached(payload, &sign(&key, payload), &pubkey),
            Ok(())
        );
    }

    #[test]
    fn tampered_payload_fails() {
        let (key, pubkey) = keypair();
        let original = br#"{"services":{"acct.core":[]}}"#;
        let sig = sign(&key, original);
        let tampered = br#"{"services":{"acct.core":[{"host":"evil"}]}}"#;
        assert_eq!(
            verify_detached(tampered, &sig, &pubkey),
            Err(ConfigSigError::Mismatch),
            "改了配置必须验签失败"
        );
    }

    #[test]
    fn signature_from_another_key_fails() {
        let (_, pubkey_a) = keypair();
        let (key_b, _) = keypair();
        let payload = b"{}";
        assert_eq!(
            verify_detached(payload, &sign(&key_b, payload), &pubkey_a),
            Err(ConfigSigError::Mismatch)
        );
    }

    #[test]
    fn malformed_signature_is_reported_not_ignored() {
        let (_, pubkey) = keypair();
        assert!(matches!(
            verify_detached(b"{}", "zz", &pubkey),
            Err(ConfigSigError::BadSignature(_))
        ));
        assert!(matches!(
            verify_detached(b"{}", "ab", &pubkey),
            Err(ConfigSigError::BadSignature(_))
        ));
    }

    #[test]
    fn malformed_pubkey_is_reported() {
        assert!(matches!(
            verify_detached(b"{}", &"00".repeat(64), "not-hex"),
            Err(ConfigSigError::BadKey(_))
        ));
    }

    #[test]
    fn signed_config_is_required_when_pubkey_present() {
        assert!(require_signed_for(Some("ab"), None), "配了公钥就必须签");
        assert!(!require_signed_for(None, None), "没公钥时不强制");
        assert!(!require_signed_for(Some("  "), None), "空公钥视为未配置");
        assert!(require_signed_for(None, Some("1")), "显式开关优先");
        assert!(require_signed_for(None, Some("TRUE")));
        assert!(!require_signed_for(Some("ab"), Some("0")), "显式关掉可覆盖");
    }
}

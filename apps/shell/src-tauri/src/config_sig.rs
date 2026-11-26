//! 配置签名：给"端点配置"加一条**完整性**保证。
//!
//! 为什么配置也要签名：端点配置决定了客户的交易数据发到哪个 IP。
//! 能改这个文件的人，就能把交易导到自己的服务器——这比改菜单包更危险，
//! 因为菜单包至少还有验签，而配置以前是明文的。
//!
//! 形态：`endpoints.json` + 同名 `endpoints.json.sig`（对**文件原始字节**的 Ed25519 签名，hex）。
//! 配置验签公钥与菜单包公钥是两把密钥，轮换节奏不同，不能共用：
//! - 生产：`build.rs` 从 `TK_CONFIG_PUBKEY_HEX` 注入，release 缺失即构建失败；
//! - 调试：可用运行期 `TK_CONFIG_PUBKEY` 覆盖，便于本地换钥匙；release 忽略运行期值，
//!   否则能改进程环境的人就能整体替换信任根。
//! 启动摘要并列打印两把公钥的指纹，便于现场核对部署的是哪一把。

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

use tellerkit_core::pubkey_from_hex;

/// 校验失败的原因。文案要能直接给运维看。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigSigError {
    #[error("缺少配置公钥：release 需在构建时设置 TK_CONFIG_PUBKEY_HEX（debug 可用 TK_CONFIG_PUBKEY 覆盖）")]
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

/// 构建期注入的配置验签公钥（32 字节 hex）。
const BUILTIN_CONFIG_PUBKEY: Option<&str> = option_env!("TK_CONFIG_PUBKEY_HEX");

fn clean_hex(value: Option<&str>) -> Option<String> {
    value
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// 选择生效的配置公钥。纯函数版本便于测试构建期/运行期两条来源的优先级。
///
/// 只有 debug 构建才认运行期覆盖；release 即使进程环境被改，也只用内置公钥。
fn select_config_pubkey(
    injected: Option<&str>,
    runtime_override: Option<&str>,
    debug: bool,
) -> Option<String> {
    if debug {
        if let Some(key) = clean_hex(runtime_override) {
            return Some(key);
        }
    }
    clean_hex(injected)
}

/// 当前生效的配置公钥。
///
/// 返回 `None` 表示未配置。**未配置时不信任任何签名**，而不是退化成"跳过校验"。
pub fn config_pubkey() -> Option<String> {
    let runtime = std::env::var("TK_CONFIG_PUBKEY").ok();
    select_config_pubkey(
        BUILTIN_CONFIG_PUBKEY,
        runtime.as_deref(),
        cfg!(debug_assertions),
    )
}

/// 公钥指纹：`hex(sha256(公钥 hex 文本))` 的前 16 位。
///
/// 只用于启动摘要人工核对，不参与任何验签判断。
pub fn pubkey_fingerprint(pubkey_hex: &str) -> String {
    let digest = sha256(pubkey_hex.trim().as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// 启动摘要：菜单与配置两把公钥并列，运维据此确认部署的信任根。
pub fn startup_digest(config_pubkey: Option<&str>) -> String {
    let menu_hex = clean_hex(option_env!("TK_MENU_PUBKEY_HEX"))
        .unwrap_or_else(tellerkit_core::mock::demo_pubkey_hex);
    let menu = pubkey_fingerprint(&menu_hex);
    let config = config_pubkey
        .map(pubkey_fingerprint)
        .unwrap_or_else(|| "未配置".to_string());
    format!("启动摘要：菜单公钥指纹={menu}，配置公钥指纹={config}")
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

/// 指纹用的 SHA-256。shell crate 没有直接依赖 `sha2`，而指纹只是给人看的摘要，
/// 不参与验签；这里用标准实现，测试用 NIST 向量钉死，避免引入新依赖。
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];
    let bit_len = (input.len() as u64).wrapping_mul(8);
    let mut data = input.to_vec();
    data.push(0x80);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in data.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in w[..16].iter_mut().enumerate() {
            *word = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().expect("4 bytes"));
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(choose)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(majority);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (slot, value) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
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
    fn sha256_matches_nist_vectors() {
        assert_eq!(
            hex::encode(sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex::encode(sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn fingerprint_is_first_16_hex_digits() {
        assert_eq!(pubkey_fingerprint("abc"), "ba7816bf8f01cfea");
        assert_eq!(
            pubkey_fingerprint(" abc "),
            "ba7816bf8f01cfea",
            "两侧空白不影响"
        );
    }

    #[test]
    fn only_debug_build_accepts_runtime_pubkey_override() {
        let builtin = "ab".repeat(32);
        let override_key = "cd".repeat(32);
        assert_eq!(
            select_config_pubkey(Some(&builtin), None, true).as_deref(),
            Some(builtin.as_str())
        );
        assert_eq!(
            select_config_pubkey(Some(&builtin), Some(&override_key), true).as_deref(),
            Some(override_key.as_str()),
            "debug 运行期覆盖优先"
        );
        assert_eq!(
            select_config_pubkey(Some(&builtin), Some(&override_key), false).as_deref(),
            Some(builtin.as_str()),
            "release 必须忽略运行期覆盖"
        );
        assert_eq!(
            select_config_pubkey(None, Some("  "), true),
            None,
            "空白不是公钥"
        );
    }

    #[test]
    fn startup_digest_lists_both_keys() {
        let line = startup_digest(Some(&"ab".repeat(32)));
        assert!(line.contains("菜单公钥指纹="), "实际：{line}");
        assert!(line.contains("配置公钥指纹="), "实际：{line}");
        assert!(line.contains(&pubkey_fingerprint(&"ab".repeat(32))));
        assert!(startup_digest(None).contains("配置公钥指纹=未配置"));
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

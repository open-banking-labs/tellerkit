//! 账本字段加密：把交易要素（账号、摘要）从明文变成密文。
//!
//! ## 能防什么
//!
//! - 拷走 `tx.db` 单文件（技术支持、备份系统、期末审计存档）读不到账号；
//! - 拿到旧备份但没拿到密钥文件时，历史交易仍是密文。
//!
//! ## 不能防什么（必须如实告知）
//!
//! 密钥文件就在旁边（`ledger.key`）。**能执行代码的本地攻击者可以两样一起拿走。**
//! 真正的保护要等 OS 钥匙串（Windows DPAPI / macOS Keychain / Linux Secret Service），
//! 那是 P4 的活，见 `docs/security-boundary.md`。
//!
//! 所以这里的目标是**提高单文件泄露的成本**，不是宣称"账本安全了"。
//!
//! ## 格式
//!
//! `v1:<hex nonce>:<hex ciphertext+tag>`。没有 `v1:` 前缀的数据**拒绝读取**：
//! 旧库在 `SqliteJournal::open` 时一次性迁移（读后重写），迁移后仍无前缀即视为降级攻击。
//! 这个前缀不是认证本身，而是"这批数据是否已经迁移"的显式开关；真正的完整性由 GCM tag 保证。

use std::path::{Path, PathBuf};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};

const PREFIX: &str = "v1:";

/// 账本加密器。密钥来自 `<appdata>/ledger.key`（首次启动生成）。
pub struct LedgerCipher {
    key: LessSafeKey,
    path: PathBuf,
}

impl std::fmt::Debug for LedgerCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 绝不打印密钥
        f.debug_struct("LedgerCipher")
            .field("key_path", &self.path)
            .finish()
    }
}

impl LedgerCipher {
    /// 读取密钥；没有就生成一个（权限 0600，尽力而为）。
    pub fn open_or_create(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let raw = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let fresh = Self::generate_key_bytes()?;
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| format!("创建账本目录失败：{e}"))?;
                }
                std::fs::write(&path, &fresh).map_err(|e| format!("写入账本密钥失败：{e}"))?;
                Self::restrict_permissions(&path);
                fresh
            }
            Err(e) => return Err(format!("读取账本密钥失败（{}）：{e}", path.display())),
        };

        if raw.len() != 32 {
            return Err(format!(
                "账本密钥长度非法（{} 字节，应为 32）：{}",
                raw.len(),
                path.display()
            ));
        }
        Self::from_bytes(&raw, path)
    }

    /// 测试与内存模式用：不落盘。
    pub fn ephemeral() -> Result<Self, String> {
        let raw = Self::generate_key_bytes()?;
        Self::from_bytes(&raw, PathBuf::from("<ephemeral>"))
    }

    fn generate_key_bytes() -> Result<Vec<u8>, String> {
        let mut buf = vec![0u8; 32];
        SystemRandom::new()
            .fill(&mut buf)
            .map_err(|_| "系统随机数不可用".to_string())?;
        Ok(buf)
    }

    fn from_bytes(raw: &[u8], path: PathBuf) -> Result<Self, String> {
        let unbound = UnboundKey::new(&AES_256_GCM, raw)
            .map_err(|_| "账本密钥非法（AES-256 要求 32 字节）".to_string())?;
        Ok(Self {
            key: LessSafeKey::new(unbound),
            path,
        })
    }

    #[cfg(unix)]
    fn restrict_permissions(path: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }

    #[cfg(not(unix))]
    fn restrict_permissions(_path: &Path) {
        // Windows 上没有等价的简单 chmod；ACL 设置属于 P4（OS 钥匙串方案的一部分）
    }

    /// 加密。返回可直接入库的字符串。
    pub fn seal(&self, plaintext: &str) -> Result<String, String> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| "系统随机数不可用".to_string())?;

        let mut in_out = plaintext.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::empty(),
                &mut in_out,
            )
            .map_err(|_| "账本加密失败".to_string())?;

        Ok(format!(
            "{PREFIX}{}:{}",
            hex::encode(nonce_bytes),
            hex::encode(in_out)
        ))
    }

    /// 解密。没有 `v1:` 前缀一律拒绝：静默按明文返回等于给降级改写开后门。
    pub fn open(&self, stored: &str) -> Result<String, String> {
        let Some(rest) = stored.strip_prefix(PREFIX) else {
            return Err("账本数据缺少 v1: 前缀，拒绝读取（疑似密文被剥离）".to_string());
        };
        let (nonce_hex, body_hex) = rest
            .split_once(':')
            .ok_or_else(|| "账本密文格式非法（缺少分隔符）".to_string())?;

        let nonce_raw = hex::decode(nonce_hex).map_err(|e| format!("nonce 非法：{e}"))?;
        let nonce_bytes: [u8; NONCE_LEN] = nonce_raw
            .as_slice()
            .try_into()
            .map_err(|_| format!("nonce 长度非法：{}", nonce_raw.len()))?;
        let mut body = hex::decode(body_hex).map_err(|e| format!("密文非法：{e}"))?;

        let plain = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::empty(),
                &mut body,
            )
            .map_err(|_| "账本解密失败：密钥不匹配或数据被篡改".to_string())?;

        String::from_utf8(plain.to_vec()).map_err(|e| format!("账本明文不是 UTF-8：{e}"))
    }

    /// 这条数据是否已经按 v1 密文格式落盘。
    pub fn is_sealed(stored: &str) -> bool {
        stored.starts_with(PREFIX)
    }

    /// 仅供旧库一次性迁移：把无前缀的明文就地重新加密。
    ///
    /// 已有前缀时报错——避免把密文当明文再包一层，也避免迁移逻辑被当成通用读取口。
    pub fn migrate_legacy(&self, plaintext: &str) -> Result<String, String> {
        if Self::is_sealed(plaintext) {
            return Err("数据已有 v1: 前缀，不能按旧明文重新加密".to_string());
        }
        self.seal(plaintext)
    }

    pub fn key_path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_hides_plaintext() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        let secret = r#"{"account":"6222021234567890123"}"#;
        let stored = cipher.seal(secret).unwrap();

        assert!(stored.starts_with("v1:"));
        assert!(!stored.contains("6222"), "密文里不能出现账号明文");
        assert_eq!(cipher.open(&stored).unwrap(), secret);
    }

    #[test]
    fn same_plaintext_encrypts_differently() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        let a = cipher.seal("相同内容").unwrap();
        let b = cipher.seal("相同内容").unwrap();
        assert_ne!(a, b, "每次都要用新 nonce，否则能看出两笔交易相同");
    }

    #[test]
    fn tampered_ciphertext_is_rejected() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        let stored = cipher.seal("amount=100").unwrap();
        // 翻转密文最后一个字节（属于 GCM tag）
        let mut chars: Vec<char> = stored.chars().collect();
        let last = chars.len() - 1;
        chars[last] = if chars[last] == '0' { '1' } else { '0' };
        let tampered: String = chars.into_iter().collect();

        assert!(cipher.open(&tampered).is_err(), "篡改必须被 GCM 发现");
    }

    #[test]
    fn wrong_key_cannot_decrypt() {
        let a = LedgerCipher::ephemeral().unwrap();
        let b = LedgerCipher::ephemeral().unwrap();
        let stored = a.seal("secret").unwrap();
        assert!(b.open(&stored).is_err());
    }

    #[test]
    fn legacy_plaintext_is_rejected_by_open() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        // 旧数据只能走 journal 启动时的一次性迁移，不能成为通用读取路径。
        assert!(cipher.open(r#"{"account":"123"}"#).is_err());
    }

    #[test]
    fn stripped_prefix_is_rejected() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        let stored = cipher.seal(r#"{"amount":100}"#).unwrap();
        let stripped = stored.strip_prefix("v1:").unwrap();
        assert!(
            cipher.open(stripped).is_err(),
            "剥离前缀后即便内容未变也不能原样返回，否则改写不会被发现"
        );
    }

    #[test]
    fn migration_reseals_legacy_plaintext() {
        let cipher = LedgerCipher::ephemeral().unwrap();
        let legacy = r#"{"account":"123"}"#;
        let sealed = cipher.migrate_legacy(legacy).unwrap();
        assert!(sealed.starts_with("v1:"));
        assert_eq!(cipher.open(&sealed).unwrap(), legacy);
        assert!(cipher.migrate_legacy(&sealed).is_err());
    }

    #[test]
    fn key_file_persists_and_reloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.key");
        let first = LedgerCipher::open_or_create(&path).unwrap();
        let stored = first.seal("secret").unwrap();

        let second = LedgerCipher::open_or_create(&path).unwrap();
        assert_eq!(second.open(&stored).unwrap(), "secret", "密钥应可重复加载");
    }

    #[test]
    fn malformed_key_file_is_reported_not_silently_regenerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger.key");
        std::fs::write(&path, b"too-short").unwrap();
        let err = LedgerCipher::open_or_create(&path).unwrap_err();
        assert!(err.contains("长度非法"), "实际：{err}");
    }
}

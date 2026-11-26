//! `.mtree` 菜单包：二进制序列化 + Ed25519 签名。
//!
//! 文件布局（PLAN.md §5.4）：
//!
//! ```text
//! [0..8)    magic  "MTKMTREE"
//! [8..10)   format_version  u16 LE
//! [10..14)  payload_len     u32 LE
//! [14..22)  menu_version    u64 LE
//! [22..24)  shell_compat_min u16 LE
//! [24..26)  shell_compat_max u16 LE
//! [26..)    payload: MessagePack(MenuTree)
//! [末 64B]  Ed25519 签名 over sha256(header || payload)
//! ```
//!
//! 安全边界（必须写进任何对外表述）：签名只能"检测篡改"和"阻止非授权方下发
//! 菜单包"，**不能**防住本机管理员。真正的权限边界永远在服务端接口。

use crate::menu::model::{MenuTree, TreeError};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"MTKMTREE";
pub const FORMAT_VERSION: u16 = 1;
pub const HEADER_LEN: usize = 26;
pub const SIGNATURE_LEN: usize = 64;

/// 包解析 / 验签错误。任何一种都必须导致"拒绝使用该文件"。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackError {
    #[error("文件长度不足，可能被截断")]
    TooShort,
    #[error("magic 不匹配，不是 .mtree 文件")]
    BadMagic,
    #[error("不支持的格式版本: {0}")]
    UnsupportedFormatVersion(u16),
    #[error("payload 长度与头部声明不一致")]
    PayloadLenMismatch,
    #[error("签名校验失败：文件已损坏或被篡改")]
    SignatureInvalid,
    #[error("公钥格式非法: {0}")]
    BadPublicKey(String),
    #[error("payload 反序列化失败: {0}")]
    DecodeFailed(String),
    #[error("菜单树结构非法: {0}")]
    InvalidTree(#[from] TreeError),
    #[error("签名失败: {0}")]
    SignFailed(String),
}

/// 已验签通过的菜单包。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuPack {
    format_version: u16,
    menu_version: u64,
    shell_compat_min: u16,
    shell_compat_max: u16,
    payload: Vec<u8>,
}

impl MenuPack {
    pub fn format_version(&self) -> u16 {
        self.format_version
    }

    pub fn menu_version(&self) -> u64 {
        self.menu_version
    }

    pub fn shell_compat_min(&self) -> u16 {
        self.shell_compat_min
    }

    pub fn shell_compat_max(&self) -> u16 {
        self.shell_compat_max
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// 把菜单树打包成带签名的 `.mtree` 字节流。
    ///
    /// 打包前先 `validate()`：结构非法的包绝不允许落到客户端。
    pub fn encode(
        tree: &MenuTree,
        shell_compat_min: u16,
        shell_compat_max: u16,
        signing_key: &SigningKey,
    ) -> Result<Vec<u8>, PackError> {
        tree.validate()?;
        // 用位置式序列化（结构体 -> 数组），不写字段名：
        // 体积更小，也不会把 schema 字段名暴露成明文。
        let payload = rmp_serde::to_vec(tree)
            .map_err(|e| PackError::DecodeFailed(format!("序列化失败: {e}")))?;
        let payload_len: u32 = payload
            .len()
            .try_into()
            .map_err(|_| PackError::DecodeFailed("payload 过大".into()))?;

        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
        header.extend_from_slice(&payload_len.to_le_bytes());
        header.extend_from_slice(&tree.menu_version().to_le_bytes());
        header.extend_from_slice(&shell_compat_min.to_le_bytes());
        header.extend_from_slice(&shell_compat_max.to_le_bytes());
        debug_assert_eq!(header.len(), HEADER_LEN);

        let digest = Sha256::new()
            .chain_update(&header)
            .chain_update(&payload)
            .finalize();
        let signature = signing_key.sign(digest.as_slice());

        let mut out = Vec::with_capacity(HEADER_LEN + payload.len() + SIGNATURE_LEN);
        out.extend_from_slice(&header);
        out.extend_from_slice(&payload);
        out.extend_from_slice(&signature.to_bytes());
        Ok(out)
    }

    /// 验签并拆包。签名覆盖"头部 + payload"，因此改版本号同样会被发现。
    pub fn verify(bytes: &[u8], verifying_key: &VerifyingKey) -> Result<MenuPack, PackError> {
        let pack = Self::parse(bytes)?;
        let header = &bytes[..HEADER_LEN];
        let payload_end = HEADER_LEN + pack.payload.len();
        let digest = Sha256::new()
            .chain_update(header)
            .chain_update(&pack.payload)
            .finalize();
        let sig_bytes: [u8; SIGNATURE_LEN] = bytes[payload_end..payload_end + SIGNATURE_LEN]
            .try_into()
            .map_err(|_| PackError::TooShort)?;
        let signature = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify(digest.as_slice(), &signature)
            .map_err(|_| PackError::SignatureInvalid)?;
        Ok(pack)
    }

    /// 只解析结构、不验签。**仅**用于诊断工具与版本探测。
    pub fn parse(bytes: &[u8]) -> Result<MenuPack, PackError> {
        if bytes.len() < HEADER_LEN + SIGNATURE_LEN {
            return Err(PackError::TooShort);
        }
        if &bytes[..8] != MAGIC {
            return Err(PackError::BadMagic);
        }
        let format_version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if format_version != FORMAT_VERSION {
            return Err(PackError::UnsupportedFormatVersion(format_version));
        }
        let payload_len = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
        let menu_version =
            u64::from_le_bytes(bytes[14..22].try_into().map_err(|_| PackError::TooShort)?);
        let shell_compat_min = u16::from_le_bytes([bytes[22], bytes[23]]);
        let shell_compat_max = u16::from_le_bytes([bytes[24], bytes[25]]);

        let expected = HEADER_LEN
            .checked_add(payload_len)
            .and_then(|n| n.checked_add(SIGNATURE_LEN))
            .ok_or(PackError::TooShort)?;
        if bytes.len() < expected {
            return Err(PackError::TooShort);
        }
        if bytes.len() > expected {
            return Err(PackError::PayloadLenMismatch);
        }
        Ok(MenuPack {
            format_version,
            menu_version,
            shell_compat_min,
            shell_compat_max,
            payload: bytes[HEADER_LEN..HEADER_LEN + payload_len].to_vec(),
        })
    }

    /// 反序列化并再次校验树结构（纵深防御：签名合法 ≠ 结构合法）。
    pub fn decode(&self) -> Result<MenuTree, PackError> {
        let tree: MenuTree = rmp_serde::from_slice(&self.payload)
            .map_err(|e| PackError::DecodeFailed(e.to_string()))?;
        tree.validate()?;
        Ok(tree)
    }
}

/// 一步到位：验签 → 解码 → 结构校验。
pub fn open(bytes: &[u8], verifying_key: &VerifyingKey) -> Result<MenuTree, PackError> {
    MenuPack::verify(bytes, verifying_key)?.decode()
}

/// 不验签地读出菜单版本号，供"文件列表/回滚选择"使用。
pub fn peek_version(bytes: &[u8]) -> Option<u64> {
    MenuPack::parse(bytes).ok().map(|p| p.menu_version())
}

/// 公钥 hex 编解码。
pub fn pubkey_to_hex(key: &VerifyingKey) -> String {
    hex::encode(key.to_bytes())
}

/// 从 hex 解析公钥（客户端内置的就是这串字符）。
pub fn pubkey_from_hex(s: &str) -> Result<VerifyingKey, PackError> {
    let raw = hex::decode(s.trim()).map_err(|e| PackError::BadPublicKey(e.to_string()))?;
    let arr: [u8; 32] = raw
        .as_slice()
        .try_into()
        .map_err(|_| PackError::BadPublicKey("长度必须为 32 字节".into()))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| PackError::BadPublicKey(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::model::MenuNode;
    use rand::rngs::OsRng;

    fn tree() -> MenuTree {
        MenuTree::new(
            7,
            vec![
                MenuNode::group("deposit", None, "存款", 1),
                MenuNode::page("deposit.demand", Some("deposit"), "活期存款", 1),
            ],
        )
    }

    fn key() -> SigningKey {
        SigningKey::generate(&mut OsRng)
    }

    #[test]
    fn encode_verify_decode_roundtrip() {
        let sk = key();
        let bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        let opened = open(&bytes, &sk.verifying_key()).unwrap();
        assert_eq!(opened, tree());
        assert_eq!(peek_version(&bytes), Some(7));
    }

    #[test]
    fn header_layout_matches_spec() {
        let sk = key();
        let bytes = MenuPack::encode(&tree(), 2, 5, &sk).unwrap();
        assert_eq!(&bytes[..8], MAGIC);
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), FORMAT_VERSION);
        assert_eq!(u64::from_le_bytes(bytes[14..22].try_into().unwrap()), 7);
        assert_eq!(u16::from_le_bytes([bytes[22], bytes[23]]), 2);
        assert_eq!(u16::from_le_bytes([bytes[24], bytes[25]]), 5);
    }

    #[test]
    fn flipping_one_payload_byte_is_detected() {
        let sk = key();
        let mut bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        let idx = HEADER_LEN + 5; // payload 中间
        bytes[idx] ^= 0x01;
        assert_eq!(
            open(&bytes, &sk.verifying_key()).unwrap_err(),
            PackError::SignatureInvalid
        );
    }

    #[test]
    fn flipping_version_byte_is_detected() {
        let sk = key();
        let mut bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        bytes[14] ^= 0x01; // menu_version 最低字节
        assert_eq!(
            MenuPack::verify(&bytes, &sk.verifying_key()).unwrap_err(),
            PackError::SignatureInvalid
        );
    }

    #[test]
    fn wrong_key_is_rejected() {
        let sk = key();
        let other = key();
        let bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        assert_eq!(
            MenuPack::verify(&bytes, &other.verifying_key()).unwrap_err(),
            PackError::SignatureInvalid
        );
    }

    #[test]
    fn truncated_file_is_rejected() {
        let sk = key();
        let bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        let short = &bytes[..bytes.len() - 1];
        assert_eq!(
            MenuPack::verify(short, &sk.verifying_key()).unwrap_err(),
            PackError::TooShort
        );
    }

    #[test]
    fn bad_magic_is_rejected() {
        let sk = key();
        let mut bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        bytes[0] = b'X';
        assert_eq!(
            MenuPack::verify(&bytes, &sk.verifying_key()).unwrap_err(),
            PackError::BadMagic
        );
    }

    #[test]
    fn invalid_tree_cannot_be_packed() {
        let sk = key();
        let bad = MenuTree::new(1, vec![MenuNode::page("x", Some("ghost"), "X", 1)]);
        assert!(matches!(
            MenuPack::encode(&bad, 1, 99, &sk),
            Err(PackError::InvalidTree(TreeError::MissingParent { .. }))
        ));
    }

    #[test]
    fn pubkey_hex_roundtrip() {
        let sk = key();
        let vk = sk.verifying_key();
        let hexs = pubkey_to_hex(&vk);
        assert_eq!(pubkey_from_hex(&hexs).unwrap(), vk);
        assert!(matches!(
            pubkey_from_hex("zz"),
            Err(PackError::BadPublicKey(_))
        ));
    }

    #[test]
    fn payload_is_binary_not_json() {
        // 注意：MessagePack 不是加密。结构是二进制的、没有编辑器能直接改，
        // 但字符串字段（标题）仍以 UTF-8 原样存在。要保密必须再加一层 AES-GCM，
        // 当前阶段不做——菜单标题本身不属于机密，机密在客户数据。
        let sk = key();
        let bytes = MenuPack::encode(&tree(), 1, 99, &sk).unwrap();
        let payload = &bytes[HEADER_LEN..];
        assert_ne!(payload[0], b'{', "payload 不应以 JSON 对象开始");
        let text = String::from_utf8_lossy(payload);
        assert!(!text.contains("\"nodes\""), "payload 不应是 JSON 文本");
        assert!(!text.contains("parent_code"), "不应出现字段名明文");
    }
}

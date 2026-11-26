//! 权限快照：登录响应带下来的"当前用户可见性"。
//!
//! 快照体积很小（几百个 code），登录时一次性拿到，常驻内存 HashSet，
//! 菜单过滤与按钮判断都是 O(1) 查表。

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use ts_rs::TS;

/// 登录后由服务端下发的权限快照。
///
/// 注意：JWT 里**只**放 `userId` + `permVersion`，权限明细走这份快照，
/// 因为 Token 签发后无法撤回，而权限变更必须能即时生效。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermSnapshot {
    user_id: String,
    /// 柜员号，用于水印与审计。
    teller_no: String,
    /// 网点号，用于水印与审计。
    outlet_code: String,
    /// 权限版本号。服务端权限变更时递增，客户端比对后决定是否重拉。
    #[ts(type = "number")]
    perm_version: u64,
    /// 可见的菜单节点 code（通常只放叶子/可导航节点）。
    menu_codes: Vec<String>,
    /// 可见的按钮级权限码。
    action_codes: Vec<String>,
    /// 该用户的自定义快捷键：menuCode -> 组合键。
    #[serde(default)]
    shortcuts: BTreeMap<String, String>,
    /// 展示用角色名（审计与演示需要）。
    #[serde(default)]
    role_label: String,
}

impl PermSnapshot {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        user_id: &str,
        teller_no: &str,
        outlet_code: &str,
        perm_version: u64,
        menu_codes: Vec<String>,
        action_codes: Vec<String>,
        shortcuts: BTreeMap<String, String>,
        role_label: &str,
    ) -> Self {
        Self {
            user_id: user_id.to_string(),
            teller_no: teller_no.to_string(),
            outlet_code: outlet_code.to_string(),
            perm_version,
            menu_codes,
            action_codes,
            shortcuts,
            role_label: role_label.to_string(),
        }
    }

    pub fn user_id(&self) -> &str {
        &self.user_id
    }

    pub fn teller_no(&self) -> &str {
        &self.teller_no
    }

    pub fn outlet_code(&self) -> &str {
        &self.outlet_code
    }

    pub fn perm_version(&self) -> u64 {
        self.perm_version
    }

    pub fn menu_codes(&self) -> &[String] {
        &self.menu_codes
    }

    pub fn action_codes(&self) -> &[String] {
        &self.action_codes
    }

    pub fn shortcuts(&self) -> &BTreeMap<String, String> {
        &self.shortcuts
    }

    pub fn role_label(&self) -> &str {
        &self.role_label
    }

    /// 判断菜单节点是否被授予。
    pub fn allows_menu(&self, code: &str) -> bool {
        self.menu_codes.iter().any(|c| c == code)
    }

    /// 判断按钮级权限是否被授予。
    pub fn allows_action(&self, code: &str) -> bool {
        self.action_codes.iter().any(|c| c == code)
    }

    /// 转为 HashSet，供高频过滤路径使用。
    pub fn menu_set(&self) -> HashSet<&str> {
        self.menu_codes.iter().map(|s| s.as_str()).collect()
    }

    /// 转为 HashSet，供高频过滤路径使用。
    pub fn action_set(&self) -> HashSet<&str> {
        self.action_codes.iter().map(|s| s.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PermSnapshot {
        PermSnapshot::new(
            "u1",
            "T0231",
            "01023",
            7,
            vec!["a".into(), "a.b".into()],
            vec!["action.print".into()],
            BTreeMap::new(),
            "柜员",
        )
    }

    #[test]
    fn allows_only_granted_codes() {
        let p = sample();
        assert!(p.allows_menu("a.b"));
        assert!(!p.allows_menu("a.c"));
        assert!(p.allows_action("action.print"));
        assert!(!p.allows_action("action.reversal"));
    }

    #[test]
    fn snapshot_survives_json_roundtrip() {
        let p = sample();
        let json = serde_json::to_string(&p).unwrap();
        let back: PermSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }
}

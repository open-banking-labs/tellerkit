//! P0 阶段的 mock 宿主：模拟服务端的菜单版本探测、菜单包下发、登录鉴权。
//!
//! 全部换成真实网络调用时，只替换本模块，上层 `MenuStore` 与前端契约不变。
//!
//! ⚠️ 这里的签名私钥是**公开的演示种子**，只能用于 demo。生产环境私钥必须
//! 只存在于服务端构建环节，客户端只内置公钥。

use crate::model::{MenuNode, MenuTree, NodeStatus};
use crate::pack::MenuPack;
use crate::perm::PermSnapshot;
use ed25519_dalek::{SigningKey, VerifyingKey};
use std::collections::BTreeMap;

/// 演示用固定种子（生产禁用）。
const DEMO_SEED: [u8; 32] = [
    0x54, 0x4b, 0x2d, 0x44, 0x45, 0x4d, 0x4f, 0x2d, 0x4f, 0x4e, 0x4c, 0x59, 0x2d, 0x4e, 0x4f, 0x54,
    0x2d, 0x50, 0x52, 0x4f, 0x44, 0x55, 0x43, 0x54, 0x49, 0x4f, 0x4e, 0x2d, 0x30, 0x30, 0x30, 0x31,
];

/// 主壳兼容的格式版本范围。
pub const SHELL_COMPAT_MIN: u16 = 1;
pub const SHELL_COMPAT_MAX: u16 = 1;

pub fn demo_signing_key() -> SigningKey {
    SigningKey::from_bytes(&DEMO_SEED)
}

pub fn demo_verifying_key() -> VerifyingKey {
    demo_signing_key().verifying_key()
}

pub fn demo_pubkey_hex() -> String {
    crate::pack::pubkey_to_hex(&demo_verifying_key())
}

/// 演示菜单树：覆盖典型柜面分组、叶子与按钮级权限。
pub fn demo_tree(menu_version: u64) -> MenuTree {
    fn page(code: &str, parent: &str, title: &str, sort: i32, actions: &[&str]) -> MenuNode {
        let mut node = MenuNode::page(code, Some(parent), title, sort);
        node.actions = actions.iter().map(|s| s.to_string()).collect();
        node
    }

    let mut nodes = vec![
        MenuNode::group("account", None, "账务处理", 10),
        page(
            "account.deposit",
            "account",
            "活期存款",
            10,
            &["action.deposit.print"],
        ),
        page(
            "account.withdraw",
            "account",
            "活期取款",
            20,
            &["action.withdraw.print", "action.withdraw.reversal"],
        ),
        page(
            "account.transfer",
            "account",
            "行内转账",
            30,
            &["action.transfer.print"],
        ),
        MenuNode::group("customer", None, "客户管理", 20),
        page("customer.query", "customer", "客户信息查询", 10, &[]),
        page(
            "customer.open",
            "customer",
            "开户",
            20,
            &["action.customer.print"],
        ),
        page("customer.profile", "customer", "客户画像", 30, &[]),
        MenuNode::group("credit", None, "信贷业务", 30),
        page("credit.loan.apply", "credit", "贷款申请", 10, &[]),
        page(
            "credit.loan.disburse",
            "credit",
            "贷款发放",
            20,
            &["action.loan.approve"],
        ),
        MenuNode::group("approval", None, "授权中心", 40),
        page("approval.queue", "approval", "待办授权", 10, &[]),
        page(
            "approval.reversal",
            "approval",
            "冲正处理",
            20,
            &["action.reversal.approve"],
        ),
        MenuNode::group("report", None, "查询报表", 50),
        page("report.flow", "report", "交易流水查询", 10, &[]),
        page(
            "report.daily",
            "report",
            "日终报表",
            20,
            &["action.report.export"],
        ),
        MenuNode::group("system", None, "系统管理", 60),
        page("system.device", "system", "设备管理", 10, &[]),
        page("system.cache", "system", "缓存管理", 20, &[]),
    ];

    // 演示 `status=hidden` 的效果：节点被授予但状态为 hidden 时不出现。
    if let Some(n) = nodes.iter_mut().find(|n| n.code == "system.cache") {
        n.status = NodeStatus::Hidden;
    }

    MenuTree::new(menu_version, nodes)
}

/// 演示角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoRole {
    Teller,
    Supervisor,
    Lobby,
}

impl DemoRole {
    pub const ALL: [DemoRole; 3] = [DemoRole::Teller, DemoRole::Supervisor, DemoRole::Lobby];

    pub fn id(self) -> &'static str {
        match self {
            DemoRole::Teller => "teller",
            DemoRole::Supervisor => "supervisor",
            DemoRole::Lobby => "lobby",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DemoRole::Teller => "柜员",
            DemoRole::Supervisor => "主管",
            DemoRole::Lobby => "大堂经理",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.id() == id)
    }

    /// 三个角色的菜单/按钮授权明显不同，用来验证"同一份 .mtree 服务多角色"。
    pub fn menu_codes(self) -> Vec<&'static str> {
        match self {
            DemoRole::Teller => vec![
                "account.deposit",
                "account.withdraw",
                "account.transfer",
                "customer.query",
                "customer.open",
                "report.flow",
            ],
            DemoRole::Supervisor => vec![
                // 注意：主管没有 account.* 的叶子，但通过 approval 等分支仍能看到部分菜单；
                // 这里刻意让"父节点无权、子节点有权"的路径出现。
                "account.withdraw",
                "approval.queue",
                "approval.reversal",
                "credit.loan.disburse",
                "report.flow",
                "report.daily",
                "system.device",
            ],
            DemoRole::Lobby => vec![
                "customer.query",
                "customer.profile",
                "report.flow",
                "account.transfer",
            ],
        }
    }

    pub fn action_codes(self) -> Vec<&'static str> {
        match self {
            DemoRole::Teller => vec![
                "action.deposit.print",
                "action.withdraw.print",
                "action.transfer.print",
                "action.customer.print",
            ],
            DemoRole::Supervisor => vec![
                "action.withdraw.print",
                "action.withdraw.reversal",
                "action.reversal.approve",
                "action.loan.approve",
                "action.report.export",
            ],
            DemoRole::Lobby => vec!["action.transfer.print"],
        }
    }

    pub fn shortcuts(self) -> BTreeMap<String, String> {
        let mut m = BTreeMap::new();
        m.insert("account.deposit".to_string(), "Alt+1".to_string());
        m.insert("account.withdraw".to_string(), "Alt+2".to_string());
        m.insert("account.transfer".to_string(), "Alt+3".to_string());
        if self == DemoRole::Supervisor {
            m.insert("approval.queue".to_string(), "Alt+A".to_string());
        }
        m
    }

    pub fn snapshot(self, perm_version: u64) -> PermSnapshot {
        PermSnapshot {
            user_id: format!("u-{}", self.id()),
            teller_no: match self {
                DemoRole::Teller => "T0231",
                DemoRole::Supervisor => "T0101",
                DemoRole::Lobby => "T0450",
            }
            .to_string(),
            outlet_code: "01023".to_string(),
            perm_version,
            menu_codes: self.menu_codes().into_iter().map(String::from).collect(),
            action_codes: self.action_codes().into_iter().map(String::from).collect(),
            shortcuts: self.shortcuts(),
            role_label: self.label().to_string(),
        }
    }
}

/// 版本探测响应（真实环境应 < 1KB）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MenuVersionProbe {
    pub latest: u64,
    pub shell_compat_min: u16,
    pub shell_compat_max: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    #[error("网络不可达（mock offline）")]
    Offline,
    #[error("菜单包构建失败: {0}")]
    Pack(String),
}

/// mock 服务端。`offline` 用来验证"断网启动仍可用本地菜单包"。
#[derive(Debug, Clone)]
pub struct MockHost {
    pub menu_version: u64,
    pub offline: bool,
    /// 每次"下载"都在这里切换成被篡改的包，用于演示验签失败回退。
    pub serve_tampered: bool,
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            menu_version: 7,
            offline: false,
            serve_tampered: false,
        }
    }
}

impl MockHost {
    pub fn new(menu_version: u64) -> Self {
        Self {
            menu_version,
            ..Default::default()
        }
    }

    pub fn probe(&self) -> Result<MenuVersionProbe, HostError> {
        if self.offline {
            return Err(HostError::Offline);
        }
        Ok(MenuVersionProbe {
            latest: self.menu_version,
            shell_compat_min: SHELL_COMPAT_MIN,
            shell_compat_max: SHELL_COMPAT_MAX,
        })
    }

    pub fn download(&self) -> Result<Vec<u8>, HostError> {
        if self.offline {
            return Err(HostError::Offline);
        }
        let tree = demo_tree(self.menu_version);
        let mut bytes = MenuPack::encode(
            &tree,
            SHELL_COMPAT_MIN,
            SHELL_COMPAT_MAX,
            &demo_signing_key(),
        )
        .map_err(|e| HostError::Pack(e.to_string()))?;
        if self.serve_tampered && bytes.len() > crate::pack::HEADER_LEN + 3 {
            // 故意改坏一个 payload 字节：客户端必须拒绝并回退。
            let idx = crate::pack::HEADER_LEN + 3;
            bytes[idx] ^= 0xFF;
        }
        Ok(bytes)
    }

    pub fn login(&self, role_id: &str, perm_version: u64) -> Option<PermSnapshot> {
        DemoRole::from_id(role_id).map(|r| r.snapshot(perm_version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::filter_visible;
    use crate::pack;

    #[test]
    fn demo_tree_is_valid() {
        demo_tree(7).validate().unwrap();
    }

    #[test]
    fn three_roles_see_three_different_menus() {
        let tree = demo_tree(7);
        let mut seen = Vec::new();
        for role in DemoRole::ALL {
            let view = filter_visible(&tree, &role.snapshot(1));
            let mut codes = Vec::new();
            for n in &view {
                n.collect_codes(&mut codes);
            }
            codes.sort();
            seen.push(codes.join(","));
        }
        assert_ne!(seen[0], seen[1]);
        assert_ne!(seen[1], seen[2]);
        assert_ne!(seen[0], seen[2]);
    }

    #[test]
    fn teller_cannot_see_approval_center() {
        let tree = demo_tree(7);
        let view = filter_visible(&tree, &DemoRole::Teller.snapshot(1));
        let mut codes = Vec::new();
        for n in &view {
            n.collect_codes(&mut codes);
        }
        assert!(!codes.iter().any(|c| c.starts_with("approval")));
        assert!(codes.contains(&"account.deposit".to_string()));
    }

    #[test]
    fn hidden_node_never_shows_even_when_granted() {
        let tree = demo_tree(7);
        let snap = DemoRole::Supervisor.snapshot(1);
        let view = filter_visible(&tree, &snap);
        let mut codes = Vec::new();
        for n in &view {
            n.collect_codes(&mut codes);
        }
        assert!(!codes.contains(&"system.cache".to_string()));
        assert!(codes.contains(&"system.device".to_string()));
    }

    #[test]
    fn downloaded_pack_verifies_and_matches_tree() {
        let host = MockHost::new(9);
        let bytes = host.download().unwrap();
        let tree = pack::open(&bytes, &demo_verifying_key()).unwrap();
        assert_eq!(tree, demo_tree(9));
        assert_eq!(pack::peek_version(&bytes), Some(9));
    }

    #[test]
    fn tampered_download_is_rejected() {
        let host = MockHost {
            serve_tampered: true,
            ..MockHost::new(9)
        };
        let bytes = host.download().unwrap();
        assert!(pack::open(&bytes, &demo_verifying_key()).is_err());
    }

    #[test]
    fn offline_host_errors() {
        let host = MockHost {
            offline: true,
            ..MockHost::new(9)
        };
        assert_eq!(host.probe().unwrap_err(), HostError::Offline);
        assert_eq!(host.download().unwrap_err(), HostError::Offline);
    }
}

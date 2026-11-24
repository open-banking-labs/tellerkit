//! 菜单节点与全量菜单树的契约。
//!
//! 设计要点：
//! - 菜单以**扁平列表**存储（`Vec<MenuNode>`），父子关系用 `parent_code` 表达。
//!   扁平结构让"业务包增量合并"变成一次追加 + 一次排序，而不是深拷贝整棵树。
//! - 全量树服务所有角色；权限差异只体现在 `PermSnapshot`，不存在"按角色打包"。

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use std::collections::{BTreeMap, HashMap, HashSet};

/// 节点类型。`Group` 只是容器，`Page` 可导航，`Action` 是按钮级权限点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum NodeType {
    Group,
    Page,
    Action,
}

impl NodeType {
    /// 只有 Group 和 Page 会出现在导航树里；Action 只作为 Code 被引用。
    pub fn is_navigable(self) -> bool {
        matches!(self, NodeType::Group | NodeType::Page)
    }
}

/// 节点状态。`Hidden` 用于灰度下线，不需要改权限快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeStatus {
    Active,
    Hidden,
}

fn default_source() -> String {
    "shell".to_string()
}

fn default_status() -> NodeStatus {
    NodeStatus::Active
}

fn default_sort() -> i32 {
    0
}

/// 单个菜单节点。
///
/// `code` 必须与权限码体系对齐，`PermSnapshot.menu_codes` 里存的就是这些值。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuNode {
    pub code: String,
    #[serde(default)]
    pub parent_code: Option<String>,
    pub title: String,
    #[serde(default)]
    pub title_key: Option<String>,
    #[serde(rename = "type")]
    pub node_type: NodeType,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default = "default_sort")]
    pub sort: i32,
    /// 该页面挂载的按钮级权限码（仅 Page 使用）。
    #[serde(default)]
    pub actions: Vec<String>,
    /// 来源标记：`shell` 或 `pkg:<packageId>`，用于排障与冲突定位。
    #[serde(default = "default_source")]
    pub source: String,
    #[serde(default = "default_status")]
    pub status: NodeStatus,
}

impl MenuNode {
    /// 便捷构造：页面节点。
    pub fn page(code: &str, parent: Option<&str>, title: &str, sort: i32) -> Self {
        Self {
            code: code.to_string(),
            parent_code: parent.map(|s| s.to_string()),
            title: title.to_string(),
            title_key: None,
            node_type: NodeType::Page,
            route: Some(format!("/tx/{}", code.replace('.', "/"))),
            icon: None,
            sort,
            actions: Vec::new(),
            source: "shell".to_string(),
            status: NodeStatus::Active,
        }
    }

    /// 便捷构造：分组节点。
    pub fn group(code: &str, parent: Option<&str>, title: &str, sort: i32) -> Self {
        Self {
            node_type: NodeType::Group,
            route: None,
            ..Self::page(code, parent, title, sort)
        }
    }
}

/// 全量菜单树（扁平存储）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuTree {
    /// 菜单包版本号，与 `.mtree` 头部、服务端 `menuVersion` 一致。
    pub menu_version: u64,
    pub nodes: Vec<MenuNode>,
}

/// 菜单树结构错误。这些错误在任何时候都应导致"拒绝使用该包"。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TreeError {
    #[error("重复的菜单 code: {0}")]
    DuplicateCode(String),
    #[error("节点 {code} 的父节点 {parent} 不存在")]
    MissingParent { code: String, parent: String },
    #[error("菜单树存在环，涉及节点: {0}")]
    Cycle(String),
    #[error("Action 节点不能作为父节点: {0}")]
    ActionAsParent(String),
}

impl MenuTree {
    pub fn new(menu_version: u64, nodes: Vec<MenuNode>) -> Self {
        Self {
            menu_version,
            nodes,
        }
    }

    /// 结构校验：重复 code、父节点缺失、环、Action 作为父节点。
    pub fn validate(&self) -> Result<(), TreeError> {
        let mut codes = HashSet::new();
        for node in &self.nodes {
            if !codes.insert(node.code.as_str()) {
                return Err(TreeError::DuplicateCode(node.code.clone()));
            }
        }
        for node in &self.nodes {
            if let Some(parent) = &node.parent_code {
                if !codes.contains(parent.as_str()) {
                    return Err(TreeError::MissingParent {
                        code: node.code.clone(),
                        parent: parent.clone(),
                    });
                }
            }
        }
        // 环检测：沿 parent 链上溯，步数上限为节点数。
        let by_code: HashMap<&str, &MenuNode> =
            self.nodes.iter().map(|n| (n.code.as_str(), n)).collect();
        for node in &self.nodes {
            let mut cursor = node.parent_code.as_deref();
            let mut steps = 0usize;
            while let Some(code) = cursor {
                steps += 1;
                if steps > self.nodes.len() {
                    return Err(TreeError::Cycle(node.code.clone()));
                }
                if code == node.code {
                    return Err(TreeError::Cycle(node.code.clone()));
                }
                cursor = by_code.get(code).and_then(|n| n.parent_code.as_deref());
            }
        }
        // Action 不能有子节点。
        for node in &self.nodes {
            if node.node_type == NodeType::Action {
                if self
                    .nodes
                    .iter()
                    .any(|n| n.parent_code.as_deref() == Some(node.code.as_str()))
                {
                    return Err(TreeError::ActionAsParent(node.code.clone()));
                }
            }
        }
        Ok(())
    }

    /// 按 code 建立索引。
    pub fn index(&self) -> HashMap<&str, &MenuNode> {
        self.nodes.iter().map(|n| (n.code.as_str(), n)).collect()
    }

    /// 根节点（parent 为空），按 sort 排序。
    pub fn roots(&self) -> Vec<&MenuNode> {
        let mut out: Vec<&MenuNode> = self
            .nodes
            .iter()
            .filter(|n| n.parent_code.is_none())
            .collect();
        sort_nodes(&mut out);
        out
    }

    /// 指定父节点的直接子节点，按 sort 排序。
    pub fn children_of(&self, parent: &str) -> Vec<&MenuNode> {
        let mut out: Vec<&MenuNode> = self
            .nodes
            .iter()
            .filter(|n| n.parent_code.as_deref() == Some(parent))
            .collect();
        sort_nodes(&mut out);
        out
    }

    /// 合并业务包注入的增量节点。
    ///
    /// 冲突策略：同一 code 重复出现时，**已存在者优先**，包节点被丢弃并记录。
    /// 这样业务包永远无法覆盖主壳的基础菜单，避免"包劫持导航"。
    pub fn merge_package(&mut self, nodes: Vec<MenuNode>) -> Vec<String> {
        let mut existing: HashSet<String> = self.nodes.iter().map(|n| n.code.clone()).collect();
        let mut rejected = Vec::new();
        for node in nodes {
            if existing.insert(node.code.clone()) {
                self.nodes.push(node);
            } else {
                rejected.push(node.code);
            }
        }
        rejected
    }

    /// 导出 code -> title 映射，便于审计日志记录"用户点了什么"。
    pub fn title_map(&self) -> BTreeMap<&str, &str> {
        self.nodes
            .iter()
            .map(|n| (n.code.as_str(), n.title.as_str()))
            .collect()
    }
}

fn sort_nodes(nodes: &mut Vec<&MenuNode>) {
    nodes.sort_by(|a, b| a.sort.cmp(&b.sort).then_with(|| a.code.cmp(&b.code)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(nodes: Vec<MenuNode>) -> MenuTree {
        MenuTree::new(1, nodes)
    }

    #[test]
    fn validate_rejects_duplicate_code() {
        let t = tree(vec![
            MenuNode::group("a", None, "A", 1),
            MenuNode::page("a", None, "A2", 2),
        ]);
        assert_eq!(t.validate(), Err(TreeError::DuplicateCode("a".into())));
    }

    #[test]
    fn validate_rejects_missing_parent() {
        let t = tree(vec![MenuNode::page("a.b", Some("a"), "B", 1)]);
        assert!(matches!(t.validate(), Err(TreeError::MissingParent { .. })));
    }

    #[test]
    fn validate_rejects_cycle() {
        let mut a = MenuNode::group("a", Some("b"), "A", 1);
        let b = MenuNode::group("b", Some("a"), "B", 1);
        a.parent_code = Some("b".into());
        let t = tree(vec![a, b]);
        assert!(matches!(t.validate(), Err(TreeError::Cycle(_))));
    }

    #[test]
    fn validate_rejects_action_as_parent() {
        let mut action = MenuNode::page("a", None, "A", 1);
        action.node_type = NodeType::Action;
        let t = tree(vec![action, MenuNode::page("child", Some("a"), "C", 1)]);
        assert!(matches!(t.validate(), Err(TreeError::ActionAsParent(_))));
    }

    #[test]
    fn merge_package_keeps_existing_and_reports_conflicts() {
        let mut t = tree(vec![MenuNode::group("a", None, "A", 1)]);
        let rejected = t.merge_package(vec![
            MenuNode::group("a", None, "A-hijack", 1),
            MenuNode::page("a.x", Some("a"), "X", 1),
        ]);
        assert_eq!(rejected, vec!["a".to_string()]);
        assert_eq!(t.nodes.len(), 2);
        assert_eq!(t.nodes[0].title, "A");
    }

    #[test]
    fn children_are_sorted_by_sort_then_code() {
        let t = tree(vec![
            MenuNode::group("root", None, "R", 0),
            MenuNode::page("root.b", Some("root"), "B", 2),
            MenuNode::page("root.a", Some("root"), "A", 1),
        ]);
        let codes: Vec<_> = t
            .children_of("root")
            .iter()
            .map(|n| n.code.clone())
            .collect();
        assert_eq!(codes, vec!["root.a", "root.b"]);
    }
}

//! 菜单可见性剪枝。
//!
//! 唯一规则（PLAN.md §5.3）：
//! 1. `status == Hidden` 一律不可见；
//! 2. 节点自身 code 在快照里 → 可见（`direct = true`）；
//! 3. 或它的任意后代可见 → 容器保留（`direct = false`），避免"父节点无权"连带
//!    隐藏有权的子孙；
//! 4. Action 节点不参与导航，只作为页面上的按钮码被过滤。
//!
//! 整个函数是纯内存递归，2000 节点量级 < 1ms。

use crate::menu::model::{MenuNode, MenuTree, NodeStatus, NodeType};
use crate::perm::PermSnapshot;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use ts_rs::TS;

/// 给前端渲染用的可见菜单节点。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MenuView {
    code: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title_key: Option<String>,
    #[serde(rename = "type")]
    node_type: NodeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    route: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    sort: i32,
    /// `shell` 或 `pkg:<id>`。前端据此决定用主壳页面还是加载业务包模块。
    source: String,
    /// 自身是否被授予权限。`false` 表示它只是"容器"（因为子孙有权才留下），
    /// 前端应渲染为不可点击的分组标题。
    direct: bool,
    /// 过滤后的按钮级权限码。
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    children: Vec<MenuView>,
}

impl MenuView {
    /// 收集本子树所有可见 code（不含 Action）。
    pub fn collect_codes(&self, out: &mut Vec<String>) {
        out.push(self.code.clone());
        for child in &self.children {
            child.collect_codes(out);
        }
    }
}

/// 按权限快照剪枝，返回可直接渲染的菜单森林。
///
/// 调用前应确保 `tree.validate()` 通过；本函数对非法结构做防御性处理
/// （环会被访问集切断，不会死循环）。
pub fn filter_visible(tree: &MenuTree, snapshot: &PermSnapshot) -> Vec<MenuView> {
    let menu_set = snapshot.menu_set();
    let action_set = snapshot.action_set();

    let mut children: HashMap<&str, Vec<&MenuNode>> = HashMap::new();
    let mut roots: Vec<&MenuNode> = Vec::new();
    for node in tree.nodes() {
        if !node.node_type().is_navigable() {
            continue; // Action 不进导航树
        }
        match node.parent_code() {
            Some(parent) => children.entry(parent).or_default().push(node),
            None => roots.push(node),
        }
    }
    for list in children.values_mut() {
        sort_nodes(list);
    }
    roots.sort_by(|a, b| a.sort().cmp(&b.sort()).then_with(|| a.code().cmp(b.code())));

    // 访问集用 HashSet：Vec 线性查重会让过滤退化成 O(n²)，
    // 顺手用 insert 完成「查重 + 登记」，无需每个节点 clone code。
    let mut visited: HashSet<&str> = HashSet::new();
    roots
        .into_iter()
        .filter_map(|node| build(node, &children, &menu_set, &action_set, &mut visited))
        .collect()
}

fn sort_nodes(list: &mut [&MenuNode]) {
    list.sort_by(|a, b| a.sort().cmp(&b.sort()).then_with(|| a.code().cmp(b.code())));
}

fn build<'a>(
    node: &'a MenuNode,
    children: &HashMap<&'a str, Vec<&'a MenuNode>>,
    menu_set: &HashSet<&'a str>,
    action_set: &HashSet<&'a str>,
    visited: &mut HashSet<&'a str>,
) -> Option<MenuView> {
    if node.status() == NodeStatus::Hidden {
        return None;
    }
    // 防御：环或重复访问直接剪掉，保证不死循环。
    if !visited.insert(node.code()) {
        return None;
    }

    let mut kids = Vec::new();
    if let Some(list) = children.get(node.code()) {
        for child in list {
            if let Some(view) = build(child, children, menu_set, action_set, visited) {
                kids.push(view);
            }
        }
    }

    let direct = menu_set.contains(node.code());
    // 自身无权且没有任何可见子孙 → 整棵子树剪掉。
    if !direct && kids.is_empty() {
        return None;
    }

    let actions = node
        .actions()
        .iter()
        .filter(|a| action_set.contains(a.as_str()))
        .cloned()
        .collect();

    Some(MenuView {
        code: node.code().to_string(),
        title: node.title().to_string(),
        title_key: node.title_key().map(|s| s.to_string()),
        node_type: node.node_type(),
        route: node.route().map(|s| s.to_string()),
        icon: node.icon().map(|s| s.to_string()),
        sort: node.sort(),
        source: node.source().to_string(),
        direct,
        actions,
        children: kids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::model::MenuNode;
    use crate::perm::PermSnapshot;
    use std::collections::BTreeMap;

    fn snapshot(menus: &[&str], actions: &[&str]) -> PermSnapshot {
        PermSnapshot::new(
            "u",
            "T1",
            "O1",
            1,
            menus.iter().map(|s| s.to_string()).collect(),
            actions.iter().map(|s| s.to_string()).collect(),
            BTreeMap::new(),
            "test",
        )
    }

    fn sample_tree() -> MenuTree {
        // 存款（组）
        //   ├─ 活期存款（页）      actions: action.deposit.print
        //   └─ 定期存款（页）
        // 贷款（组）
        //   └─ 贷款发放（页）
        MenuTree::new(
            3,
            vec![
                MenuNode::group("deposit", None, "存款", 1),
                MenuNode::page("deposit.demand", Some("deposit"), "活期存款", 1),
                MenuNode::page("deposit.time", Some("deposit"), "定期存款", 2),
                MenuNode::group("loan", None, "贷款", 2),
                MenuNode::page("loan.disburse", Some("loan"), "贷款发放", 1),
            ],
        )
    }

    /// 便捷构造：Action 节点。只承载按钮权限码，不是导航入口。
    fn action_node(code: &str, parent: Option<&str>) -> MenuNode {
        MenuNode::new(code, parent, code, NodeType::Action, 1)
    }

    #[test]
    fn action_nodes_never_enter_navigation() {
        // S-02 的 account 树 + Action 边界：Action 即使出现在快照的 menu_codes 里，
        // 也只能作为按钮码，不得成为根入口或 children，也不得让无权父节点复活。
        let tree = MenuTree::new(
            1,
            vec![
                MenuNode::group("account", None, "账户", 1),
                MenuNode::page("account.withdraw", Some("account"), "取款", 1),
                MenuNode::page("account.deposit", Some("account"), "存款", 2),
                action_node("account.withdraw.print", Some("account.withdraw")),
                action_node("account.deposit.print", Some("account.deposit")),
                action_node("account.batch", None),
            ],
        );
        assert!(tree.validate().is_ok(), "Action 只能做叶子，此树应结构合法");

        let perm = snapshot(
            &[
                "account.withdraw",
                "account.withdraw.print",
                "account.deposit.print",
                "account.batch",
            ],
            &[],
        );
        let view = filter_visible(&tree, &perm);

        let mut codes = Vec::new();
        for root in &view {
            root.collect_codes(&mut codes);
        }
        assert_eq!(
            codes,
            vec!["account".to_string(), "account.withdraw".to_string()],
            "导航树里只能有容器 account 与被授予的 account.withdraw"
        );
        assert_eq!(view.len(), 1, "根级 Action 不得成为导航入口");
        assert!(!view[0].direct, "account 未被授予，应作为容器");
        assert_eq!(view[0].children.len(), 1);
        assert_eq!(view[0].children[0].code, "account.withdraw");
        assert!(view[0].children[0].direct, "account.withdraw 可点击");
        assert!(
            view[0].children[0].children.is_empty(),
            "Action 不能作为导航子节点"
        );
        assert!(
            !codes.contains(&"account.deposit".to_string()),
            "未被授予的 account.deposit 必须剪掉，其下的 Action 不能让它复活"
        );
    }

    #[test]
    fn keeps_granted_leaf_and_its_ancestors() {
        let tree = sample_tree();
        let perm = snapshot(&["deposit.demand"], &[]);
        let view = filter_visible(&tree, &perm);

        assert_eq!(view.len(), 1);
        assert_eq!(view[0].code, "deposit");
        assert!(!view[0].direct, "父节点无权，应标记为容器");
        assert_eq!(view[0].children.len(), 1);
        assert_eq!(view[0].children[0].code, "deposit.demand");
        assert!(view[0].children[0].direct);
    }

    #[test]
    fn parent_without_permission_keeps_granted_children() {
        // 关键用例：父不在集合、子都在集合 → 父必须保留。
        let tree = sample_tree();
        let perm = snapshot(&["deposit.demand", "deposit.time"], &[]);
        let view = filter_visible(&tree, &perm);

        assert_eq!(view.len(), 1);
        assert_eq!(view[0].code, "deposit");
        assert!(!view[0].direct);
        assert_eq!(view[0].children.len(), 2);
    }

    #[test]
    fn parent_with_permission_drops_ungranted_children() {
        let tree = sample_tree();
        let perm = snapshot(&["deposit"], &[]);
        let view = filter_visible(&tree, &perm);

        assert_eq!(view.len(), 1);
        assert!(view[0].direct);
        assert!(
            view[0].children.is_empty(),
            "父有权但子无权时，子节点必须被剪掉"
        );
    }

    #[test]
    fn empty_permission_yields_empty_menu() {
        let tree = sample_tree();
        let view = filter_visible(&tree, &snapshot(&[], &[]));
        assert!(view.is_empty());
    }

    #[test]
    fn unrelated_group_is_pruned() {
        let tree = sample_tree();
        let perm = snapshot(&["loan.disburse"], &[]);
        let view = filter_visible(&tree, &perm);
        assert_eq!(view.len(), 1);
        assert_eq!(view[0].code, "loan");
    }

    #[test]
    fn hidden_node_is_pruned_even_when_granted() {
        let mut tree = sample_tree();
        if let Some(n) = tree
            .nodes_mut()
            .iter_mut()
            .find(|n| n.code() == "deposit.time")
        {
            n.set_status(NodeStatus::Hidden);
        }
        let perm = snapshot(&["deposit.demand", "deposit.time"], &[]);
        let view = filter_visible(&tree, &perm);
        let codes: Vec<String> = {
            let mut v = Vec::new();
            for node in &view {
                node.collect_codes(&mut v);
            }
            v
        };
        assert!(!codes.contains(&"deposit.time".to_string()));
        assert!(codes.contains(&"deposit.demand".to_string()));
    }

    #[test]
    fn actions_are_filtered_by_action_codes() {
        let mut tree = sample_tree();
        if let Some(n) = tree
            .nodes_mut()
            .iter_mut()
            .find(|n| n.code() == "deposit.demand")
        {
            n.set_actions(vec!["action.print".into(), "action.reversal".into()]);
        }
        let perm = snapshot(&["deposit.demand"], &["action.print"]);
        let view = filter_visible(&tree, &perm);
        let leaf = &view[0].children[0];
        assert_eq!(leaf.actions, vec!["action.print".to_string()]);
    }

    #[test]
    fn deep_nesting_is_pruned_correctly() {
        // a > b > c > d，只给 d。
        let tree = MenuTree::new(
            1,
            vec![
                MenuNode::group("a", None, "A", 1),
                MenuNode::group("b", Some("a"), "B", 1),
                MenuNode::group("c", Some("b"), "C", 1),
                MenuNode::page("d", Some("c"), "D", 1),
            ],
        );
        let view = filter_visible(&tree, &snapshot(&["d"], &[]));
        assert_eq!(view[0].code, "a");
        assert_eq!(view[0].children[0].code, "b");
        assert_eq!(view[0].children[0].children[0].code, "c");
        assert_eq!(view[0].children[0].children[0].children[0].code, "d");
        assert!(view[0].children[0].children[0].children[0].direct);
    }

    #[test]
    fn cyclic_tree_does_not_hang() {
        // 故意构造环，filter 必须能安全返回。
        let tree = MenuTree::new(
            1,
            vec![
                MenuNode::group("a", Some("b"), "A", 1),
                MenuNode::group("b", Some("a"), "B", 1),
            ],
        );
        let view = filter_visible(&tree, &snapshot(&["a", "b"], &[]));
        assert!(view.is_empty(), "环状子树不应作为根被渲染出来");
    }

    #[test]
    fn sibling_order_follows_sort() {
        let tree = sample_tree();
        let perm = snapshot(&["deposit.demand", "deposit.time"], &[]);
        let view = filter_visible(&tree, &perm);
        let codes: Vec<&str> = view[0].children.iter().map(|c| c.code.as_str()).collect();
        assert_eq!(codes, vec!["deposit.demand", "deposit.time"]);
    }
}

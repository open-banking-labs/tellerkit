//! 规模回归：6 万节点的菜单校验与过滤不得退回 O(n²)。
//!
//! 这里只钉复杂度级别：O(n²) 在 6 万节点上是数十亿次比较，任何机器都要到秒级；
//! 500ms 的宽松上限既能抓住退化，又不会因为 CI 机器慢而随机抖动。
//! 精确数字可手动跑：`cargo test -p tellerkit-core --test menu_scale -- --nocapture`。

use std::collections::BTreeMap;
use std::time::Instant;

use tellerkit_core::{filter_visible, MenuNode, MenuTree, NodeStatus, NodeType, PermSnapshot};

/// 20 个分组 × 1500 个页面 × (页面 + 按钮码) + 分组本身 = 60020 个节点。
const GROUPS: usize = 20;
const PAGES_PER_GROUP: usize = 1_500;
const HIDDEN_PAGE: &str = "g.0.p0";

fn big_tree(hide_page: bool) -> MenuTree {
    let mut nodes = Vec::with_capacity(GROUPS * (PAGES_PER_GROUP * 2 + 1));
    for group in 0..GROUPS {
        let group_code = format!("g.{group}");
        nodes.push(MenuNode::group(&group_code, None, "分组", group as i32));
        for page in 0..PAGES_PER_GROUP {
            let code = format!("g.{group}.p{page}");
            let mut page_node = MenuNode::page(&code, Some(&group_code), "页面", page as i32);
            if hide_page && code == HIDDEN_PAGE {
                page_node.set_status(NodeStatus::Hidden);
            }
            nodes.push(page_node);
            // 真实菜单每页都挂按钮码：Action 节点是「Action 不能当父节点」校验
            // 退化为 O(n²) 的触发器，同时它们不应出现在导航树里。
            nodes.push(MenuNode::new(
                &format!("{code}.act"),
                Some(&code),
                "按钮",
                NodeType::Action,
                0,
            ));
        }
    }
    MenuTree::new(1, nodes)
}

fn snapshot_with_all_pages() -> PermSnapshot {
    let menu_codes = (0..GROUPS)
        .flat_map(|group| (0..PAGES_PER_GROUP).map(move |page| format!("g.{group}.p{page}")))
        .collect();
    PermSnapshot::new(
        "u-1",
        "T0001",
        "OUTLET-01",
        1,
        menu_codes,
        Vec::new(),
        BTreeMap::new(),
        "柜员",
    )
}

/// 可见导航树：分组 + 页面，不含按钮码；隐藏页也被排除。
fn expected_codes(skip_hidden: bool) -> Vec<String> {
    let mut codes = Vec::with_capacity(GROUPS * (PAGES_PER_GROUP + 1));
    for group in 0..GROUPS {
        codes.push(format!("g.{group}"));
        for page in 0..PAGES_PER_GROUP {
            let code = format!("g.{group}.p{page}");
            if skip_hidden && code == HIDDEN_PAGE {
                continue;
            }
            codes.push(code);
        }
    }
    codes
}

fn visible_codes(tree: &MenuTree, snapshot: &PermSnapshot) -> Vec<String> {
    let mut codes = Vec::new();
    for root in &filter_visible(tree, snapshot) {
        root.collect_codes(&mut codes);
    }
    codes
}

#[test]
fn sixty_thousand_nodes_validate_and_filter_stay_fast() {
    let tree = big_tree(false);
    assert_eq!(tree.node_count(), GROUPS * (PAGES_PER_GROUP * 2 + 1));
    let snapshot = snapshot_with_all_pages();
    let expected = expected_codes(false);

    let started = Instant::now();
    tree.validate().expect("大树必须合法");
    let validate_ms = started.elapsed().as_secs_f64() * 1_000.0;

    let started = Instant::now();
    let codes = visible_codes(&tree, &snapshot);
    let filter_ms = started.elapsed().as_secs_f64() * 1_000.0;

    assert_eq!(codes, expected, "导航树应只含可见分组与页面");
    println!("6 万节点：validate={validate_ms:.1}ms filter={filter_ms:.1}ms");
    assert!(
        validate_ms < 500.0,
        "validate 退化了（{validate_ms:.1}ms）：检查是否又出现全表扫描"
    );
    assert!(
        filter_ms < 500.0,
        "filter_visible 退化了（{filter_ms:.1}ms）：检查访问集是否又变成线性查重"
    );
}

#[test]
fn hidden_nodes_and_actions_are_still_pruned_at_scale() {
    let tree = big_tree(true);
    tree.validate().expect("隐藏页与 Action 不影响合法性");

    let codes = visible_codes(&tree, &snapshot_with_all_pages());
    assert_eq!(codes, expected_codes(true));
    assert!(!codes.iter().any(|code| code.ends_with(".act")));
    assert_eq!(codes.len(), GROUPS * (PAGES_PER_GROUP + 1) - 1);
}

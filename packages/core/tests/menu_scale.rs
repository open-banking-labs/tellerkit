//! 规模回归：2 万节点的菜单校验与过滤不得退回 O(n²)。
//!
//! 规模评审 P1-11 的验收是「2 万节点 validate < 50ms、filter_visible < 20ms」。
//! 但把 50ms 写进 CI 断言会随机抖动（Windows runner 比开发机慢），所以这里只钉
//! **复杂度级别**：O(n²) 在 2 万节点约 4 亿次比较，任何机器都要到秒级；
//! 给 500ms 的宽松上限，既能抓住退化，又不会因为机器慢而误报。
//!
//! 想要精确数字时手动跑：`cargo test -p tellerkit-core --test menu_scale -- --nocapture`。

use std::time::Instant;

use tellerkit_core::filter::filter_visible;
use tellerkit_core::model::{MenuNode, MenuTree, NodeStatus, NodeType};
use tellerkit_core::perm::PermSnapshot;

/// 20 个分组 × 1000 个页面 = 20020 个节点，接近"两万个菜单节点"的真实上界。
const GROUPS: usize = 20;
const PAGES_PER_GROUP: usize = 1_000;

fn big_tree() -> MenuTree {
    let mut nodes = Vec::with_capacity(GROUPS * (PAGES_PER_GROUP + 1));
    for group in 0..GROUPS {
        let group_code = format!("g.{group}");
        nodes.push(MenuNode::group(&group_code, None, "分组", group as i32));
        for page in 0..PAGES_PER_GROUP {
            let code = format!("g.{group}.p{page}");
            nodes.push(MenuNode::page(
                &code,
                Some(&group_code),
                "页面",
                page as i32,
            ));
            // 真实菜单里每个页面都挂着按钮码（Action）。它们也是 O(n²) 退化
            // 的触发器：校验"Action 不能当父节点"时若每个 Action 都扫全表，
            // 2 万页面就是数亿次比较（规模评审 P1-11 点的正是这里）。
            nodes.push(MenuNode {
                node_type: NodeType::Action,
                ..MenuNode::page(&format!("{code}.act"), Some(&code), "按钮", 0)
            });
        }
    }
    MenuTree {
        menu_version: 1,
        nodes,
    }
}

fn snapshot_with_all_children() -> PermSnapshot {
    let mut menu_codes = Vec::with_capacity(GROUPS * PAGES_PER_GROUP);
    for group in 0..GROUPS {
        for page in 0..PAGES_PER_GROUP {
            menu_codes.push(format!("g.{group}.p{page}"));
        }
    }
    PermSnapshot {
        user_id: "u-1".into(),
        teller_no: "T0001".into(),
        outlet_code: "OUTLET-01".into(),
        perm_version: 1,
        menu_codes,
        action_codes: Vec::new(),
        shortcuts: Default::default(),
        role_label: "柜员".into(),
    }
}

#[test]
fn twenty_thousand_nodes_validate_and_filter_stay_fast() {
    let tree = big_tree();
    // 分组 + 页面 + 按钮码：20 × (1 + 1000 + 1000)
    assert_eq!(tree.nodes.len(), GROUPS * (PAGES_PER_GROUP * 2 + 1));

    let started = Instant::now();
    tree.validate().expect("大树必须合法");
    let validate_ms = started.elapsed().as_secs_f64() * 1_000.0;

    let snapshot = snapshot_with_all_children();
    let started = Instant::now();
    let visible = filter_visible(&tree, &snapshot);
    let filter_ms = started.elapsed().as_secs_f64() * 1_000.0;

    // 只有 20 个分组容器可见（子页面在各自 children 里）。
    assert_eq!(visible.len(), GROUPS, "分组数不对：{}", visible.len());
    let total_leaves: usize = visible.iter().map(|group| group.children.len()).sum();
    assert_eq!(total_leaves, GROUPS * PAGES_PER_GROUP);

    println!("2 万节点：validate={validate_ms:.1}ms filter={filter_ms:.1}ms");
    assert!(
        validate_ms < 500.0,
        "validate 退化了（{validate_ms:.1}ms）：检查是否又出现 O(n²) 全表扫描"
    );
    assert!(
        filter_ms < 500.0,
        "filter_visible 退化了（{filter_ms:.1}ms）：检查访问集是否又变成线性查重"
    );
}

#[test]
fn hidden_nodes_and_actions_are_still_pruned_at_scale() {
    let mut tree = big_tree();
    // 把 g.0.p0 改成隐藏页（直接改现有节点，不能新增同 code 节点——那是重复 code）。
    let hidden = tree
        .nodes
        .iter_mut()
        .find(|n| n.code == "g.0.p0")
        .expect("测试树里应有 g.0.p0");
    hidden.status = NodeStatus::Hidden;

    tree.validate().expect("隐藏页与 Action 不影响合法性");
    let visible = filter_visible(&tree, &snapshot_with_all_children());
    assert_eq!(visible.len(), GROUPS);

    let first_group = &visible[0];
    assert!(
        first_group.children.iter().all(|c| c.code != "g.0.p0"),
        "隐藏节点不得出现在可见菜单里"
    );
    assert!(
        first_group.children.iter().all(|c| c.code != "g.0.p1.act"),
        "Action 不进导航树"
    );
    assert_eq!(
        first_group.children.len(),
        PAGES_PER_GROUP - 1,
        "隐藏一个页面后，分组下应少一个可见子节点"
    );
}

# ADR-0004：借鉴 GPUI 的状态思想，不采用 GPUI 作为底座

- 状态：已接受
- 日期：2026-11-24

## 背景

Zed 的 GPUI 在基准测试中全面领先 Tauri（CPU、内存、首帧），且
长桥证券 Longbridge Pro 已从 Electron 重写到 GPUI 并投产。诱惑很大。

## 决策

- **不**把 GPUI / GPUI Kit 作为主框架。
- **借鉴**其"单一 App 状态 + Entity 所有权"模型：所有业务真相由一个
  Rust 侧 `AppState` 持有，前端只做渲染与事件转发。
- 高性能数据网格（数十万行流水查询）留到 P3 再做局部原型，不与主壳绑定。

## 理由

1. GPUI 处于 pre-1.0，演进节奏与 Zed 内部需求强绑定；柜面产品生命周期以年计。
2. GPUI 不自带控件，Longbridge 花了三年自建 60+ 组件；单人项目无法承受。
3. 已知迁移评估：渲染收益明确，但代价是重写 97k 行前端 + 331 个 e2e 测试，
   而真实收益约为界面内存降低 1/3，不是数量级改善。
4. 本项目的性能瓶颈不在 UI 渲染，而在外设联调与交易时序。

## 落地方式

`src-tauri/src/lib.rs` 的 `AppState` 就是这一思想的体现：

```rust
pub struct AppState {
    pub host: MockHost,          // 服务端替身
    pub store: MenuStore,        // 菜单包仓库
    pub snapshot: Option<PermSnapshot>,  // 当前用户权限
    pub menu: Option<MenuTree>,  // 全量树
}
```

前端拿不到全量树，只能拿 `filter_visible` 的结果——权限判断永远不在前端。

## 重新评估的触发条件

- GPUI 发布 1.0 且组件生态独立于 Zed；
- 或出现经实测无法用 WebView 承载的柜面场景（例如十万行实时刷新）。

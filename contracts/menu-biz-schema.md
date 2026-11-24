# 菜单与权限契约

## MenuNode（全量菜单树节点，存在于 `.mtree`）

| 字段 | 类型 | 说明 |
|---|---|---|
| `code` | string | 唯一，与权限码体系对齐，如 `account.deposit` |
| `parent_code` | string \| null | null 为根 |
| `title` | string | 展示名 |
| `title_key` | string \| null | i18n key |
| `type` | `"group"` \| `"page"` \| `"action"` | group 仅容器，page 可导航，action 为按钮级权限点 |
| `route` | string \| null | 页面路由 |
| `icon` | string \| null | 图标标识 |
| `sort` | number | 同级排序，小在前；同值按 code |
| `source` | string | `shell` 或 `pkg:<id>`，决定前端用主壳页面还是加载包模块 |
| `actions` | string[] | 该页面挂载的按钮级权限码 |
| `source` | string | `shell` 或 `pkg:<id>` |
| `status` | `"active"` \| `"hidden"` | hidden 用于灰度下线，不需要改权限快照 |

**结构约束**（`MenuTree::validate`，非法包直接拒绝）：
唯一 code · 父节点必须存在 · 无环 · action 不得有子节点。

## MenuView（前端拿到的已剪枝菜单）

| 字段 | 类型 | 说明 |
|---|---|---|
| `code` / `title` / `title_key` / `type` / `route` / `icon` / `sort` | 同上 | |
| `direct` | boolean | 自身被授权；`false` = 仅因子孙有权而保留的容器 |
| `actions` | string[] | **已按 actionCodes 过滤** |
| `children` | MenuView[] | 已剪枝的子节点 |

**前端不得再次判断权限**（宪法第 2 条）。`direct=false` 的节点应渲染为不可点击的容器。

## 可见性规则（唯一实现：`filter_visible`）

1. `status == hidden` → 一律不可见。
2. 节点自身 code 在 `menu_codes` 中 → 可见（`direct=true`）。
3. 或任意后代可见 → 容器保留（`direct=false`）。
4. action 节点不参与导航，只用于过滤页面上的按钮。

**必须保持的边界用例**：父节点无权、子节点有权 → 父保留为容器，子可点。

## PermSnapshot（登录响应，**不落盘**）

| 字段 | 类型 | 说明 |
|---|---|---|
| `user_id` | string | |
| `teller_no` | string | 柜员号，水印/审计用 |
| `outlet_code` | string | 网点号 |
| `perm_version` | number | 权限变更时递增，客户端据此决定是否重拉 |
| `menu_codes` | string[] | 可见的菜单 code（通常只放可导航节点） |
| `action_codes` | string[] | 可见的按钮级权限码 |
| `shortcuts` | Record<string,string> | menuCode → 组合键 |
| `role_label` | string | 展示用角色名 |

## `.mtree` 文件布局

```
[0..8)    magic  "MTKMTREE"
[8..10)   format_version  u16 LE = 1
[10..14)  payload_len     u32 LE
[14..22)  menu_version    u64 LE
[22..24)  shell_compat_min u16 LE
[24..26)  shell_compat_max u16 LE
[26..)    payload: MessagePack(MenuTree)，位置式序列化
[末 64B]  Ed25519 签名 over sha256(header || payload)
```

约束：签名覆盖头部，改版本号同样失效；写入必须 `tmp → fsync → rename`；
`.tmp` 永不作为有效包读取；文件名为 `menu.<version>.mtree`。

**安全边界**：签名只能检测篡改、阻止非授权方下发，**不防本机管理员**。
见 `docs/security-boundary.md`。

## `.bizpkg` 容器

业务包与菜单包用**不同的**容器：业务包带路由、交易元数据与依赖声明，
迭代节奏独立（菜单包随主壳发版，业务包按月甚至按周）。

```
[0..8)    magic "MTKPKG01"
[8..10)   format_version u16 LE = 1
[10..14)  payload_len u32 LE
[14..22)  预留（u64，当前为 0）
[22..26)  预留（u16 ×2，当前为 0）
[26..)    payload: MessagePack(BizPackage)
[末 64B]  Ed25519 签名 over sha256(header || payload)
```

### 字段命名

一律 **snake_case**，与 Rust serde 名一致：`min_shell` / `max_shell` /
`offline_capable`。包格式、契约、IPC 三处同一套命名，少一次翻译少一处错。

### 内容形状

| 文件 | 类型 |
|---|---|
| `manifest.json` | 对象：`id` / `name` / `version` / `min_shell` / `max_shell` / `deps` / `entry` |
| `menus.json` | 数组：`MenuNode`（见上） |
| `permissions.json` | 对象：`{ "actions": string[] }` |
| `routes.json` | 对象：路由路径 → 前端模块（顶层就是 map） |
| `transactions.json` | 数组：`code` / `idempotency` / `offline_capable` / `devices` |

### 加载约束

1. 包 ID 至少两段（`biz.demo`）
2. 菜单 code 必须等于包 ID 或以 `包ID.` 开头
3. 已存在的 code 一律保留主壳版本（包不得劫持导航）
4. `source` 在加载时强制改写为 `pkg:<包ID>`
5. 版本不兼容 / 缺依赖 / 成环 → 跳过该包并记录原因，不影响其他包

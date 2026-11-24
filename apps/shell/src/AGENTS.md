# src/AGENTS.md

## 职责

纯渲染 + 事件转发。**前端不持有业务真相，不做权限判断。**

## 允许依赖

`vue` · `@tauri-apps/api` · 本目录内的 `contracts.ts` / `labels.ts` / `mockBackend.ts`

## 禁止

- `import` 任何 Rust 侧概念（`filter_visible`、`menu_codes`、`PermSnapshot` 之外的判断逻辑）
- 用 `menu_codes` / `action_codes` 自行推导可见性——Rust 已经给过结果
- 直接读写文件、调用 `invoke` 之外的系统能力
- 在模板里内联色值：用 `styles.css` 的变量
- 为了"看起来完整"伪造数据；没有数据就显示"无"

## 约定

- 跨语言类型一律来自 `generated/`（由 Rust 生成，勿手改）；纯界面类型放 `ui-types.ts`
- 禁止新建 `contracts.ts`：手写契约文件会被 `check-contracts` 拦下
- 无 Tauri 运行时自动退到 `mockBackend`（数据由 `npm run fixtures` 用真实核心逻辑导出）

## 测试

```bash
npm run bindings  # 改了 Rust 后重新生成 generated/
npm run build     # vue-tsc 类型检查 + 打包，是这一层唯一的自动化门禁
npm run dev       # 浏览器预览（顶部会显示「浏览器预览 · 静态数据」）
```

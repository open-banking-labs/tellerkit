# 005 业务包前端模块与菜单来源字段

## 现状

`BizPackage.routes` 只是"路径 → 模块名"的映射，模块源码不在包里；
`MenuView` 也没有来源字段，前端无法判断某个页面该由主壳渲染还是加载包模块。

## 建议

1. `BizPackage` 增加 `assets: Map<路径, 源码>`，打包时把 `ui/*.js` 嵌进包。
2. `MenuView` 增加 `source: string`（`shell` 或 `pkg:<id>`）。
3. 新增命令 `package_module(route) -> string`，返回模块源码。
4. CSP 的 `script-src` 增加 `blob:`。

## 为什么源码进包、而不是给路径

包在运行时是外部文件，WebView 读不到任意本地路径；让前端拼路径等于把文件系统
访问权交给业务包。把源码交给前端，前端用 Blob URL 加载，路径始终留在 Rust 侧。

## 代价

- 包体积变大（demo 包 383B → 1946B）
- 包模块必须**自包含**：不能有 `import`，没有打包器解析依赖
- CSP 放开 `blob:`：这是为模块加载付的代价，需要写进安全边界文档

## 风险

Blob 模块与主壳同源、同 WebView，**没有进程级沙箱**。它能访问 `window` 与 DOM。
缓解只有三条：包必须签名、能力经 `shell` 白名单、CSP 限制外部加载。
真正的隔离（Worker / iframe / WASM）留到 P3 评估。

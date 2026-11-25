# 012 业务包模块沙箱

## 现状

业务包模块通过 Blob URL 在主窗口里 `import()`，与主壳同处一个 WebView。
它能直接读 `window`、`document`、`localStorage`、发 `fetch`。

后果：前面所有的权限剪枝、水印、剪贴板管控，**都能被一个签名包绕过**。
包是"签名但第三方"的代码，签名只证明来源，不证明它不会作恶。

## 建议

用 `sandbox="allow-scripts"` 的 iframe 承载包模块：

| 能力 | 结果 |
|---|---|
| 父页面 DOM / `window.parent` | 跨源，抛 `SecurityError` |
| `localStorage` / `cookie` | 不透明源，抛 `SecurityError` |
| `fetch` / XHR | CSP `connect-src 'none'` 拦截 |
| `invoke` / `__TAURI__` | 拿不到，只能走白名单 |
| 白名单能力 | `postMessage` 请求 → 父页面校验 → 执行 → 回传 |

白名单当前三项：`printVoucher`、`submitTransaction`、`log`。
每一项都仍经 Rust 侧校验，沙箱只解决"包能不能直接碰宿主"。

## 验证方式

在浏览器里直接注入一个 hostile 包，实测结果：

```
{"parentDom":"blocked:SecurityError","storage":"blocked:SecurityError",
 "cookie":"blocked:SecurityError","fetch":"blocked"}
```

不是"按设计应该拦"，是**跑出来拦住了**。

## 影响

- `apps/shell/src/platform/sandbox.ts`（新增）
- `apps/shell/src/components/PackageHost.vue`
- `check-boundaries`：主窗口里禁止 Blob URL + 动态 import（只能走沙箱）

## 风险与未覆盖

1. **同进程隔离**：iframe 与主壳仍在同一个 WebView 进程里，共享渲染进程内存。
   真正的进程隔离要 Tauri 多 webview 或独立进程，成本高，记为 P4。
2. **白名单能力可被滥用**：包可以循环调用 `printVoucher` 打空纸。
   限流属于后续，当前只保证"不许越界"。
3. **CSP 依赖浏览器的 srcdoc 行为**：已在目标 WebView（Chromium）实测，
   换 WebView 内核需要重新验证。

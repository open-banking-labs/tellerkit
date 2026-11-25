# 017 包调用身份的传递方式

## 现状

P0-7 要求把"谁发起的"带进执行路径，并在 Rust 侧按**包清单**裁决交易码与设备能力。
但 `contracts/ipc-commands.md` 里 `submit_transaction` / `print_voucher` / `package_module`
的入参没有包身份字段，`package_module` 只返回模块源码字符串。

契约是只读区，不能为实现就地加字段。

## 本次采用的过渡方式（不新增 IPC 字段）

1. `apps/shell/src-tauri/src/app.rs::package_module` 在模块源码**首行**写入由 Rust
   生成的注释：`// @tk-package-identity {"source":"pkg:<id>","code":"<menu code>"}`。
   返回类型仍是 `string`，契约不变；注释在包代码之前，包自己无法伪造这一行。
2. `apps/shell/src/platform/sandbox.ts::parsePackageIdentity` 只读取首行注释，
   把身份交给 `dispatchCall`；`apps/shell/src/components/PackageHost.vue` 把身份转录到
   既有参数 map 的保留键：`__pkg_source`、`__pkg_code`（`elements` / `fields`）。
3. Rust 侧 `package_module` 之外的执行路径先从 map 中**移除**这两个键，再校验：
   身份缺失按主壳普通调用处理；来源未知、身份残缺、格式非法一律给人类可读错误并审计。
   业务代码拿到的 map 与契约声明的字段保持一致。
4. 能力裁决在 `BizPackage::authorize_call`（packages/core）与 `ShellApp::authorize_package_call`
   （apps/shell/src-tauri）中完成；前端只传身份，不判断能否放行。

## 为什么这不是新增契约字段

- IPC 命令名、参数结构、返回类型都未变；
- 保留键借用既有 `Record<string, string>` 参数，不进入 Rust DTO / 生成类型；
- 不需要修改 `contracts/**`，也不需要 `npm run bindings`。

## 信任边界（必须随本实现一起理解）

Rust 侧校验的是「**声称的身份是已加载包，且该包清单声明了这项能力**」。
它**不**证明调用来自那个 iframe——当前 IPC 契约没有调用来源字段，命令层拿不到 frame 信息。
真正阻止业务包伪造身份的是下面两层，缺一不可：

1. Tauri 2 的 IPC bridge 只注入主框架（`for_main_frame_only`），iframe 内没有
   `window.__TAURI__` / `window.__TAURI_INTERNALS__`，因此包内 JS 无法直接 `invoke()`。
2. `apps/shell/src/platform/sandbox.ts` 的 iframe 是 `sandbox="allow-scripts"`，
   **没有 `allow-same-origin`**，是不透明源，也访问不到父窗口的 IPC 对象；
   CSP `connect-src 'none'` 进一步收口。包只能走 `postMessage` 白名单通道。
3. `apps/shell/src/components/PackageHost.vue` 用闭包身份**覆盖** postMessage 传来的同名字段，
   所以包无法经允许通道把自己的身份写成别的包。

结论：这是「沙箱保证调用来源 + Rust 按清单裁决声称身份」的组合控制。
**Rust 自身无法独立证明来源**；若 Tauri 注入策略变化、iframe 获得 same-origin，
或主壳出现可被调用方利用的 XSS，本控制即失效。该前提写明在案，供评审与回归检查。

## 后续建议

保留键毕竟是约定而非契约。若要把身份升级为一等字段、让 Rust 不依赖隐式沙箱前提，
需单独走契约变更：给 `submit_transaction` / `print_voucher` 增加可选的 `packageIdentity`
结构（由 Tauri 来源信息填充，不由前端自报），或让 `package_module` 返回带身份的结构体；
届时同步更新 `contracts/fields.json`、`contracts/ipc-commands.md` 与生成物，并移除本提案的过渡路径。

本次实现达成 P0-7 的**能力执行**（未声明的交易码/设备/离线能力在 Rust 侧被拒绝），
但不宣称解决「调用来源不可伪造」这一更深层目标；后者作为契约提案留待批准。

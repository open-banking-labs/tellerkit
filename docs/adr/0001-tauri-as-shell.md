# ADR-0001：桌面框架选 Tauri 2

- 状态：已接受
- 日期：2026-11-24
- 决策者：项目负责人

## 背景

柜面终端要替代"常开一个浏览器"的形态，核心痛点是 Chromium 常驻内存。
候选：Electron、Tauri 2、GPUI/GPUI Kit、原生（WinForms/WPF/Qt）。

## 决策

选择 **Tauri 2 + Rust 侧业务逻辑 + WebView 前端**。

## 理由

1. 包体积与常驻内存显著低于 Electron（Electron 把整个 Chromium 打进包）。
2. Rust 侧天然适合做：菜单包验签、文件原子替换、外设枚举、离线队列。
3. 前端沿用 Web 生态，单人 + AI 的产能最高；GPUI 需要自建组件库。
4. 业务逻辑放在纯 Rust crate 里，可脱离 GUI 用 `cargo test` 验证——这对单人项目是决定性的。

## 代价与缓解

| 代价 | 缓解 |
|---|---|
| 银行内网可能装不上 WebView2（Evergreen 需联网） | 安装器内嵌 Fixed Version 离线包，P2 前必须验证 |
| 前端与 Rust 两层契约需手工同步 | `contracts.ts` + serde 结构体，字段改动走同一 PR |
| Rust 学习曲线 | 业务逻辑集中在小 crate，AI 可生成样板 |

## 被否决的方案

- **Electron**：内存与包体积正是本项目要解决的问题。
- **GPUI / GPUI Kit**：pre-1.0、组件需自建、与 Zed 内部需求强绑定；迁移评估为 97k 行重写。见 ADR-0004。
- **原生 WinForms/WPF**：开发效率与跨平台性差，且无法复用 Web 生态。

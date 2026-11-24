# packages/core · 核心引擎

纯 Rust 库，**无 GUI、无 Tauri 依赖**，可脱离客户端 `cargo test`。

## 模块

| 模块 | 职责 |
|---|---|
| `model.rs` | 菜单契约、结构校验（重复/缺父/环/Action 当父） |
| `filter.rs` | `filter_visible`：唯一的权限剪枝规则 |
| `pack.rs` | `.mtree` 编解码 + Ed25519 签名/验签 |
| `store.rs` | 本地包仓库：原子替换、保留 N-1、坏包拒绝 |
| `sync.rs` | 启动回退、版本探测编排 |
| `perm.rs` | `PermSnapshot` |
| `mock.rs` | 演示用 mock 宿主：三角色、演示树、下发 |

## 为什么单独成包

柜面客户端最贵的不是界面，是"启动零等待 + 权限剪枝 + 包完整性"这三件事的正确性。
把它们做成无 GUI 依赖的库，才能在 CI 里用 53 个用例跑完，而不是靠人工点界面。

## 依赖

`serde` · `rmp-serde` · `ed25519-dalek` · `sha2` · `hex` · `thiserror`

```bash
cargo test -p tellerkit-core
```

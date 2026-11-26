//! 菜单能力域（MenuCatalog）：菜单契约、权限剪枝、签包编解码、本地存储、启动与同步。
//!
//! 物理上收在一个目录里，避免 `packages/core/src` 顶层过宽；对外仍由 `lib.rs`
//! re-export 成 `tellerkit_core::{model, filter, pack, store, sync}`，路径不变。

pub mod filter;
pub mod model;
pub mod pack;
pub mod store;
pub mod sync;

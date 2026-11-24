//! TellerKit 核心契约与纯逻辑。
//!
//! 本 crate 不依赖 Tauri，可独立 `cargo test`，便于在无 GUI 环境下验证
//! 菜单过滤、菜单包编解码、验签等关键路径。

pub mod filter;
pub mod mock;
pub mod model;
pub mod pack;
pub mod perm;
pub mod store;
pub mod sync;

pub use filter::MenuView;
pub use model::{MenuNode, MenuTree, NodeStatus, NodeType};
pub use pack::{MenuPack, PackError};
pub use perm::PermSnapshot;
pub use store::{LoadSource, LoadedMenu, MenuStore, StoreError};
pub use sync::{boot_load, check_and_update, reload, BootError, BootOutcome, UpdateOutcome};

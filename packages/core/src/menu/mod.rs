//! 菜单能力域（MenuCatalog）的深门面。
//!
//! 子模块一律 `pub(crate)`：外部只能通过这里 re-export 的类型与入口函数使用，
//! 不能直接摸到 `MenuStore`/`MenuPack` 等实现模块的完整接口。
//! 物理上收在一个目录里，也避免 `packages/core/src` 顶层过宽。

pub(crate) mod filter;
pub(crate) mod model;
pub(crate) mod pack;
pub(crate) mod store;
pub(crate) mod sync;

pub use self::filter::{filter_visible, MenuView};
pub use self::model::{MenuNode, MenuTree, NodeStatus, NodeType, TreeError};
pub use self::pack::{open, pubkey_from_hex, pubkey_to_hex, MenuPack, PackError, SIGNATURE_LEN};
pub use self::store::{LoadSource, LoadedMenu, MenuStore, StoreError};
pub use self::sync::{boot_load, check_and_update, reload, BootError, BootOutcome, UpdateOutcome};

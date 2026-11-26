//! TellerKit 核心契约与纯逻辑。
//!
//! 本 crate 不依赖 Tauri，可独立 `cargo test`，便于在无 GUI 环境下验证
//! 菜单过滤、菜单包编解码、验签等关键路径。

pub mod banking;
pub mod device;
mod menu;
pub mod mock;
pub mod perm;
pub mod pkg;
pub mod remote;
pub mod replay;
pub mod reversal;
pub mod session;
pub mod tx;
pub mod update;

pub use banking::{MenuVersionProbe, RemoteBanking, RemoteError, RoutedBanking, ServiceNames};
pub use device::{
    CardData, DeviceError, DeviceKind, DeviceStatus, Devices, PinBlock, PrintReceipt, Printer,
    Scanner, Voucher,
};
pub use menu::{
    boot_load, check_and_update, filter_visible, open, pubkey_from_hex, pubkey_to_hex, reload,
    BootError, BootOutcome, LoadSource, LoadedMenu, MenuNode, MenuPack, MenuStore, MenuTree,
    MenuView, NodeStatus, NodeType, PackError, StoreError, UpdateOutcome, SIGNATURE_LEN,
};
pub use perm::PermSnapshot;
pub use pkg::{BizManifest, BizPackage, LoadPlan, PackageDecision, PkgError, ShellRange, Version};
pub use remote::{
    Body, Codec, CodecError, Endpoint, EndpointAddr, FieldSpec, FixedWidthCodec, JsonCodec,
    Request, RouteError, Router, Transport, TransportError, WireMessage,
};
pub use replay::{ReplayAttempt, ReplayPolicy, ReplayScheduler};
pub use reversal::{
    Authorization, Authorizer, ExceptionKind, ReversalBook, ReversalError, ReversalRecord,
    ReversalRequest,
};
pub use session::{LockReason, SessionGuard, SessionPolicy};
pub use tx::{DemoTxHost, HostError, TxEngine, TxHost, TxHostMode, TxRequest, TxResult, TxStatus};
pub use update::{decide, rollout_bucket, Channel, UpdateAction, UpdateCandidate, UpdateDecision};

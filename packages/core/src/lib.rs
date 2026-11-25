//! TellerKit 核心契约与纯逻辑。
//!
//! 本 crate 不依赖 Tauri，可独立 `cargo test`，便于在无 GUI 环境下验证
//! 菜单过滤、菜单包编解码、验签等关键路径。

pub mod banking;
pub mod device;
pub mod filter;
pub mod mock;
pub mod model;
pub mod pack;
pub mod perm;
pub mod pkg;
pub mod recovery;
pub mod remote;
pub mod replay;
pub mod reversal;
pub mod session;
pub mod store;
pub mod sync;
pub mod tx;
pub mod update;

pub use banking::{MenuVersionProbe, RemoteBanking, RemoteError, RoutedBanking, ServiceNames};
pub use device::{
    CardData, DeviceError, DeviceKind, DeviceStatus, Devices, PinBlock, PrintReceipt, Printer,
    Scanner, Voucher,
};
pub use filter::MenuView;
pub use model::{MenuNode, MenuTree, NodeStatus, NodeType};
pub use pack::{MenuPack, PackError};
pub use perm::PermSnapshot;
pub use pkg::{BizManifest, BizPackage, LoadPlan, PackageDecision, PkgError, ShellRange, Version};
pub use recovery::{exception_kind_str, parse_exception_kind, RecoveryJournal};
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
pub use store::{LoadSource, LoadedMenu, MenuStore, StoreError};
pub use sync::{boot_load, check_and_update, reload, BootError, BootOutcome, UpdateOutcome};
pub use tx::{DemoTxHost, HostError, TxEngine, TxHost, TxHostMode, TxRequest, TxResult, TxStatus};
pub use update::{decide, rollout_bucket, Channel, UpdateAction, UpdateCandidate, UpdateDecision};

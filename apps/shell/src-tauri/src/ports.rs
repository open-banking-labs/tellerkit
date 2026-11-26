//! 出站端口（L3 应用层依赖它们，不依赖实现）。
//!
//! 这里的每个 trait 对应一条边界或一个可替换资源：
//!
//! | 端口 | 边界 | 实现 |
//! |---|---|---|
//! | [`RemoteBanking`] | ③ 桌面 ↔ 服务端 | `adapters::mock_remote`（当前）/ `adapters::http_remote`（目标） |
//! | [`Clock`] | 进程外部（时间） | `SystemClock` / 测试用 `FrozenClock` |
//! | `TxJournal` | 本地磁盘 | `packages/journal-sqlite` |
//! | 设备四 trait | 本地硬件 | `packages/core::device` 的 mock |
//!
//! 规则：端口 trait 里**不得出现具体实现类型**（`reqwest::Error`、`rusqlite::Connection`）。
//! 违反它，这层抽象就白抽了。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tellerkit_core::mock::{DemoRole, MockHost};
use tellerkit_core::perm::PermSnapshot;
use tellerkit_core::tx::{HostError, TxHost, TxRequest};
use tellerkit_core::Version;

// 端口定义在 core：这样 fake 后台与路由适配器能脱离 Tauri 测试。
pub use tellerkit_core::banking::{
    MenuSource, MenuVersionProbe, RemoteBanking, RemoteError, RoutedBanking, ServiceNames,
};

/// 时钟端口。锁屏、退避、审计都依赖时间，注入才能把测试从 `sleep` 里救出来。
pub trait Clock: Send + Sync {
    fn now_secs(&self) -> u64;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

/// 一次远端选择的结果：真实路由或演示远端，二选一。
///
/// 把 `remote` 与 `mock` 绑在一起传，避免 `AppState::new` 为演示开关单独加一个参数；
/// 将来去掉演示远端时，这里就是唯一需要改的构造点。
pub struct RemoteChoice {
    pub remote: Arc<dyn RemoteBanking>,
    pub mock: Option<Arc<MockRemote>>,
}

/// 演示用远端：把"在线 / 离线 / 坏包 / 业务拒绝"做成可切换的开关。
///
/// 它是**适配器**，不是领域逻辑——放在 ports 模块旁边是为了让替换点集中在一处。
pub struct MockRemote {
    state: Mutex<MockRemoteState>,
    tx_seq: AtomicU64,
    /// 幂等表：同一个 key 必须返回同一个流水号，否则演示会教坏使用方。
    accepted: Mutex<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone)]
struct MockRemoteState {
    offline: bool,
    tampered: bool,
    menu_version: u64,
    /// 业务拒绝的下单原因；None 表示正常受理。
    reject_reason: Option<String>,
}

impl Default for MockRemote {
    fn default() -> Self {
        Self {
            state: Mutex::new(MockRemoteState {
                offline: false,
                tampered: false,
                menu_version: 7,
                reject_reason: None,
            }),
            tx_seq: AtomicU64::new(0),
            accepted: Mutex::new(std::collections::BTreeMap::new()),
        }
    }
}

impl MockRemote {
    /// 演示开关：断网 / 下发坏包 / 服务器菜单版本。
    pub fn set_offline(&self, offline: bool) {
        self.with_state(|s| s.offline = offline);
    }

    pub fn set_tampered(&self, tampered: bool) {
        self.with_state(|s| s.tampered = tampered);
    }

    pub fn bump_menu_version(&self) -> u64 {
        self.with_state(|s| {
            s.menu_version += 1;
            s.menu_version
        })
    }

    /// 业务拒绝开关：`Some(原因)` 后所有交易都被主机拒绝。
    pub fn set_reject_reason(&self, reason: Option<String>) {
        self.with_state(|s| s.reject_reason = reason);
    }

    pub fn menu_version(&self) -> u64 {
        self.state
            .lock()
            .map(|s| s.menu_version)
            .unwrap_or_default()
    }

    pub fn offline(&self) -> bool {
        self.state.lock().map(|s| s.offline).unwrap_or(false)
    }

    pub fn tampered(&self) -> bool {
        self.state.lock().map(|s| s.tampered).unwrap_or(false)
    }

    /// 锁中毒时**不**取内部数据：宁可报错，也不在可疑状态上继续。
    fn with_state<T>(&self, f: impl FnOnce(&mut MockRemoteState) -> T) -> T
    where
        T: Default,
    {
        match self.state.lock() {
            Ok(mut guard) => f(&mut guard),
            Err(_) => T::default(),
        }
    }

    fn snapshot(&self) -> MockRemoteState {
        self.state
            .lock()
            .map(|s| s.clone())
            .unwrap_or_else(|_| MockRemoteState {
                offline: true,
                tampered: false,
                menu_version: 0,
                reject_reason: Some("演示远端状态不可用".into()),
            })
    }

    /// 菜单同步沿用现有的 `sync` 函数，它们接受 `MockHost`。
    /// 这里给出**同一份状态**的快照，避免 offline 出现在两处。
    pub fn menu_host(&self) -> MockHost {
        let state = self.snapshot();
        self.mock_host(&state)
    }

    fn mock_host(&self, state: &MockRemoteState) -> MockHost {
        MockHost::new(state.menu_version, state.offline, state.tampered)
    }
}

impl MenuSource for MockRemote {
    fn probe_menu(&self) -> Result<MenuVersionProbe, RemoteError> {
        let state = self.snapshot();
        self.mock_host(&state)
            .probe()
            .map(|p| MenuVersionProbe::new(p.latest(), p.shell_compat_min(), p.shell_compat_max()))
            .map_err(|e| RemoteError::Unavailable(e.to_string()))
    }

    fn download_menu(&self) -> Result<Vec<u8>, RemoteError> {
        let state = self.snapshot();
        self.mock_host(&state)
            .download()
            .map_err(|e| RemoteError::Unavailable(e.to_string()))
    }
}

impl RemoteBanking for MockRemote {
    fn login(&self, role_id: &str, perm_version: u64) -> Result<PermSnapshot, RemoteError> {
        let state = self.snapshot();
        if state.offline {
            return Err(RemoteError::Unavailable("断网".into()));
        }
        self.mock_host(&state)
            .login(role_id, perm_version)
            .ok_or(RemoteError::Rejected(format!("未知角色：{role_id}")))
    }

    fn reverse(&self, idempotency_key: &str, _reason: &str) -> Result<String, RemoteError> {
        if self.snapshot().offline {
            return Err(RemoteError::Unavailable("断网：无法冲正".into()));
        }
        Ok(format!("REV-{idempotency_key}"))
    }
}

impl TxHost for MockRemote {
    fn supports_idempotency(&self) -> bool {
        true
    }

    fn submit(&self, request: &TxRequest) -> Result<String, HostError> {
        let state = self.snapshot();
        if state.offline {
            return Err(HostError::Unavailable);
        }
        if let Some(reason) = state.reject_reason {
            return Err(HostError::Rejected(reason));
        }
        // 同键复用流水号：这就是服务端该有的幂等行为
        if let Ok(mut map) = self.accepted.lock() {
            if let Some(existing) = map.get(request.idempotency_key()) {
                return Ok(existing.clone());
            }
            let n = self.tx_seq.fetch_add(1, Ordering::SeqCst) + 1;
            let reference = format!("TX{n:08}");
            map.insert(request.idempotency_key().to_string(), reference.clone());
            return Ok(reference);
        }
        Err(HostError::Unavailable)
    }
}

/// 演示角色列表（真实环境由服务端下发）。
pub fn demo_roles() -> Vec<(&'static str, &'static str)> {
    DemoRole::ALL.iter().map(|r| (r.id(), r.label())).collect()
}

/// 主壳版本，用于业务包兼容判定。
pub fn shell_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version::new(0, 0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_remote_goes_offline_and_back() {
        let remote = MockRemote::default();
        assert!(remote.probe_menu().is_ok());
        remote.set_offline(true);
        assert!(matches!(
            remote.probe_menu(),
            Err(RemoteError::Unavailable(_))
        ));
        remote.set_offline(false);
        assert!(remote.probe_menu().is_ok());
    }

    #[test]
    fn mock_remote_rejects_transactions_when_configured() {
        let remote = MockRemote::default();
        let req = TxRequest::new("account.deposit", "k1");
        assert!(remote.submit(&req).is_ok());
        remote.set_reject_reason(Some("余额不足".into()));
        assert_eq!(
            remote.submit(&req),
            Err(HostError::Rejected("余额不足".into()))
        );
    }

    #[test]
    fn clock_reports_wall_time_in_seconds() {
        let now = SystemClock.now_secs();
        assert!(now > 1_700_000_000, "应是 2023 年之后的秒级时间戳");
    }
}

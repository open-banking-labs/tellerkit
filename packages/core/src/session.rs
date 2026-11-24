//! 会话与离岗锁屏（P3）。
//!
//! 柜员离开座位、屏幕还停着客户信息，是柜面最常见的信息泄露场景。
//! 锁屏本身不难，难的是**判定条件**：单看超时会漏掉"人走了但鼠标被碰到"，
//! 单看失焦会漏掉"窗口没失焦但人已经走了"。
//!
//! 所以这里把多个触发条件合到一个状态机里，并且**时钟由调用方注入**——
//! 纯逻辑才能在测试里精确控制时间流逝。

use serde::{Deserialize, Serialize};

/// 锁屏原因。要能回放到审计日志里，所以带具体类别而不是布尔。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockReason {
    /// 无操作超时。
    IdleTimeout,
    /// 窗口失去焦点（切走、最小化）。
    WindowBlur,
    /// 外设断开（打印机/读卡器被拔）。
    DeviceDisconnected,
    /// 柜员主动锁屏。
    Manual,
}

impl LockReason {
    pub fn label(self) -> &'static str {
        match self {
            LockReason::IdleTimeout => "无操作超时",
            LockReason::WindowBlur => "窗口失焦",
            LockReason::DeviceDisconnected => "外设断开",
            LockReason::Manual => "主动锁屏",
        }
    }
}

/// 锁屏策略。由网点配置下发，不写死在客户端。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPolicy {
    /// 无操作多少秒后锁定。0 表示不按超时锁。
    pub idle_timeout_secs: u64,
    /// 窗口失焦是否立即锁。
    ///
    /// 默认 true：柜面终端不应在柜员看不到的时候保持解锁。
    /// 但演示/调试时可以关掉，否则每次切窗口都要重新登录。
    pub lock_on_blur: bool,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 180,
            lock_on_blur: true,
        }
    }
}

/// 会话状态机。
#[derive(Debug, Clone)]
pub struct SessionGuard {
    policy: SessionPolicy,
    locked: bool,
    reason: Option<LockReason>,
    last_activity_secs: u64,
}

impl SessionGuard {
    pub fn new(policy: SessionPolicy, now_secs: u64) -> Self {
        unimplemented!()
    }

    pub fn policy(&self) -> &SessionPolicy {
        &self.policy
    }

    pub fn is_locked(&self) -> bool {
        unimplemented!()
    }

    pub fn reason(&self) -> Option<LockReason> {
        unimplemented!()
    }

    /// 记录一次操作（鼠标、键盘、交易提交）。锁定状态下不恢复会话。
    pub fn touch(&mut self, now_secs: u64) {
        unimplemented!()
    }

    /// 距离上次操作的秒数。时钟倒退时返回 0，不 panic。
    pub fn idle_secs(&self, now_secs: u64) -> u64 {
        unimplemented!()
    }

    /// 周期性评估：返回本次是否**新**进入锁定。
    ///
    /// 已经锁着再评估应返回 None——上层拿它决定"要不要弹锁屏"，
    /// 每秒钟重复弹一次是 bug。
    pub fn evaluate(&mut self, now_secs: u64) -> Option<LockReason> {
        unimplemented!()
    }

    /// 窗口失焦。返回本次是否新锁定。
    pub fn on_blur(&mut self, _now_secs: u64) -> Option<LockReason> {
        unimplemented!()
    }

    /// 外设断开。**不受策略影响**：设备断了就必须锁，这不是可配置项。
    pub fn on_device_lost(&mut self, _now_secs: u64) -> Option<LockReason> {
        unimplemented!()
    }

    /// 主动锁屏。
    pub fn lock(&mut self, reason: LockReason) -> Option<LockReason> {
        unimplemented!()
    }

    /// 解锁。调用方必须先验证身份（密码/指纹），这里只负责状态。
    pub fn unlock(&mut self, now_secs: u64) {
        unimplemented!()
    }
}

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
        Self {
            policy,
            locked: false,
            reason: None,
            last_activity_secs: now_secs,
        }
    }

    pub fn policy(&self) -> &SessionPolicy {
        &self.policy
    }

    /// 网点下发新策略。
    ///
    /// 只换策略，**不动锁定状态与计时**——重建 guard 会把已锁的会话解锁，
    /// 那是安全问题，不是配置更新。
    pub fn set_policy(&mut self, policy: SessionPolicy) {
        self.policy = policy;
    }

    pub fn is_locked(&self) -> bool {
        self.locked
    }

    pub fn reason(&self) -> Option<LockReason> {
        self.reason
    }

    /// 记录一次操作（鼠标、键盘、交易提交）。锁定状态下不恢复会话。
    pub fn touch(&mut self, now_secs: u64) {
        // 关键：锁定期间不刷新 last_activity。否则锁屏界面上鼠标划过
        // （甚至柜员把鼠标碰下桌）就等价于"人在"，超时永远不到期。
        if !self.locked {
            self.last_activity_secs = now_secs;
        }
    }

    /// 距离上次操作的秒数。时钟倒退时返回 0，不 panic。
    pub fn idle_secs(&self, now_secs: u64) -> u64 {
        // 调用方注入的时间戳可能来自墙钟（NTP 校正、夏令时、用户改表），
        // 倒退时 saturating_sub 落到 0，而不是 debug 下 panic / release 下回绕成天文数字。
        now_secs.saturating_sub(self.last_activity_secs)
    }

    /// 周期性评估：返回本次是否**新**进入锁定。
    ///
    /// 已经锁着再评估应返回 None——上层拿它决定"要不要弹锁屏"，
    /// 每秒钟重复弹一次是 bug。
    pub fn evaluate(&mut self, now_secs: u64) -> Option<LockReason> {
        if self.locked {
            return None;
        }
        // 0 表示"不按超时锁"，不是"立刻锁"。
        let timeout = self.policy.idle_timeout_secs;
        if timeout > 0 && self.idle_secs(now_secs) >= timeout {
            return self.lock(LockReason::IdleTimeout);
        }
        None
    }

    /// 窗口失焦。返回本次是否新锁定。
    pub fn on_blur(&mut self, _now_secs: u64) -> Option<LockReason> {
        // 策略关掉时什么都不做：不锁、不改 last_activity、不改 reason。
        if !self.policy.lock_on_blur {
            return None;
        }
        self.lock(LockReason::WindowBlur)
    }

    /// 外设断开。**不受策略影响**：设备断了就必须锁，这不是可配置项。
    pub fn on_device_lost(&mut self, _now_secs: u64) -> Option<LockReason> {
        // 不看 policy：读卡器/打印机被拔，说明物理现场已不受控，
        // 即使 lock_on_blur = false、idle_timeout_secs = 0 也必须锁。
        self.lock(LockReason::DeviceDisconnected)
    }

    /// 主动锁屏。
    pub fn lock(&mut self, reason: LockReason) -> Option<LockReason> {
        // 已锁定时保留**第一次**的原因：先超时锁、再拔设备，
        // 审计里应该看到"为什么第一次锁"，而不是被后来的触发覆盖。
        if self.locked {
            return None;
        }
        self.locked = true;
        self.reason = Some(reason);
        Some(reason)
    }

    /// 解锁。调用方必须先验证身份（密码/指纹），这里只负责状态。
    pub fn unlock(&mut self, now_secs: u64) {
        self.locked = false;
        self.reason = None;
        // 解锁即重新计时：否则离开前那段空闲会立刻把刚解锁的会话再锁一次。
        self.last_activity_secs = now_secs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(idle_timeout_secs: u64, lock_on_blur: bool) -> SessionPolicy {
        SessionPolicy {
            idle_timeout_secs,
            lock_on_blur,
        }
    }

    // S-01：无操作超时锁定。t=179 不锁，t=180 锁，原因是 IdleTimeout。
    #[test]
    fn idle_timeout_locks_at_threshold_only() {
        let mut guard = SessionGuard::new(policy(180, true), 0);

        assert_eq!(guard.evaluate(179), None);
        assert!(!guard.is_locked());

        assert_eq!(guard.evaluate(180), Some(LockReason::IdleTimeout));
        assert!(guard.is_locked());
        assert_eq!(guard.reason(), Some(LockReason::IdleTimeout));
    }

    // S-02：已锁定再评估返回 None，不重复弹锁屏。
    #[test]
    fn repeated_evaluate_does_not_report_again() {
        let mut guard = SessionGuard::new(policy(180, true), 0);
        assert_eq!(guard.evaluate(180), Some(LockReason::IdleTimeout));

        // 同一时刻之后每秒评估都不再上报。
        for now in [181, 240, 10_000] {
            assert_eq!(guard.evaluate(now), None, "t={now} 不应重复上报");
            assert!(guard.is_locked());
        }
        // 也不允许被后来的触发换成别的原因。
        assert_eq!(guard.reason(), Some(LockReason::IdleTimeout));
        assert_eq!(guard.lock(LockReason::Manual), None);
        assert_eq!(guard.reason(), Some(LockReason::IdleTimeout));
    }

    // S-03：锁定状态下 touch 不续命；unlock 后从解锁时刻重新计时。
    #[test]
    fn touch_while_locked_does_not_extend_and_unlock_restarts_timer() {
        let mut guard = SessionGuard::new(policy(180, true), 0);
        assert_eq!(guard.lock(LockReason::Manual), Some(LockReason::Manual));

        // 锁定期间"有人碰鼠标"：既不解锁，也不刷新 last_activity。
        guard.touch(150);
        assert!(guard.is_locked());
        assert_eq!(guard.idle_secs(150), 150);

        // 解锁时刻即新的计时起点。
        guard.unlock(200);
        assert!(!guard.is_locked());
        assert_eq!(guard.reason(), None);
        assert_eq!(guard.idle_secs(200), 0);

        // 若沿用旧起点，t=380 早就该锁了；重新计时后 t=379 仍解锁。
        assert_eq!(guard.evaluate(379), None);
        assert!(!guard.is_locked());
        assert_eq!(guard.evaluate(380), Some(LockReason::IdleTimeout));
    }

    // S-04：失焦按策略决定；策略关闭时不改变任何状态。
    #[test]
    fn blur_follows_policy_and_is_noop_when_disabled() {
        let mut locking = SessionGuard::new(policy(180, true), 0);
        assert_eq!(locking.on_blur(10), Some(LockReason::WindowBlur));
        assert!(locking.is_locked());

        let mut lenient = SessionGuard::new(policy(180, false), 0);
        assert_eq!(lenient.on_blur(10), None);
        assert!(!lenient.is_locked());
        assert_eq!(lenient.reason(), None);
        // 失焦不该顺手刷新活动时间：否则切窗口等于"人在"。
        assert_eq!(lenient.idle_secs(10), 10);

        // 同一时刻显式锁屏仍然有效。
        assert_eq!(lenient.lock(LockReason::Manual), Some(LockReason::Manual));
        assert!(lenient.is_locked());
    }

    // S-05：策略全关时外设断开仍无条件锁定。
    #[test]
    fn device_lost_locks_even_when_all_policies_are_off() {
        let mut guard = SessionGuard::new(policy(0, false), 0);

        // 策略全关：超时和失焦都不该锁。
        assert_eq!(guard.evaluate(99_999), None);
        assert_eq!(guard.on_blur(99_999), None);
        assert!(!guard.is_locked());

        assert_eq!(
            guard.on_device_lost(99_999),
            Some(LockReason::DeviceDisconnected)
        );
        assert!(guard.is_locked());
        assert_eq!(guard.reason(), Some(LockReason::DeviceDisconnected));
        // 已经锁着时再拔设备不重复上报。
        assert_eq!(guard.on_device_lost(100_000), None);
    }

    // S-06：时钟倒退时空闲秒数为 0，且评估不 panic、不误锁。
    #[test]
    fn clock_rollback_clamps_idle_to_zero() {
        let mut guard = SessionGuard::new(policy(180, true), 0);
        guard.touch(1_000);
        assert_eq!(guard.idle_secs(1_000), 0);

        // 墙钟倒退 100 秒：空闲按 0 算，不退回、不回绕、不误锁。
        assert_eq!(guard.idle_secs(900), 0);
        assert_eq!(guard.evaluate(900), None);
        assert!(!guard.is_locked());

        // 计时基准仍是 t=1000 那次操作（倒退不当作"重新活动"），
        // 所以真要到期得等墙钟重新走到 1000+180。
        assert_eq!(guard.idle_secs(1_179), 179);
        assert_eq!(guard.evaluate(1_179), None);
        assert_eq!(guard.evaluate(1_180), Some(LockReason::IdleTimeout));
    }

    // 超时配置 0 = 不按超时锁，而不是立即锁。
    #[test]
    fn zero_idle_timeout_disables_idle_lock() {
        let mut guard = SessionGuard::new(policy(0, true), 0);
        assert_eq!(guard.evaluate(u64::MAX), None);
        assert!(!guard.is_locked());
    }

    // 未锁定时 touch 正常刷新计时起点。
    #[test]
    fn touch_while_unlocked_restarts_idle_timer() {
        let mut guard = SessionGuard::new(policy(180, false), 0);
        guard.touch(100);

        assert_eq!(guard.idle_secs(100), 0);
        assert_eq!(guard.evaluate(279), None);
        assert_eq!(guard.evaluate(280), Some(LockReason::IdleTimeout));
    }

    // 新会话的初始状态：未锁、无原因、计时从构造时刻开始。
    #[test]
    fn new_session_starts_unlocked_at_given_time() {
        let guard = SessionGuard::new(policy(180, true), 500);

        assert!(!guard.is_locked());
        assert_eq!(guard.reason(), None);
        assert_eq!(guard.idle_secs(500), 0);
        assert_eq!(guard.idle_secs(501), 1);
        assert_eq!(guard.policy().idle_timeout_secs, 180);
        assert!(guard.policy().lock_on_blur);
    }

    // 未锁定时 unlock 也应把计时重置到 now（上层可能无条件调用）。
    #[test]
    fn unlock_when_already_unlocked_resets_timer() {
        let mut guard = SessionGuard::new(policy(180, true), 0);
        guard.touch(120);
        guard.unlock(300);

        assert!(!guard.is_locked());
        assert_eq!(guard.idle_secs(300), 0);
    }
}

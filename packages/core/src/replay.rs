//! 自动重放调度（P3）。
//!
//! 断网时挂起的交易恢复后要重放。但"恢复就全部重放"是危险的：
//! 主机刚恢复时最脆弱，一股脑推过去容易把主机打垮，也容易在
//! 部分成功的情况下制造难查的半批状态。
//!
//! 所以这里是**调度策略**而不是执行器：回答"现在该重放哪几笔"、
//! "失败几次算放弃"、"下次什么时候再试"。执行仍由 `TxEngine::replay_pending` 做。

use std::collections::HashMap;

/// 重放策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayPolicy {
    /// 单笔的**总失败预算**（含第一次尝试）。达到后不再自动重放，交给人工。
    pub max_attempts: u32,
    /// 首次退避秒数，之后指数增长。
    pub base_backoff_secs: u64,
    /// 退避上限，避免退到天荒地老。
    pub max_backoff_secs: u64,
}

impl Default for ReplayPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            base_backoff_secs: 30,
            max_backoff_secs: 600,
        }
    }
}

/// 单笔交易的重放进度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayAttempt {
    pub key: String,
    pub attempts: u32,
    pub next_at_secs: u64,
}

/// 重放调度器。纯逻辑：时间由调用方传入。
#[derive(Debug, Default)]
pub struct ReplayScheduler {
    policy: ReplayPolicy,
    attempts: HashMap<String, ReplayAttempt>,
}

impl ReplayScheduler {
    pub fn new(policy: ReplayPolicy) -> Self {
        Self {
            policy,
            attempts: HashMap::new(),
        }
    }

    pub fn policy(&self) -> &ReplayPolicy {
        &self.policy
    }

    /// 某一刻应当重放的交易，按"最早该重放的在前"排序。
    ///
    /// 同一时刻到期的多笔按幂等键排序，保证顺序可复现。
    pub fn due(&self, now_secs: u64) -> Vec<String> {
        let mut due: Vec<&ReplayAttempt> = self
            .attempts
            .values()
            // 已达上限的留在表里等人工，但绝不再自动重放。
            .filter(|a| a.next_at_secs <= now_secs && a.attempts < self.policy.max_attempts)
            .collect();
        // HashMap 遍历顺序不稳定，必须显式排序：先按到期时刻，再按幂等键。
        due.sort_by(|a, b| {
            a.next_at_secs
                .cmp(&b.next_at_secs)
                .then_with(|| a.key.cmp(&b.key))
        });
        due.into_iter().map(|a| a.key.clone()).collect()
    }

    /// 确认一笔交易在管。已存在则不动（**不重置退避**），新登记立即可试。
    ///
    /// 名字不是 "update"：这里不会刷新任何进度，只保证它在调度表里。
    pub fn track(&mut self, key: &str, now_secs: u64) {
        // 已在管则不重置退避：重复登记不能把已经等到的下一次重放又推回"立即"。
        self.attempts
            .entry(key.to_string())
            .or_insert_with(|| ReplayAttempt {
                key: key.to_string(),
                attempts: 0,
                next_at_secs: now_secs,
            });
    }

    /// 重放失败：累加次数并按退避推迟下一次。返回累计尝试次数。
    pub fn record_failure(&mut self, key: &str, now_secs: u64) -> u32 {
        // 容错：允许"先失败后登记"，从 0 起算第一次失败。
        let attempts = {
            let attempt = self
                .attempts
                .entry(key.to_string())
                .or_insert_with(|| ReplayAttempt {
                    key: key.to_string(),
                    attempts: 0,
                    next_at_secs: now_secs,
                });
            attempt.attempts = attempt.attempts.saturating_add(1);
            attempt.attempts
        };
        // 先算退避再可变借用，避免同时借用 self 与 self.attempts。
        let next_at_secs = now_secs.saturating_add(self.backoff_for(attempts));
        self.attempts
            .get_mut(key)
            .expect("record_failure 刚插入的条目必然存在")
            .next_at_secs = next_at_secs;
        attempts
    }

    /// 重放成功：从调度器移除。
    ///
    /// 未登记也安全（幂等）。
    pub fn record_success(&mut self, key: &str) {
        self.attempts.remove(key);
    }

    /// 是否已放弃自动重放（达到上限），需要人工介入。
    pub fn exhausted(&self, key: &str) -> bool {
        // 未登记不等于已放弃；只有累计次数真正达到上限才算。
        match self.attempts.get(key) {
            Some(attempt) => attempt.attempts >= self.policy.max_attempts,
            None => false,
        }
    }

    /// 第 `attempts` 次失败后的退避秒数（指数增长，受上限约束）。
    pub fn backoff_for(&self, attempts: u32) -> u64 {
        if attempts == 0 {
            return 0;
        }
        // 2^(attempts-1)：attempts 很大时 checked_shl 返回 None，直接用 u64::MAX
        // 参与饱和乘法，保证第 10 次、第 u32::MAX 次都不会 panic 或回绕。
        let factor = 1u64.checked_shl(attempts - 1).unwrap_or(u64::MAX);
        self.policy
            .base_backoff_secs
            .saturating_mul(factor)
            .min(self.policy.max_backoff_secs)
    }

    /// 人工处置完毕后释放：移出调度表以便重新自动重放。
    ///
    /// 与 `record_success` 的区别是语义：那个是"真的成功了"，
    /// 这个是"人已经处理过，重新开始计数"。混用会让审计里出现假成功。
    pub fn release(&mut self, key: &str) {
        self.attempts.remove(key);
    }

    /// 当前在管的交易数（含已达上限、等人工的）。
    pub fn tracked(&self) -> usize {
        self.attempts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scheduler(max_attempts: u32) -> ReplayScheduler {
        ReplayScheduler::new(ReplayPolicy {
            max_attempts,
            base_backoff_secs: 30,
            max_backoff_secs: 600,
        })
    }

    fn keys(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|key| (*key).to_string()).collect()
    }

    // S-01
    #[test]
    fn s01_new_track_is_due_immediately() {
        let mut s = scheduler(5);
        s.track("K-001", 100);

        assert!(s.due(99).is_empty(), "到点之前不应出现");
        assert_eq!(s.due(100), keys(&["K-001"]));
        assert_eq!(s.tracked(), 1);
        assert!(!s.exhausted("K-001"));
    }

    // S-02：逐次断言 30 / 60 / 120，而不是"变大了"。
    #[test]
    fn s02_failure_backs_off_exponentially() {
        let mut s = scheduler(5);
        s.track("K-001", 100);

        assert_eq!(s.record_failure("K-001", 100), 1);
        assert!(s.due(129).is_empty(), "第 1 次失败后 29 秒仍不该重放");
        assert_eq!(s.due(130), keys(&["K-001"]), "第 1 次失败后退避 30 秒");

        assert_eq!(s.record_failure("K-001", 130), 2);
        assert!(s.due(189).is_empty(), "第 2 次失败后 59 秒仍不该重放");
        assert_eq!(s.due(190), keys(&["K-001"]), "第 2 次失败后退避 60 秒");

        assert_eq!(s.record_failure("K-001", 190), 3);
        assert!(s.due(309).is_empty(), "第 3 次失败后 119 秒仍不该重放");
        assert_eq!(s.due(310), keys(&["K-001"]), "第 3 次失败后退避 120 秒");
    }

    // S-03：第 10 次必须是上限 600，且不能 panic。
    #[test]
    fn s03_backoff_is_capped_at_max() {
        let s = scheduler(5);

        assert_eq!(s.backoff_for(0), 0);
        assert_eq!(s.backoff_for(1), 30);
        assert_eq!(s.backoff_for(2), 60);
        assert_eq!(s.backoff_for(3), 120);
        assert_eq!(s.backoff_for(5), 480);
        assert_eq!(s.backoff_for(10), 600, "第 10 次失败封顶 600 秒且不 panic");
        assert_eq!(s.backoff_for(u32::MAX), 600, "极端次数也不 panic");
    }

    // S-04：达到上限后不在 due，但仍在 tracked。
    #[test]
    fn s04_exhausted_stops_automatic_replay() {
        let mut s = scheduler(3);
        s.track("K-001", 0);

        assert_eq!(s.record_failure("K-001", 0), 1);
        assert_eq!(s.record_failure("K-001", 30), 2);
        assert!(!s.exhausted("K-001"));

        assert_eq!(s.record_failure("K-001", 90), 3);
        assert!(s.exhausted("K-001"));
        assert!(s.due(u64::MAX).is_empty(), "达到上限后不再出现在 due");
        assert_eq!(s.tracked(), 1, "仍在管，等人工处理");
    }

    // S-05：成功即移除，tracked 减一。
    #[test]
    fn s05_success_removes_entry() {
        let mut s = scheduler(5);
        s.track("K-001", 100);
        s.track("K-002", 100);
        assert_eq!(s.tracked(), 2);

        s.record_success("K-001");
        assert_eq!(s.tracked(), 1);
        assert_eq!(s.due(100), keys(&["K-002"]));

        s.record_success("K-404"); // 未登记的成功也容错，不 panic
        assert_eq!(s.tracked(), 1);
    }

    // S-06：同一时刻按幂等键排序，多次调用一致。
    #[test]
    fn s06_due_order_is_reproducible() {
        let mut s = scheduler(5);
        for key in ["K-003", "K-001", "K-002"] {
            s.track(key, 100);
        }

        let expected = keys(&["K-001", "K-002", "K-003"]);
        assert_eq!(s.due(100), expected, "同一时刻按幂等键排序");
        for _ in 0..5 {
            assert_eq!(s.due(100), expected, "重复调用顺序一致");
        }
    }

    #[test]
    fn due_sorts_by_time_then_key() {
        let mut s = scheduler(5);
        s.track("K-002", 100);
        s.track("K-001", 200);

        assert_eq!(s.due(199), keys(&["K-002"]));
        assert_eq!(s.due(200), keys(&["K-002", "K-001"]));

        assert_eq!(s.record_failure("K-002", 200), 1);
        assert_eq!(s.due(200), keys(&["K-001"]));
        assert_eq!(s.due(230), keys(&["K-001", "K-002"]));
    }

    #[test]
    fn tracking_again_does_not_reset_backoff() {
        let mut s = scheduler(5);
        s.track("K-001", 100);
        s.record_failure("K-001", 100);

        s.track("K-001", 110); // 已在管：不重置 attempts / next_at_secs
        assert!(s.due(129).is_empty());
        assert_eq!(s.due(130), keys(&["K-001"]));
        assert_eq!(s.record_failure("K-001", 130), 2);
    }

    #[test]
    fn failure_before_track_is_tolerated() {
        let mut s = scheduler(5);

        assert_eq!(s.record_failure("K-404", 100), 1);
        assert_eq!(s.tracked(), 1);
        assert!(s.due(129).is_empty());
        assert_eq!(s.due(130), keys(&["K-404"]));
        assert!(!s.exhausted("K-404"));
    }

    #[test]
    fn retrack_after_success_starts_fresh() {
        let mut s = scheduler(5);
        s.track("K-001", 100);
        s.record_failure("K-001", 100);
        s.record_success("K-001");

        s.track("K-001", 500);
        assert_eq!(s.due(500), keys(&["K-001"]));
        assert_eq!(s.record_failure("K-001", 500), 1, "成功后重新登记从 0 起算");
    }

    #[test]
    fn untracked_key_is_not_exhausted() {
        let s = scheduler(5);
        assert!(!s.exhausted("K-404"));
        assert_eq!(s.tracked(), 0);
    }

    #[test]
    fn policy_is_kept_by_constructor() {
        let policy = ReplayPolicy {
            max_attempts: 7,
            base_backoff_secs: 5,
            max_backoff_secs: 40,
        };
        let s = ReplayScheduler::new(policy.clone());
        assert_eq!(s.policy(), &policy);
    }
}

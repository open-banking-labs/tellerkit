//! 自动重放调度（P3）。
//!
//! 断网时挂起的交易恢复后要重放。但"恢复就全部重放"是危险的：
//! 主机刚恢复时最脆弱，一股脑推过去容易把主机打垮，也容易在
//! 部分成功的情况下制造难查的半批状态。
//!
//! 所以这里是**调度策略**而不是执行器：回答"现在该重放哪几笔"、
//! "失败几次算放弃"、"下次什么时候再试"。执行仍由 `TxEngine::replay_pending` 做。

use crate::tx::{TxEngine, TxHost, TxStatus};
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
        // 用模式匹配而不是 expect：让编译器证明条目存在，而不是靠注释解释
        if let Some(entry) = self.attempts.get_mut(key) {
            entry.next_at_secs = next_at_secs;
        }
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

    /// 从持久化账本恢复进度。attempts 与下次重试时间都原样读回，
    /// 否则反复重启就能把失败预算清零。
    pub fn restore(&mut self, attempts: Vec<ReplayAttempt>) {
        self.attempts.clear();
        for attempt in attempts {
            self.attempts.insert(attempt.key.clone(), attempt);
        }
    }

    /// 写回用的快照。调用方逐条 `save_attempt`，删条目时调 `remove_attempt`。
    pub fn snapshot(&self) -> Vec<ReplayAttempt> {
        let mut out: Vec<ReplayAttempt> = self.attempts.values().cloned().collect();
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
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

/// 一轮自动重放的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayRun {
    /// 本轮尝试的笔数。
    pub attempted: usize,
    /// 成功提交的笔数。
    pub committed: usize,
    /// 队列里仍挂着的笔数（含本轮失败与未到期的）。
    pub still_pending: usize,
    /// 本轮达到重试上限、交给人工的幂等键。
    pub gave_up: Vec<String>,
}

/// 跑一轮自动重放。
///
/// 顺序：先把引擎里的挂起交易纳入调度（重启后第一次运行时调度表是空的），
/// 再取到期的逐笔重放，最后按结果更新退避。
///
/// 为什么逐笔而不是整批：整批失败时无法区分"哪笔该退避"，
/// 而且主机刚恢复时一批推过去更容易打垮它。
pub fn run_once(
    engine: &mut TxEngine,
    scheduler: &mut ReplayScheduler,
    host: &dyn TxHost,
    now_secs: u64,
) -> ReplayRun {
    // 1. 接管引擎里尚未登记的挂起交易
    let pending_keys: Vec<String> = engine
        .pending_requests()
        .iter()
        .map(|r| r.idempotency_key.clone())
        .collect();
    for key in &pending_keys {
        scheduler.track(key, now_secs);
    }

    // 2. 到期的逐笔重放
    let due = scheduler.due(now_secs);
    let mut committed = 0;
    let mut gave_up = Vec::new();

    for key in &due {
        match engine.replay_one(host, key) {
            Some((_, result)) => {
                if result.status == TxStatus::Committed {
                    committed += 1;
                    scheduler.record_success(key);
                } else {
                    scheduler.record_failure(key, now_secs);
                    if scheduler.exhausted(key) {
                        gave_up.push(key.clone());
                    }
                }
            }
            // 队列里没有这笔（可能被手动重放掉了）：释放调度名额
            None => scheduler.record_success(key),
        }
    }

    ReplayRun {
        attempted: due.len(),
        committed,
        still_pending: engine.pending_requests().len(),
        gave_up,
    }
}

#[cfg(test)]
mod coordinator_tests {
    use super::*;
    use crate::tx::{HostError, TxEngine, TxHost, TxRequest, TxStatus};
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    /// 按脚本依次返回结果，并记录调用次数。
    struct ScriptedHost {
        script: RefCell<VecDeque<Result<String, HostError>>>,
        calls: Cell<u32>,
    }

    impl ScriptedHost {
        fn new(script: Vec<Result<String, HostError>>) -> Self {
            Self {
                script: RefCell::new(script.into()),
                calls: Cell::new(0),
            }
        }
        fn calls(&self) -> u32 {
            self.calls.get()
        }
    }

    impl TxHost for ScriptedHost {
        fn submit(&self, _r: &TxRequest) -> Result<String, HostError> {
            self.calls.set(self.calls.get() + 1);
            self.script
                .borrow_mut()
                .pop_front()
                .unwrap_or(Err(HostError::Unavailable))
        }
    }

    fn req(key: &str) -> TxRequest {
        TxRequest::new("account.deposit", key)
            .amount(10_000)
            .offline(true)
    }

    /// 造一个已经挂着 N 笔交易的引擎：断网提交 → Pending。
    fn engine_with_pending(keys: &[&str]) -> TxEngine {
        let offline = ScriptedHost::new(vec![]);
        let mut engine = TxEngine::new();
        for key in keys {
            let result = engine.submit(&offline, req(key));
            assert_eq!(result.status, TxStatus::Pending);
        }
        engine
    }

    #[test]
    fn first_run_adopts_pending_and_replays_when_host_is_back() {
        let mut engine = engine_with_pending(&["k1", "k2"]);
        let mut scheduler = ReplayScheduler::new(ReplayPolicy::default());
        let host = ScriptedHost::new(vec![Ok("TX-1".into()), Ok("TX-2".into())]);

        let run = run_once(&mut engine, &mut scheduler, &host, 100);
        assert_eq!(run.attempted, 2);
        assert_eq!(run.committed, 2);
        assert_eq!(run.still_pending, 0);
        assert!(run.gave_up.is_empty());
        assert_eq!(host.calls(), 2);
        assert_eq!(scheduler.tracked(), 0, "成功后应移出调度");
    }

    #[test]
    fn unreachable_host_keeps_transaction_and_backs_off() {
        let mut engine = engine_with_pending(&["k1"]);
        let mut scheduler = ReplayScheduler::new(ReplayPolicy::default());
        let host = ScriptedHost::new(vec![Err(HostError::Unavailable)]);

        let run = run_once(&mut engine, &mut scheduler, &host, 100);
        assert_eq!(run.attempted, 1);
        assert_eq!(run.committed, 0);
        assert_eq!(run.still_pending, 1, "不可达不得丢单");

        // 退避期内不再重试
        let run2 = run_once(&mut engine, &mut scheduler, &host, 110);
        assert_eq!(run2.attempted, 0, "未到期不应重试");
        assert_eq!(host.calls(), 1);
    }

    #[test]
    fn gives_up_after_max_attempts_and_stops_retrying() {
        let mut engine = engine_with_pending(&["k1"]);
        let policy = ReplayPolicy {
            max_attempts: 2,
            base_backoff_secs: 10,
            max_backoff_secs: 10,
        };
        let mut scheduler = ReplayScheduler::new(policy);
        let host = ScriptedHost::new(vec![
            Err(HostError::Unavailable),
            Err(HostError::Unavailable),
        ]);

        let first = run_once(&mut engine, &mut scheduler, &host, 100);
        assert!(first.gave_up.is_empty());
        let second = run_once(&mut engine, &mut scheduler, &host, 110);
        assert_eq!(second.gave_up, vec!["k1".to_string()]);
        assert_eq!(engine.pending_requests().len(), 1, "放弃不等于丢弃");

        let third = run_once(&mut engine, &mut scheduler, &host, 999);
        assert_eq!(third.attempted, 0, "放弃后不再自动重试");
        assert_eq!(host.calls(), 2);
    }

    #[test]
    fn manual_replay_removes_entry_from_scheduler() {
        let mut engine = engine_with_pending(&["k1"]);
        let mut scheduler = ReplayScheduler::new(ReplayPolicy::default());
        let online = ScriptedHost::new(vec![Ok("TX-MANUAL".into())]);

        // 上一轮自动重放已经把它纳入调度（否则下面的 due 是空的，测不到接管分支）
        scheduler.track("k1", 100);

        // 手动重放掉这笔（模拟用户点了按钮）
        assert!(engine.replay_pending(&online).len() == 1);
        assert_eq!(engine.pending_requests().len(), 0);

        // 自动重放接管时发现队列已空，应释放调度名额
        let run = run_once(&mut engine, &mut scheduler, &online, 200);
        assert_eq!(run.attempted, 1);
        assert_eq!(run.committed, 0);
        assert_eq!(scheduler.tracked(), 0);
    }
}

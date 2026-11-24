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
    /// 单笔最多重试几次。达到后不再自动重放，交给人工。
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
        unimplemented!()
    }

    pub fn policy(&self) -> &ReplayPolicy {
        &self.policy
    }

    /// 某一刻应当重放的交易，按"最早该重放的在前"排序。
    ///
    /// 同一时刻到期的多笔按幂等键排序，保证顺序可复现。
    pub fn due(&self, now_secs: u64) -> Vec<String> {
        unimplemented!()
    }

    /// 登记/更新一笔待重放交易。首次登记时 `next_at_secs` 取当前时间（立即可试）。
    pub fn track(&mut self, key: &str, now_secs: u64) {
        unimplemented!()
    }

    /// 重放失败：累加次数并按退避推迟下一次。返回累计尝试次数。
    pub fn record_failure(&mut self, key: &str, now_secs: u64) -> u32 {
        unimplemented!()
    }

    /// 重放成功：从调度器移除。
    pub fn record_success(&mut self, key: &str) {
        unimplemented!()
    }

    /// 是否已放弃自动重放（达到上限），需要人工介入。
    pub fn exhausted(&self, key: &str) -> bool {
        unimplemented!()
    }

    /// 第 `attempts` 次失败后的退避秒数（指数增长，受上限约束）。
    pub fn backoff_for(&self, attempts: u32) -> u64 {
        unimplemented!()
    }

    /// 当前在管的交易数（含已达上限、等人工的）。
    pub fn tracked(&self) -> usize {
        unimplemented!()
    }
}

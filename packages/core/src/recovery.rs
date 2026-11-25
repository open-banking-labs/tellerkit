//! 重启恢复所需的状态端口。
//!
//! 幂等索引由 `TxJournal` 负责（它本来就是交易账本的一部分）；
//! 这里收口另外三类"进程重启后必须还在"的状态：异常/冲正清单、重放进度、
//! 以及原交易要素（冲正必须带原交易码与金额，不能让柜员手填）。
//!
//! 全部方法都有空默认实现：纯内存预览模式不接账本，行为与改动前一致。

use crate::replay::ReplayAttempt;
use crate::reversal::{ExceptionKind, ReversalRecord};
use crate::tx::TxRequest;

/// 异常种类的稳定字符串表示（写盘格式，不能随手改）。
pub fn exception_kind_str(kind: ExceptionKind) -> &'static str {
    match kind {
        ExceptionKind::MarkedForReversal => "marked_for_reversal",
        ExceptionKind::MarkedForRollForward => "marked_for_roll_forward",
    }
}

/// 读回异常种类；未知值报错而不是静默降级成另一种处置。
pub fn parse_exception_kind(raw: &str) -> Result<ExceptionKind, String> {
    match raw {
        "marked_for_reversal" => Ok(ExceptionKind::MarkedForReversal),
        "marked_for_roll_forward" => Ok(ExceptionKind::MarkedForRollForward),
        other => Err(format!("未知的异常种类：{other}")),
    }
}

/// 恢复状态端口。由持久化账本实现，主壳在启动时读回、在状态变更时写回。
pub trait RecoveryJournal: Send + Sync {
    /// 待处置异常清单：(幂等键, 异常种类)。
    fn load_exceptions(&self) -> Result<Vec<(String, String)>, String> {
        Ok(Vec::new())
    }
    fn save_exception(&self, _key: &str, _kind: &str) -> Result<(), String> {
        Ok(())
    }

    /// 冲正记录（含"已授权未确认"的中间态）。
    fn load_reversals(&self) -> Result<Vec<ReversalRecord>, String> {
        Ok(Vec::new())
    }
    fn save_reversal(&self, _record: &ReversalRecord) -> Result<(), String> {
        Ok(())
    }

    /// 重放进度。attempts 跨重启不重置，否则反复重启就能绕过失败预算。
    fn load_attempts(&self) -> Result<Vec<ReplayAttempt>, String> {
        Ok(Vec::new())
    }
    fn save_attempt(&self, _attempt: &ReplayAttempt) -> Result<(), String> {
        Ok(())
    }
    fn remove_attempt(&self, _key: &str) -> Result<(), String> {
        Ok(())
    }

    /// 原交易要素。冲正只能引用已落盘的原交易，不能由调用方凭空填写。
    fn tx_request(&self, _key: &str) -> Result<Option<TxRequest>, String> {
        Ok(None)
    }
}

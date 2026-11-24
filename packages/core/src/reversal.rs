//! 冲正与授权（P3）。
//!
//! 柜面最危险的时刻不是交易失败，是**不知道成没成**：
//! 超时的交易可能已入账，直接重做会重复记账。所以异常交易不进"失败"，
//! 而是进一个显式的处置状态，由人带着授权来决定下一步。
//!
//! 两种异常形态（对齐 Oracle FLEXCUBE 的处置语义）：
//! - `MarkedForReversal`：一阶段就超时，不知道主机有没有处理 → 查账后冲正；
//! - `MarkedForRollForward`：首阶段成功、次阶段超时 → 继续推进而不是回滚。
//!
//! 硬规则：冲正必须经**他人**授权。发起人自己批自己等于没有授权。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 异常处置状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExceptionKind {
    /// 状态不明：主机可能已处理，需要查账后决定是否冲正。
    MarkedForReversal,
    /// 两阶段交易的第一步成功、第二步超时：继续推进，不回去。
    MarkedForRollForward,
}

/// 一次冲正申请。由柜员发起，带着原交易的关键信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReversalRequest {
    /// 原交易的幂等键。
    pub idempotency_key: String,
    /// 发起冲正的柜员号。
    pub initiator: String,
    pub tx_code: String,
    pub amount_cents: i64,
    pub reason: String,
}

/// 授权结论。`approver` 必须与发起人不同。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authorization {
    pub approver: String,
    pub granted: bool,
    pub reason: String,
}

/// 授权端口。真实实现走主管密码/指纹，测试实现可以精确控制结论。
pub trait Authorizer {
    fn authorize(&self, request: &ReversalRequest) -> Authorization;
}

/// 冲正完成的凭证，进审计用。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReversalRecord {
    pub idempotency_key: String,
    pub initiator: String,
    pub approver: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReversalError {
    #[error("该交易没有异常标记，不需要冲正")]
    NotMarked,
    #[error("该交易已经冲正过，不能重复冲正")]
    AlreadyReversed,
    #[error("冲正必须由他人授权：发起人 {0} 不能自己批准")]
    SelfApproval(String),
    #[error("授权被拒绝：{0}")]
    NotAuthorized(String),
}

/// 冲正账本：谁被标记、谁批了、冲过没有。
///
/// 刻意不挂在 `TxEngine` 上：冲正是"异常处置"流程，与正常交易的状态机
/// 生命周期不同（可能隔天处置）。分开后两边都能独立测试。
#[derive(Debug, Default)]
pub struct ReversalBook {
    /// 幂等键 → 异常种类
    exceptions: HashMap<String, ExceptionKind>,
    /// 幂等键 → 冲正凭证
    reversals: HashMap<String, ReversalRecord>,
}

impl ReversalBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// 给一笔交易打异常标记。重复标记以最后一次为准。
    pub fn mark(&mut self, idempotency_key: &str, kind: ExceptionKind) {
        self.exceptions.insert(idempotency_key.to_string(), kind);
    }

    pub fn exception_of(&self, idempotency_key: &str) -> Option<ExceptionKind> {
        self.exceptions.get(idempotency_key).copied()
    }

    /// 待处置的异常交易，按幂等键排序（便于界面稳定展示）。
    pub fn outstanding(&self) -> Vec<(String, ExceptionKind)> {
        unimplemented!()
    }

    pub fn reversal_of(&self, idempotency_key: &str) -> Option<&ReversalRecord> {
        self.reversals.get(idempotency_key)
    }

    /// 申请冲正。
    ///
    /// 顺序：先校验状态（有没有异常、冲过没有、是不是自批），再请求授权。
    /// 校验放在授权之前，是因为"程序上不该发生的操作"不该打扰主管。
    pub fn request(
        &mut self,
        request: &ReversalRequest,
        authorizer: &dyn Authorizer,
    ) -> Result<ReversalRecord, ReversalError> {
        unimplemented!()
    }
}

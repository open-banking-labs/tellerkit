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
    #[error("该交易是待推进（Roll Forward），不能冲正")]
    NotReversible,
    #[error("授权被拒绝：{0}")]
    NotAuthorized(String),
}

/// 冲正账本：谁被标记、谁批了、冲过没有。
///
/// 设计取舍：冲正成功后**保留**异常标记。它是幂等墓碑——
/// 删掉的话，重复申请会先命中"没有异常标记"而返回 `NotMarked`，
/// 报错信息就从"已经冲过了"退化成"这笔没异常"，排障时会误导人。
/// 已冲正的交易由 `outstanding()` 过滤掉，不再出现在待办里。
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
        let mut rows: Vec<(String, ExceptionKind)> = self
            .exceptions
            .iter()
            // 已冲正的交易处置已完成，不再待办；标记本身留给幂等判断（S-05）。
            .filter(|(key, _)| !self.reversals.contains_key(*key))
            .map(|(key, kind)| (key.clone(), *kind))
            .collect();
        // HashMap 的迭代顺序不稳定；排序后界面与测试看到的才是同一个顺序。
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }

    pub fn reversal_of(&self, idempotency_key: &str) -> Option<&ReversalRecord> {
        self.reversals.get(idempotency_key)
    }

    /// 申请冲正。
    ///
    /// 顺序：先校验状态（有没有异常标记、冲过没有、异常类型是否可冲正），
    /// 再请求授权，最后校验授权人不能是发起人。
    ///
    /// 为什么自批只能在授权之后判：授权人身份来自 `authorize()` 的返回值，
    /// 事前根本不知道是谁批的。所以"不打扰主管"只适用于程序上就不成立的操作
    /// （未标记、已冲正、待推进），自批必须走完授权流程再否决。
    pub fn request(
        &mut self,
        request: &ReversalRequest,
        authorizer: &dyn Authorizer,
    ) -> Result<ReversalRecord, ReversalError> {
        // 1. 没有异常标记：这笔交易本就不该走冲正，也不该惊动授权人。
        if !self.exceptions.contains_key(&request.idempotency_key) {
            return Err(ReversalError::NotMarked);
        }
        // 2. 幂等：冲正一次就够，重复冲正等于重复记账。
        if self.reversals.contains_key(&request.idempotency_key) {
            return Err(ReversalError::AlreadyReversed);
        }
        // 3. 待推进（Roll Forward）的语义是"继续推进"，不是"回滚"。
        //    它和待冲正是两种处置方向，不能被同一套流程处理。
        if self.exceptions.get(&request.idempotency_key)
            == Some(&ExceptionKind::MarkedForRollForward)
        {
            return Err(ReversalError::NotReversible);
        }
        // 4. 授权人身份只能从授权端口取得（`ReversalRequest` 里没有 approver 字段），
        //    所以先拿结论，再由引擎自己否决自批：不看 granted，也不落任何记录。
        let authorization = authorizer.authorize(request);
        if authorization.approver == request.initiator {
            return Err(ReversalError::SelfApproval(request.initiator.clone()));
        }
        // 5. 授权被拒：状态原样保留，交易继续待处置。
        if !authorization.granted {
            return Err(ReversalError::NotAuthorized(authorization.reason));
        }
        // 6. 通过：留凭证。异常标记保留——它是幂等判断的依据（S-05）；
        //    "已处置完、不再待办"由 `outstanding` 扣除已冲正的交易来表达。
        let record = ReversalRecord {
            idempotency_key: request.idempotency_key.clone(),
            initiator: request.initiator.clone(),
            approver: authorization.approver,
            reason: request.reason.clone(),
        };
        self.reversals
            .insert(request.idempotency_key.clone(), record.clone());
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// 测试用授权端口：结论固定，并记录调用次数。
    /// 调用次数用来证明"校验在授权之前"——不该打扰主管时确实一次都没调用。
    struct StubAuthorizer {
        approver: &'static str,
        granted: bool,
        reason: &'static str,
        calls: Cell<usize>,
    }

    impl StubAuthorizer {
        fn granting(approver: &'static str) -> Self {
            Self {
                approver,
                granted: true,
                reason: "同意冲正",
                calls: Cell::new(0),
            }
        }

        fn denying(approver: &'static str, reason: &'static str) -> Self {
            Self {
                approver,
                granted: false,
                reason,
                calls: Cell::new(0),
            }
        }

        fn calls(&self) -> usize {
            self.calls.get()
        }
    }

    impl Authorizer for StubAuthorizer {
        fn authorize(&self, _request: &ReversalRequest) -> Authorization {
            self.calls.set(self.calls.get() + 1);
            Authorization {
                approver: self.approver.to_string(),
                granted: self.granted,
                reason: self.reason.to_string(),
            }
        }
    }

    fn request(initiator: &str) -> ReversalRequest {
        ReversalRequest {
            idempotency_key: "TX-001".to_string(),
            initiator: initiator.to_string(),
            tx_code: "1001".to_string(),
            amount_cents: 25_000,
            reason: "主机超时，查账确认未入账".to_string(),
        }
    }

    // S-01 异常交易进入待处置，而不是失败。
    #[test]
    fn s01_marked_transaction_appears_in_outstanding() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        book.mark("TX-002", ExceptionKind::MarkedForRollForward);

        assert_eq!(
            book.outstanding(),
            vec![
                ("TX-001".to_string(), ExceptionKind::MarkedForReversal),
                ("TX-002".to_string(), ExceptionKind::MarkedForRollForward),
            ]
        );
        assert_eq!(
            book.exception_of("TX-001"),
            Some(ExceptionKind::MarkedForReversal)
        );
    }

    // S-02 没有异常标记的交易不能冲正，且不请求授权。
    #[test]
    fn s02_unmarked_transaction_is_rejected_without_authorizing() {
        let mut book = ReversalBook::new();
        let authorizer = StubAuthorizer::granting("T02");

        let err = book.request(&request("T01"), &authorizer).unwrap_err();

        assert_eq!(err, ReversalError::NotMarked);
        assert_eq!(authorizer.calls(), 0, "没有异常标记不该打扰主管");
        assert!(book.reversal_of("TX-001").is_none());
        assert!(book.outstanding().is_empty());
    }

    // S-03 发起人不能自批；关键在于授权端口返回 granted=true。
    #[test]
    fn s03_self_approval_is_rejected_even_when_authorizer_grants() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        // 授权端口明确 granted=true，且授权人恰好是发起人本人。
        let authorizer = StubAuthorizer::granting("T01");

        let err = book.request(&request("T01"), &authorizer).unwrap_err();

        assert_eq!(err, ReversalError::SelfApproval("T01".to_string()));
        assert!(book.reversal_of("TX-001").is_none(), "自批不得留下冲正凭证");
        assert_eq!(
            book.exception_of("TX-001"),
            Some(ExceptionKind::MarkedForReversal)
        );
        assert_eq!(book.outstanding().len(), 1, "交易必须仍在待处置清单里");
    }

    // S-03 自批的拦截先于"授权被拒"的判定：结论是 SelfApproval，而不是 NotAuthorized。
    #[test]
    fn s03_self_approval_outranks_authorizer_denial() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        let authorizer = StubAuthorizer::denying("T01", "不同意");

        let err = book.request(&request("T01"), &authorizer).unwrap_err();

        assert_eq!(err, ReversalError::SelfApproval("T01".to_string()));
    }

    // S-04 授权被拒则不留痕，交易仍在待处置清单里。
    #[test]
    fn s04_denied_authorization_leaves_no_trace() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        let authorizer = StubAuthorizer::denying("T02", "金额与凭证不符");

        let err = book.request(&request("T01"), &authorizer).unwrap_err();

        assert_eq!(
            err,
            ReversalError::NotAuthorized("金额与凭证不符".to_string())
        );
        assert!(book.reversal_of("TX-001").is_none(), "被拒不得产生冲正记录");
        assert_eq!(
            book.outstanding(),
            vec![("TX-001".to_string(), ExceptionKind::MarkedForReversal)]
        );
        assert_eq!(authorizer.calls(), 1);
    }

    // S-05 不能重复冲正，且第二次申请不再请求授权。
    #[test]
    fn s05_reversal_cannot_repeat() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        let authorizer = StubAuthorizer::granting("T02");

        book.request(&request("T01"), &authorizer).unwrap();
        let err = book.request(&request("T01"), &authorizer).unwrap_err();

        assert_eq!(err, ReversalError::AlreadyReversed);
        assert_eq!(authorizer.calls(), 1, "重复冲正不该再次请求授权");
    }

    // S-06 待处置清单顺序稳定（按幂等键排序）。
    /// 待推进的语义是继续推进，不是回滚：它不该被冲正流程处理。
    #[test]
    fn roll_forward_cannot_be_reversed() {
        struct Grant;
        impl Authorizer for Grant {
            fn authorize(&self, _r: &ReversalRequest) -> Authorization {
                Authorization {
                    approver: "T999".into(),
                    granted: true,
                    reason: String::new(),
                }
            }
        }
        let mut book = ReversalBook::new();
        // 测试 helper 固定用 TX-001 作为幂等键，这里必须对齐
        book.mark("TX-001", ExceptionKind::MarkedForRollForward);

        let err = book.request(&request("T01"), &Grant).unwrap_err();
        assert_eq!(err, ReversalError::NotReversible);
        assert!(book.reversal_of("TX-001").is_none());
        // 它仍然是待处置的（方向未定，等人判断）
        assert_eq!(book.outstanding().len(), 1);
    }

    #[test]
    fn s06_outstanding_order_is_stable_by_idempotency_key() {
        let mut book = ReversalBook::new();
        for key in ["TX-009", "TX-002", "TX-007", "TX-001"] {
            book.mark(key, ExceptionKind::MarkedForReversal);
        }

        let first = book.outstanding();
        let keys: Vec<&str> = first.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["TX-001", "TX-002", "TX-007", "TX-009"]);

        for _ in 0..5 {
            assert_eq!(book.outstanding(), first, "多次读取顺序必须一致");
        }
    }

    // 授权通过：留下凭证，并从待处置清单与异常标记中移除（处置完成）。
    #[test]
    fn granted_reversal_records_voucher_and_clears_pending() {
        let mut book = ReversalBook::new();
        book.mark("TX-001", ExceptionKind::MarkedForReversal);
        let authorizer = StubAuthorizer::granting("T02");

        let record = book.request(&request("T01"), &authorizer).unwrap();

        assert_eq!(
            record,
            ReversalRecord {
                idempotency_key: "TX-001".to_string(),
                initiator: "T01".to_string(),
                approver: "T02".to_string(),
                reason: request("T01").reason,
            }
        );
        assert_eq!(book.reversal_of("TX-001"), Some(&record));
        assert!(book.outstanding().is_empty(), "已冲正的交易不再待处置");
        // 决策：标记不删除（它记录的是"当初为何异常"，且是 AlreadyReversed 的判断依据），
        // 待办状态由 `outstanding` 扣除已冲正的交易来表达。
        assert_eq!(
            book.exception_of("TX-001"),
            Some(ExceptionKind::MarkedForReversal),
            "标记保留作幂等凭据，不再出现在待处置清单里"
        );
    }
}

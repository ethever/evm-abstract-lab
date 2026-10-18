//! 可选完成结果的预付许可；发布和丢弃都不再访问累计账本。

use super::WorkBudget;

/// 可选预付被拒绝的原因；余额不足不会把普通执行标记为耗尽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdmissionStop {
    Cancelled,
    AlreadyExhausted,
    Insufficient,
    Overflow,
}

/// 拥有已消费的预付款与原始取消身份，只能消费一次。
///
/// 调用者提供的可信上界必须涵盖结果保留、丢弃及常数大小的发布操作。
/// 许可不保存账本引用，也不退回未使用的预付款。
#[derive(Debug)]
#[must_use = "completion publication requires consuming its prepaid permit"]
pub(crate) struct CompletionPermit {
    cancellation: analysis_control::Cancellation,
}

impl CompletionPermit {
    /// 紧邻结果移交调用；只检查原始取消身份，不再消耗工作额度。
    pub(crate) fn publish_allowed(self) -> bool {
        !self.cancellation.is_cancelled()
    }
}

impl WorkBudget {
    /// 在普通分析之前预付完成结果的完整可信上界。
    ///
    /// `usize::MAX` 是上界计算溢出的哨兵。可选预付先检查余额，不能用
    /// `charge` 的拒绝路径把尚可继续的普通执行变成 sticky 耗尽。
    pub(crate) fn try_completion_escrow(
        &mut self,
        bound: usize,
    ) -> Result<CompletionPermit, AdmissionStop> {
        if self.cancellation.is_cancelled() {
            return Err(AdmissionStop::Cancelled);
        }
        if self.exhausted {
            return Err(AdmissionStop::AlreadyExhausted);
        }
        if bound == usize::MAX {
            return Err(AdmissionStop::Overflow);
        }
        if !self.can_afford(bound) {
            return Err(if self.cancellation.is_cancelled() {
                AdmissionStop::Cancelled
            } else {
                AdmissionStop::Insufficient
            });
        }
        // 账本由独占借用保护，余额检查后只有原始取消信号能使 charge 失败。
        if !self.charge(bound) {
            return Err(AdmissionStop::Cancelled);
        }
        Ok(CompletionPermit {
            cancellation: self.cancellation(),
        })
    }
}

#[cfg(test)]
mod tests;

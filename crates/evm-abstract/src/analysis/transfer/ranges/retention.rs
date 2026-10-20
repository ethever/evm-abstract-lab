//! Raw causal evidence for a missing memory-guard expression.
//!
//! Later operations can propagate a lost expression without another error.
//! Inherited refusals and nonfinite failures prevent attributing that loss to
//! this guard's finite retention policy, even if a finite failure also occurs.

use crate::domain::WordElement;

use crate::{
    analysis::FrontierReason,
    domain::{EvmValue, relational::QueryReason, symbolic::ExprError},
};

use super::super::{RetentionCause, errors::retention_expression};

pub(super) struct GuardRetention {
    cause: Option<RetentionCause>,
    blocked: bool,
}

impl GuardRetention {
    pub(super) fn new<N: WordElement>(offset: &EvmValue<N>, size: &EvmValue<N>) -> Self {
        Self {
            cause: None,
            blocked: offset.symbolic_limit_reached() || size.symbolic_limit_reached(),
        }
    }

    /// A declined identity is independent of this guard's finite retention cap.
    pub(super) fn veto_identity_failure(&mut self) {
        self.cause = None;
        self.blocked = true;
    }

    /// Record each original guard operation once. No new error preserves the
    /// earlier cause; an ineligible raw error permanently clears it.
    pub(super) fn record(&mut self, error: Option<&ExprError>) {
        if self.blocked {
            return;
        }
        if let Some(error) = error {
            self.cause = retention_expression(error);
            self.blocked = self.cause.is_none();
        }
    }

    pub(super) fn cause(&self) -> Option<RetentionCause> {
        self.cause
    }
}

/// Only the constrained branch's missing expression can use guard evidence.
/// Observation, assumption and work failures preserve their independent cause.
pub(super) struct Failure {
    pub(super) reason: FrontierReason,
    pub(super) retention: Option<RetentionCause>,
}

impl Failure {
    pub(super) fn plain(reason: FrontierReason) -> Self {
        Self {
            reason,
            retention: None,
        }
    }

    pub(super) fn missing_expression(retention: Option<&GuardRetention>) -> Self {
        Self {
            reason: FrontierReason::Relations(QueryReason::ExpressionLimit),
            retention: retention.and_then(GuardRetention::cause),
        }
    }
}

#[cfg(test)]
mod tests;

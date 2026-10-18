//! Preserve cancellation and symbolic resource failures at effect boundaries.

use crate::{
    analysis::{ByteRangeBoundary, FrontierReason, JoinBoundary},
    domain::{
        relational::QueryReason,
        symbolic::{ExprError, StateError},
    },
    world::{RangeError, StoreError},
};

use super::RetentionCause;

pub(super) fn retention_expression(error: &ExprError) -> Option<RetentionCause> {
    match error {
        ExprError::Limit { nodes, depth } if *nodes != usize::MAX && *depth != usize::MAX => {
            Some(RetentionCause::Expression)
        }
        _ => None,
    }
}

pub(super) fn retention_bytes(error: &RangeError) -> Option<RetentionCause> {
    match error {
        RangeError::Expression(error) => retention_expression(error),
        RangeError::State(StateError::Limit { nodes, depth })
            if *nodes != usize::MAX && *depth != usize::MAX =>
        {
            Some(RetentionCause::State)
        }
        _ => None,
    }
}

pub(super) fn retention_store(error: &StoreError) -> Option<RetentionCause> {
    match error {
        StoreError::Expression(error) => retention_expression(error),
        StoreError::State(StateError::Limit { nodes, depth })
            if *nodes != usize::MAX && *depth != usize::MAX =>
        {
            Some(RetentionCause::State)
        }
        StoreError::Bytes(error) => retention_bytes(error),
        _ => None,
    }
}

pub(super) fn expression(error: ExprError) -> FrontierReason {
    FrontierReason::Relations(match error {
        ExprError::Cancelled => QueryReason::Cancelled,
        ExprError::Limit { .. } | ExprError::ExportBytes { .. } => QueryReason::ExpressionLimit,
        ExprError::Unsupported(op) => {
            QueryReason::ScalarFactError(crate::domain::facts::FactError::UnsupportedOperation(op))
        }
        ExprError::Arity {
            opcode,
            expected,
            actual,
        } => QueryReason::ScalarFactError(crate::domain::facts::FactError::InvalidOperation {
            opcode,
            expected,
            actual,
        }),
    })
}

pub(super) fn bytes(error: RangeError) -> FrontierReason {
    match error {
        RangeError::Expression(error) => expression(error),
        RangeError::State(StateError::Cancelled) => {
            FrontierReason::Relations(QueryReason::Cancelled)
        }
        RangeError::State(StateError::Limit { .. }) => {
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        }
        RangeError::State(StateError::IdentityExhausted) => {
            FrontierReason::StateJoin(JoinBoundary::IdentityExhausted)
        }
        RangeError::UnknownSize => FrontierReason::ByteRange(ByteRangeBoundary::UnknownSize),
        RangeError::OffsetTooLarge(offset) => {
            FrontierReason::ByteRange(ByteRangeBoundary::OffsetTooLarge(offset))
        }
        RangeError::Overflow { offset, size } => {
            FrontierReason::ByteRange(ByteRangeBoundary::Overflow { offset, size })
        }
        RangeError::TooLarge { requested, limit } => {
            FrontierReason::ByteRange(ByteRangeBoundary::TooLarge { requested, limit })
        }
    }
}

pub(super) fn store(error: StoreError) -> FrontierReason {
    match error {
        StoreError::Expression(error) => expression(error),
        StoreError::State(error) => bytes(RangeError::State(error)),
        StoreError::Bytes(error) => bytes(error),
    }
}

//! Preserve cancellation and symbolic resource failures at effect boundaries.

use crate::{
    analysis::{ByteRangeBoundary, FrontierReason, JoinBoundary},
    domain::{
        relational::QueryReason,
        symbolic::{ExprError, StateError},
    },
    world::{RangeError, StoreError},
};

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

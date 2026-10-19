//! Canonical numerical state shared by machine joins and relative procedures.
//!
//! These templates retain whole-state bounds and optional finite guarantees,
//! without executable source identities. Instantiation always creates fresh
//! roots. Scheduling, retry admission and publication remain with callers.

use super::machine::{Frame, FrameByteKind, FrameMapOrder};
use crate::{
    analysis::{
        CallStack, ChildFrame, Continuation, FrameCode, MachineError, MachineKey, MachinePayload,
        RootFrame,
    },
    domain::{AbstractValue, Domain, NumericValue, relational::RelationState, symbolic::ExprId},
    resource::WorkBudget,
    world::{ByteArray, ByteArraySummary, Entry, Snapshot, StoreSummary},
};
use alloy_primitives::Address;
use std::collections::{BTreeMap, BTreeSet};

pub(super) mod bounded;
pub(super) mod context;
pub(super) mod loop_bound;
pub(super) mod payload;
pub(super) mod projection;
pub(super) mod stack_alias;
pub(super) mod storage_observations;
mod transform;

pub(super) use loop_bound::LoopBound;
pub(super) use payload::{form, instantiate, join, semantic_eq};
pub(super) use transform::StatePart;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FrameSummary {
    pub(super) frame: Frame<ByteArraySummary, StoreSummary, AbstractValue>,
    pub(super) stack_aliases: stack_alias::StackAliases,
}

impl std::ops::Deref for FrameSummary {
    type Target = Frame<ByteArraySummary, StoreSummary, AbstractValue>;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl std::ops::DerefMut for FrameSummary {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PayloadSummary {
    pub(super) frames: Vec<FrameSummary>,
    pub(super) continuations: Vec<Continuation>,
    pub(super) store: StoreSummary,
    pub(super) storage_observations: storage_observations::StorageObservations,
    /// Canonical retained-object and independent-instantiation charge.
    pub(super) work: usize,
    /// Procedure location only; this is never a witness for the joined invariant.
    pub(super) location: MachineKey,
}

/// Establish a runtime value without equating separate executions of a PC.
pub(super) fn identify(value: AbstractValue, enabled: bool) -> AbstractValue {
    if enabled
        && value.singleton().is_none()
        && value.expression().is_none()
        && let Some(expression) = ExprId::fresh()
    {
        return value.with_expression(expression);
    }
    value
}

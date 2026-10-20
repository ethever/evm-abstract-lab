//! Canonical numerical state shared by machine joins and relative procedures.
//!
//! These templates retain whole-state bounds and optional finite guarantees,
//! without executable source identities. Instantiation always creates fresh
//! roots. Scheduling, retry admission and publication remain with callers.

use crate::domain::WordElement;

use super::machine::{EvmFrame, FrameByteKind, FrameMapOrder};
use crate::{
    analysis::machine::{
        EvmCallStack, EvmChildFrame, EvmContinuation, EvmMachinePayload, EvmRootFrame, FrameCode,
        MachineError, MachineKey,
    },
    domain::{AlgebraRef, EvmValue, relational::RelationState, symbolic::ExprId},
    resource::WorkBudget,
    world::{EvmByteArray, EvmByteArraySummary, EvmEntry, EvmSnapshot, EvmStoreSummary},
};
use alloy_primitives::Address;
use std::collections::{BTreeMap, BTreeSet};

pub(super) mod bounded;
#[cfg(test)]
mod carrier_tests;
pub(super) mod context;
pub(super) mod loop_bound;
pub(super) mod payload;
pub(super) mod projection;
pub(super) mod stack_alias;
pub(super) mod storage_observations;
mod transform;

pub(super) use loop_bound::{EvmLoopBound, LoopBound};
pub(super) use payload::{form, form_normalized, instantiate, join, semantic_eq};
pub(super) use transform::StatePart;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EvmFrameSummary<N: WordElement> {
    pub(super) frame: EvmFrame<N, EvmByteArraySummary<N>, EvmStoreSummary<N>, EvmValue<N>>,
    pub(super) stack_aliases: stack_alias::StackAliases,
}

impl<N: WordElement> std::ops::Deref for EvmFrameSummary<N> {
    type Target = EvmFrame<N, EvmByteArraySummary<N>, EvmStoreSummary<N>, EvmValue<N>>;
    fn deref(&self) -> &Self::Target {
        &self.frame
    }
}

impl<N: WordElement> std::ops::DerefMut for EvmFrameSummary<N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.frame
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EvmPayloadSummary<N: WordElement> {
    pub(super) frames: Vec<EvmFrameSummary<N>>,
    pub(super) continuations: Vec<EvmContinuation<N>>,
    pub(super) store: EvmStoreSummary<N>,
    pub(super) storage_observations: storage_observations::EvmStorageObservations<N>,
    /// Canonical retained-object and independent-instantiation charge.
    pub(super) work: usize,
    /// Procedure location only; this is never a witness for the joined invariant.
    pub(super) location: MachineKey,
}

/// Establish a runtime value without equating separate executions of a PC.
pub(super) fn identify<N: WordElement>(value: EvmValue<N>, enabled: bool) -> EvmValue<N> {
    if enabled
        && value.singleton().is_none()
        && value.expression().is_none()
        && let Some(expression) = ExprId::fresh()
    {
        return value.with_expression(expression);
    }
    value
}

pub(in crate::analysis) type FrameSummary = EvmFrameSummary<crate::domain::NumericValue>;

pub(in crate::analysis) type PayloadSummary = EvmPayloadSummary<crate::domain::NumericValue>;

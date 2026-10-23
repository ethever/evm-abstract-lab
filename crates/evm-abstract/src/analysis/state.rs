//! Canonical numerical state shared by machine joins and relative procedures.
//!
//! These templates retain whole-state bounds and optional finite guarantees,
//! without executable source identities. Instantiation always creates fresh
//! roots. Scheduling, retry admission and publication remain with callers.

use crate::domain::{WordElement, provenance::Origin};

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
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::BTreeSet;

pub(super) mod bounded;
#[cfg(test)]
mod carrier_tests;
#[cfg(test)]
pub(super) mod context;
pub(super) mod loop_bound;
pub(super) mod payload;
mod policy;
pub(super) mod projection;
pub(super) mod query;
pub(super) mod stack_alias;
pub(super) mod storage_observations;
#[cfg(test)]
mod tests;
mod transform;

pub(super) use loop_bound::EvmLoopBound;
#[cfg(test)]
pub(super) use payload::{form, form_normalized};
pub(super) use payload::{instantiate, join, semantic_eq};
pub use policy::ProcedureOutput;
pub(super) use transform::StatePart;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct EvmFrameSummary<N: WordElement> {
    pub(super) semantics: super::TransferSemantics,
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

/// Opaque independent numerical template for a complete relative machine state.
/// Use the metered policy operations to project, join and instantiate it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmPayloadSummary<N: WordElement> {
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
/// Keep cached canonical literal proofs usable by literal-only consumers.
/// Exact observations and inputs still get independent expressions; optional
/// identification never discovers an opaque numeric singleton.
pub(super) fn identify<N: WordElement>(value: EvmValue<N>, enabled: bool) -> EvmValue<N> {
    if !enabled || value.expression().is_some() {
        return value;
    }
    let literal = value.identity().input().is_none()
        && !value.symbolic_limit_reached()
        && !value.provenance().is_code_address()
        && value
            .provenance()
            .origins()
            .sources()
            .is_some_and(|sources| sources.len() == 1 && sources.contains(&Origin::Constant))
        && (value.constants().is_some_and(|values| values.len() == 1)
            || value.known_bits().singleton().is_some()
            || value.interval().singleton().is_some());
    if !literal && let Some(expression) = ExprId::fresh() {
        return value.with_expression(expression);
    }
    value
}

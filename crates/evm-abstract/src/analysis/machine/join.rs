//! One selector covers the entire machine. A failed construction never installs
//! a subset of fields. Scheduling retains complete represented alternatives,
//! rather than treating a numeric widening as a symbolic state fixed point.

use super::{ByteRangeBoundary, FrontierReason, MachinePayload};
use crate::{
    domain::{
        AbstractValue, Domain,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprLimits, StateError},
    },
    resource::WorkBudget,
    world::{RangeError, StoreError},
};
use serde::Serialize;
use std::{collections::BTreeSet, sync::Arc};
use thiserror::Error;

mod cases;
mod coverage;
mod widening;

/// Machine-level policy failures, independently of scalar or array retention.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum JoinBoundary {
    /// Frame roles, code, or continuations disagree despite a matching key.
    Shape,
    /// No unique selector can be allocated.
    IdentityExhausted,
    /// A further complete alternative exceeds the frozen disjunction policy.
    Alternatives {
        /// Maximum retained complete alternatives for one structural state.
        limit: usize,
    },
}

/// An atomic machine transformation could not retain its complete result.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum MachineError {
    /// Execution roles or continuation destinations disagree.
    #[error("machine frame or continuation shapes disagree")]
    Shape,
    /// The unique selector namespace cannot allocate another identity.
    #[error("machine selector namespace exhausted")]
    IdentityExhausted,
    /// A new uncovered complete alternative exceeds the disjunction policy.
    #[error("machine alternatives exceed limit {limit}")]
    Alternatives {
        /// Maximum represented alternatives per structural state.
        limit: usize,
    },
    /// The shared cumulative work reservation was rejected.
    #[error("machine transformation work exhausted")]
    Work,
    /// Relative procedure allocation exceeded the shared state capacity.
    #[error("machine state budget exhausted")]
    States,
    /// Scalar expression construction failed before the result was installed.
    #[error("machine expression: {0}")]
    Expression(ExprError),
    /// A byte root, source, length or view could not be retained.
    #[error("machine byte state: {0}")]
    Bytes(RangeError),
    /// Transaction or rollback state could not be retained.
    #[error("machine store: {0}")]
    Store(StoreError),
    /// The complete path disjunction could not be retained.
    #[error("machine path join could not finish: {0:?}")]
    Relations(QueryReason),
}

impl From<ExprError> for MachineError {
    fn from(value: ExprError) -> Self {
        Self::Expression(value)
    }
}
impl From<RangeError> for MachineError {
    fn from(value: RangeError) -> Self {
        Self::Bytes(value)
    }
}
impl From<StoreError> for MachineError {
    fn from(value: StoreError) -> Self {
        Self::Store(value)
    }
}

impl MachineError {
    /// Only complete-state precision refusals admit whole-entry projection.
    /// Frontier classification is deliberately broader and is not eligibility.
    fn permits_bounded_promotion(&self) -> bool {
        matches!(
            self,
            Self::Alternatives { .. }
                | Self::Expression(ExprError::Limit { .. })
                | Self::Bytes(
                    RangeError::Expression(ExprError::Limit { .. })
                        | RangeError::State(StateError::Limit { .. })
                )
                | Self::Store(
                    StoreError::Expression(ExprError::Limit { .. })
                        | StoreError::State(StateError::Limit { .. })
                        | StoreError::Bytes(
                            RangeError::Expression(ExprError::Limit { .. })
                                | RangeError::State(StateError::Limit { .. })
                        )
                )
                | Self::Relations(QueryReason::ExpressionLimit | QueryReason::ConstraintLimit)
        )
    }

    pub(crate) fn check_cancelled() -> Result<(), Self> {
        if analysis_control::current_cancellation().is_cancelled() {
            Err(Self::Relations(QueryReason::Cancelled))
        } else {
            Ok(())
        }
    }

    pub(crate) fn work_failure() -> Self {
        if analysis_control::current_cancellation().is_cancelled() {
            Self::Relations(QueryReason::Cancelled)
        } else {
            Self::Work
        }
    }

    pub(crate) fn frontier_reason(self) -> FrontierReason {
        match self {
            Self::Shape => FrontierReason::StateJoin(JoinBoundary::Shape),
            Self::IdentityExhausted => FrontierReason::StateJoin(JoinBoundary::IdentityExhausted),
            Self::Alternatives { limit } => {
                FrontierReason::StateJoin(JoinBoundary::Alternatives { limit })
            }
            Self::Work => FrontierReason::Work,
            Self::States => FrontierReason::Budget(crate::analysis::Limit::States),
            Self::Expression(error) => expression_reason(error),
            Self::Bytes(error) => bytes_reason(error),
            Self::Store(error) => store_reason(error),
            Self::Relations(reason) => FrontierReason::Relations(reason),
        }
    }
}

fn expression_reason(error: ExprError) -> FrontierReason {
    FrontierReason::Relations(match error {
        ExprError::Cancelled => QueryReason::Cancelled,
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
        ExprError::Limit { .. } | ExprError::ExportBytes { .. } => QueryReason::ExpressionLimit,
    })
}

fn state_reason(error: StateError) -> FrontierReason {
    match error {
        StateError::Cancelled => FrontierReason::Relations(QueryReason::Cancelled),
        StateError::Limit { .. } => FrontierReason::Relations(QueryReason::ExpressionLimit),
        StateError::IdentityExhausted => FrontierReason::StateJoin(JoinBoundary::IdentityExhausted),
    }
}

fn bytes_reason(error: RangeError) -> FrontierReason {
    match error {
        RangeError::Expression(error) => expression_reason(error),
        RangeError::State(error) => state_reason(error),
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

fn store_reason(error: StoreError) -> FrontierReason {
    match error {
        StoreError::Expression(error) => expression_reason(error),
        StoreError::State(error) => state_reason(error),
        StoreError::Bytes(error) => bytes_reason(error),
    }
}

pub(super) fn checkpoint() -> Result<(), MachineError> {
    MachineError::check_cancelled()
}

pub(super) fn join_value(
    left: &AbstractValue,
    right: &AbstractValue,
    domain: Domain,
    selector: &ExprId,
    limits: ExprLimits,
) -> Result<AbstractValue, MachineError> {
    checkpoint()?;
    if domain.spec().relations().enabled {
        Ok(left.join_guarded(right, domain, selector.clone(), limits)?)
    } else {
        Ok(domain.join(left, right))
    }
}

impl MachinePayload {
    /// Comparison-only normalization removes redundant singleton terms and
    /// ephemeral copy labels. It never rewrites a source, byte/slot root, path
    /// guard or historical read. The executable payload keeps its expressions.
    pub(crate) fn content_form(&self) -> Result<Self, MachineError> {
        checkpoint()?;
        let mut content = self.clone();
        let mut normalize = |value: &mut AbstractValue| {
            value.forget_identity();
            if value.singleton().is_some() {
                value.forget_expression();
            }
        };
        for frame in content.call_stack.iter_mut() {
            checkpoint()?;
            for value in &mut frame.stack {
                normalize(value);
            }
            normalize(&mut frame.call_value);
            for value in [&mut frame.address_operand, &mut frame.caller_operand]
                .into_iter()
                .flatten()
            {
                normalize(value);
            }
            frame.memory.update_values(&mut normalize);
            frame.calldata.update_values(&mut normalize);
            frame.returndata.update_values(&mut normalize);
            let mut saved = frame.saved_store.state().clone();
            saved.update_values(&mut normalize);
            frame.saved_store = crate::world::Snapshot::from_state(saved);
        }
        for child in content.call_stack.children_mut() {
            checkpoint()?;
            normalize(&mut child.continuation.output_offset);
            normalize(&mut child.continuation.output_size);
        }
        content.store.update_values(&mut normalize);
        checkpoint()?;
        Ok(content)
    }

    pub(crate) fn join(&self, other: &Self, domain: Domain) -> Result<Self, MachineError> {
        checkpoint()?;
        if self == other {
            return Ok(self.clone());
        }
        if self.key() != other.key() {
            return Err(MachineError::Shape);
        }
        let selector = ExprId::fresh().ok_or(MachineError::IdentityExhausted)?;
        let policy = domain.spec().relations();
        let limits = policy.expressions();
        let mut result = self.clone();
        result.call_stack =
            self.call_stack
                .join_guarded(&other.call_stack, domain, selector.clone(), limits)?;
        result.store = self
            .store
            .join_guarded(&other.store, domain, selector.clone(), limits)?;
        result.relations = if policy.enabled {
            self.relations
                .join_guarded(&other.relations, selector, &policy)
                .map_err(MachineError::Relations)?
        } else {
            // Numeric mode keeps a sound union of roots but cannot consume the
            // scalar/root pairing. Common guarantees are safe in this mode.
            self.relations.join(&other.relations)
        };
        checkpoint()?;
        Ok(result)
    }
}

/// Exact complete states whose valuations are already included in an entry.
/// An equality match is a coverage proof; a digest or an unchanged Fresh leaf
/// alone is never used as one. No list is preallocated from its configured cap.
#[derive(Clone)]
pub(crate) struct EntryAlternatives {
    represented: Vec<Arc<MachinePayload>>,
    cycle_widened: bool,
    loop_bound: Option<crate::analysis::transfer::procedure::LoopBound>,
}

impl EntryAlternatives {
    /// The caller charges the payload copy before retaining it.
    pub(crate) fn new(payload: &MachinePayload) -> Self {
        Self {
            represented: vec![Arc::new(payload.clone())],
            cycle_widened: false,
            loop_bound: None,
        }
    }

    pub(crate) fn work_size(&self) -> usize {
        self.represented.iter().fold(
            self.loop_bound
                .as_ref()
                .map_or(1, |bound| bound.work_size()),
            |cost, payload| cost.saturating_add(payload.work_size()),
        )
    }

    pub(crate) fn has_loop_bound(&self) -> bool {
        self.loop_bound.is_some()
    }

    pub(crate) fn can_widen_loop(&self) -> bool {
        self.has_loop_bound() || self.represented.len() >= 2
    }

    /// Shared policy for propagation and staged replay. Exact acyclic retention
    /// gets the first attempt; only a typed precision refusal can promote it.
    /// Failed attempts keep metadata and both operands intact, but never refund
    /// work. LoopBound owns the single paid coarse materialization retry.
    #[expect(
        clippy::too_many_arguments,
        reason = "whole-entry policy shares the execution environment, selectors and ledger"
    )]
    pub(crate) fn join_entry(
        &mut self,
        entry: &MachinePayload,
        incoming: &MachinePayload,
        environment: &crate::world::Entry,
        domain: Domain,
        budget: &mut WorkBudget,
        selectors: &BTreeSet<ExprId>,
        cyclic: bool,
    ) -> Result<Option<MachinePayload>, MachineError> {
        if self.has_loop_bound() || cyclic && self.can_widen_loop() {
            return self.join_loop(entry, incoming, environment, domain, budget);
        }
        match self.join_cyclic(entry, incoming, domain, budget, selectors, cyclic) {
            Err(error) if !cyclic && error.permits_bounded_promotion() => {
                checkpoint()?;
                self.join_loop(entry, incoming, environment, domain, budget)
            }
            result => result,
        }
    }

    /// Retain one canonical bound instead of historical complete alternatives.
    /// Admission belongs to join_entry; publication follows full construction.
    pub(crate) fn join_loop(
        &mut self,
        entry: &MachinePayload,
        incoming: &MachinePayload,
        environment: &crate::world::Entry,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<Option<MachinePayload>, MachineError> {
        let updated = if let Some(bound) = &self.loop_bound {
            bound.update(incoming, environment, domain, budget)?
        } else {
            Some(crate::analysis::transfer::procedure::LoopBound::new(
                entry,
                incoming,
                environment,
                domain,
                budget,
            )?)
        };
        let Some((bound, payload)) = updated else {
            return Ok(None);
        };
        if payload.key() != entry.key() {
            return Err(MachineError::Shape);
        }
        checkpoint()?;
        self.loop_bound = Some(bound);
        self.represented.clear();
        self.cycle_widened = false;
        Ok(Some(payload))
    }

    /// None means the whole incoming payload is already covered. Metadata is
    /// committed separately, after every field of the join has succeeded.
    #[cfg(test)]
    pub(crate) fn join(
        &mut self,
        entry: &MachinePayload,
        incoming: &MachinePayload,
        domain: Domain,
        budget: &mut WorkBudget,
    ) -> Result<Option<MachinePayload>, MachineError> {
        self.join_cyclic(entry, incoming, domain, budget, &BTreeSet::new(), false)
    }

    pub(crate) fn join_cyclic(
        &mut self,
        entry: &MachinePayload,
        incoming: &MachinePayload,
        domain: Domain,
        budget: &mut WorkBudget,
        selectors: &BTreeSet<ExprId>,
        cyclic: bool,
    ) -> Result<Option<MachinePayload>, MachineError> {
        let mut incoming_content = None;
        for previous in std::iter::once(entry).chain(self.represented.iter().map(Arc::as_ref)) {
            if !coverage::same_byte_roots(previous, incoming, budget)? {
                continue;
            }
            let cost = previous
                .work_size()
                .saturating_add(incoming.work_size())
                .saturating_mul(4);
            if cost == usize::MAX || !budget.charge(cost) {
                return Err(MachineError::work_failure());
            }
            // Only a root-matching candidate needs this clone. Reuse it for
            // later matching candidates; their complete comparisons stay paid.
            if incoming_content.is_none() {
                incoming_content = Some(incoming.content_form()?);
            }
            let incoming_content = incoming_content
                .as_ref()
                .expect("prepared matching content");
            if previous == incoming || previous.content_form()? == *incoming_content {
                return Ok(None);
            }
            if coverage::covers(previous, incoming, domain, budget)? {
                return Ok(None);
            }
        }
        if !self.cycle_widened
            && self.represented.len() >= 2
            && let Some(widened) = widening::stack(entry, incoming, domain)?
        {
            return Ok((widened.content_form()? != entry.content_form()?).then_some(widened));
        }
        let unconstrained = entry.relations.is_empty() && incoming.relations.is_empty();
        if self.cycle_widened && !cyclic && !unconstrained {
            let joined = entry.join(incoming, domain)?;
            checkpoint()?;
            self.represented = vec![Arc::new(entry.clone())];
            self.cycle_widened = false;
            return Ok(Some(joined));
        }
        if self.cycle_widened || cyclic && self.represented.len() >= 2 {
            let (previous, initial) = if self.cycle_widened {
                (self.represented.as_slice(), None)
            } else {
                (&[][..], Some(entry))
            };
            let prepared = cases::prepare(previous, initial, incoming, selectors, domain, budget)?;
            if self.cycle_widened && prepared == self.represented {
                return Ok(None);
            }
            let mut iter = prepared.iter();
            let mut widened = iter.next().ok_or(MachineError::Shape)?.as_ref().clone();
            for alternative in iter {
                let cost = widened
                    .work_size()
                    .saturating_add(alternative.work_size())
                    .saturating_mul(4);
                if cost == usize::MAX || !budget.charge(cost) {
                    return Err(MachineError::work_failure());
                }
                widened = widened.join(alternative, domain)?;
            }
            checkpoint()?;
            self.represented = prepared;
            self.cycle_widened = true;
            return Ok(Some(widened));
        }
        let limit = domain.spec().relations().max_constraints;
        if self.represented.len() >= limit {
            return Err(MachineError::Alternatives { limit });
        }
        // Reserve construction plus persistent retention, in addition to the
        // comparisons above. Logical byte lengths never enter this cost.
        let cost = entry
            .work_size()
            .saturating_add(incoming.work_size())
            .saturating_mul(4);
        if cost == usize::MAX || !budget.charge(cost) {
            return Err(MachineError::work_failure());
        }
        Ok(Some(entry.join(incoming, domain)?))
    }

    pub(crate) fn retain(&mut self, incoming: MachinePayload) {
        if !self.cycle_widened && !self.has_loop_bound() {
            self.represented.push(Arc::new(incoming));
        }
    }
}

#[cfg(test)]
mod tests;

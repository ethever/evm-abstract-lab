//! One selector covers the entire machine. A failed construction never installs
//! a subset of fields. Scheduling retains complete represented alternatives,
//! rather than treating a numeric widening as a symbolic state fixed point.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{ByteRangeBoundary, EvmMachinePayload, FrontierReason};
use crate::{
    domain::{
        AlgebraRef, EvmValue,
        relational::QueryReason,
        symbolic::{ExprError, ExprId, ExprLimits, StateError},
    },
    resource::WorkBudget,
    world::{RangeError, StoreError},
};
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;

mod coverage;

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
    /// A required coarse-process representation or consumer is unavailable.
    #[error("coarse process boundary: {0:?}")]
    Process(crate::analysis::ProcessBoundary),
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
        self.permits_bound_coarsening()
            || matches!(
                self,
                Self::Alternatives { .. }
                    | Self::Relations(QueryReason::ExpressionLimit | QueryReason::ConstraintLimit)
            )
    }

    /// Coarsening an admitted bound handles retained expression/array capacity.
    /// Runtime admission separately requires finite raw producer limits; this
    /// fallback policy cannot grant permission to retry an execution.
    pub(in crate::analysis) fn permits_bound_coarsening(&self) -> bool {
        matches!(
            self,
            Self::Expression(ExprError::Limit { .. })
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
            Self::Process(reason) => FrontierReason::Process(reason),
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

pub(super) fn join_value<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    left: &EvmValue<N>,
    right: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
    selector: &ExprId,
    limits: ExprLimits,
) -> Result<EvmValue<N>, MachineError> {
    checkpoint()?;
    if domain.spec().relations().enabled {
        Ok(left.join_guarded(right, domain, selector.clone(), limits)?)
    } else {
        Ok(domain.join(left, right))
    }
}

impl<N: WordElement> EvmMachinePayload<N> {
    /// Conservative whole-machine retention/traversal quote for semantic
    /// policies. Owned copying requires its separate declared clone payment.
    pub fn policy_work_size(&self) -> usize {
        self.work_size()
    }

    /// Copy a complete input under the run's shared work and cancellation token.
    pub fn copy_for_policy<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut crate::analysis::engine::policy::PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        cx.reserve(self.policy_work_size().saturating_mul(2))?;
        self.pay_clone_with(cx.budget)?;
        let copied = self.clone();
        cx.reserve(0)?;
        Ok(copied)
    }

    /// Cover this input while forgetting optional histories of every suspended
    /// and active frame. The returned payload supplies the coarser actual key.
    pub fn forget_jump_history<D: EvmNumericAlgebra<Element = N>>(
        &self,
        cx: &mut crate::analysis::engine::policy::PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        let mut prepared = self.copy_for_policy(cx)?;
        for frame in prepared.call_stack.iter_mut() {
            frame.key.jump_history.clear();
        }
        cx.reserve(0)?;
        Ok(prepared)
    }

    /// Comparison-only normalization removes ephemeral copy labels. Retained
    /// expressions remain evidence even around numeric Top; this traversal has
    /// no independent numeric-query budget to prove any term redundant.
    pub(crate) fn content_form(&self) -> Result<Self, MachineError> {
        checkpoint()?;
        let mut content = self.clone();
        let mut normalize = |value: &mut EvmValue<N>| value.forget_identity();
        content.update_values(&mut normalize)?;
        checkpoint()?;
        Ok(content)
    }

    pub(crate) fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> Result<Self, MachineError> {
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
pub(crate) struct EvmEntryAlternatives<N: WordElement> {
    represented: Vec<Arc<EvmMachinePayload<N>>>,
    loop_bound: Option<crate::analysis::state::EvmLoopBound<N>>,
}

impl<N: WordElement> EvmEntryAlternatives<N> {
    /// The caller charges the payload copy before retaining it.
    pub(crate) fn new(payload: &EvmMachinePayload<N>) -> Self {
        Self {
            represented: vec![Arc::new(payload.clone())],
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

    /// Shared policy for propagation and staged replay. Exact retention gets
    /// the first attempt unless the separate scheduler requests acceleration;
    /// a typed precision refusal may promote a Join without numeric widening.
    /// Failed attempts keep metadata and both operands intact, but never refund
    /// work. The persistent bounded representation owns the paid materialization
    /// fallback, independently of this admission and publication policy.
    pub(crate) fn join_entry<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        entry: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        environment: &crate::world::EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        accelerate: bool,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        if self.has_loop_bound() || accelerate {
            return if accelerate {
                self.join_loop(entry, incoming, environment, domain, budget)
            } else {
                self.join_loop_scheduled(entry, incoming, environment, domain, budget, false)
            };
        }
        match self.join_correlated(entry, incoming, domain, budget) {
            Err(error) if error.permits_bounded_promotion() => {
                checkpoint()?;
                self.join_loop_scheduled(entry, incoming, environment, domain, budget, accelerate)
            }
            result => result,
        }
    }

    /// Retain one canonical bound instead of historical complete alternatives.
    /// Admission belongs to join_entry; publication follows full construction.
    pub(crate) fn join_loop<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        entry: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        environment: &crate::world::EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        self.join_loop_scheduled(entry, incoming, environment, domain, budget, true)
    }

    /// Bound promotion is a covering representation change; only the explicit
    /// schedule may widen scalar fields, including updates after promotion.
    pub(crate) fn join_loop_scheduled<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        entry: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        environment: &crate::world::EvmEntry<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
        widen: bool,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        let updated = if let Some(bound) = &self.loop_bound {
            if widen {
                bound.update(incoming, environment, domain, budget)?
            } else {
                bound.update_scheduled(incoming, environment, domain, budget, false)?
            }
        } else if widen {
            Some(crate::analysis::state::EvmLoopBound::new(
                entry,
                incoming,
                environment,
                domain,
                budget,
            )?)
        } else {
            Some(crate::analysis::state::EvmLoopBound::new_scheduled(
                entry,
                incoming,
                environment,
                domain,
                budget,
                false,
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
        Ok(Some(payload))
    }

    /// None means the whole incoming payload is already covered. Metadata is
    /// committed separately, after every field of the join has succeeded.
    #[cfg(test)]
    pub(crate) fn join<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        entry: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        self.join_correlated(entry, incoming, domain, budget)
    }

    pub(crate) fn join_correlated<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        entry: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
        budget: &mut WorkBudget,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
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

    pub(crate) fn retain(&mut self, incoming: EvmMachinePayload<N>) {
        if !self.has_loop_bound() {
            self.represented.push(Arc::new(incoming));
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) type EntryAlternatives = EvmEntryAlternatives<crate::domain::NumericValue>;

//! Semantic operations own their prepared values; callers stage publication.

use std::collections::BTreeSet;

use super::{PartitionRefusal, PartitionResult, PolicyKey, RelativePartitionResult, ownership};

use crate::{
    analysis::machine::{EvmEntryAlternatives, EvmMachinePayload, MachineError, MachineKey},
    domain::{AlgebraRef, EvmNumericAlgebra, WordElement, symbolic::ExprId},
    resource::WorkBudget,
    world::EvmEntry,
};

pub use crate::analysis::state::{EvmPayloadSummary as RelativeState, ProcedureOutput};

/// Control boundary at which an input is prepared or admitted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicySite {
    /// Ordinary transaction-wide execution.
    World,
    /// Entry into a caller-independent procedure.
    Procedure,
    /// Basic block within a relative procedure.
    Body,
    /// A body retained under its frozen runtime retry closure after a typed
    /// representation refusal. Admission still owns the resulting semantics.
    RuntimeRetry,
    /// Retained caller continuation awaiting procedure outputs.
    Subscriber,
    /// Return, revert or exceptional procedure output.
    Output,
}

/// Borrowed environment and cumulative ledger for one semantic operation.
pub struct PolicyContext<'a, D: EvmNumericAlgebra> {
    /// The run's frozen numeric algebra; semantic policies do not replace it.
    pub domain: AlgebraRef<'a, D>,
    /// Original entry environment shared by complete and relative execution.
    pub environment: &'a EvmEntry<D::Element>,
    /// Shared cumulative logical work budget.
    pub budget: &'a mut WorkBudget,
}

impl<'a, D: EvmNumericAlgebra> PolicyContext<'a, D> {
    /// Borrow the frozen algebra, environment and ledger for an operation.
    pub fn new(
        domain: AlgebraRef<'a, D>,
        environment: &'a EvmEntry<D::Element>,
        budget: &'a mut WorkBudget,
    ) -> Self {
        Self {
            domain,
            environment,
            budget,
        }
    }

    /// Reserve before doing work; saturated costs and cancellation are refusals.
    /// An operation must check again before publishing its completed result.
    pub fn reserve(&mut self, cost: usize) -> Result<(), MachineError> {
        MachineError::check_cancelled()?;
        if self.budget.cancellation().is_cancelled() {
            return Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled,
            ));
        }
        if cost == usize::MAX || !self.budget.charge(cost) {
            return Err(if self.budget.cancellation().is_cancelled() {
                MachineError::Relations(crate::domain::relational::QueryReason::Cancelled)
            } else {
                MachineError::work_failure()
            });
        }
        MachineError::check_cancelled()?;
        if self.budget.cancellation().is_cancelled() {
            return Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled,
            ));
        }
        Ok(())
    }
}

/// A policy-owned identity and the complete covering input it identifies.
#[derive(Debug)]
pub struct Partitioned<K, N: WordElement> {
    /// Frozen policy's abstract identity.
    pub key: K,
    /// Covering preparation; the engine must execute and retain this input.
    pub payload: EvmMachinePayload<N>,
}

/// A policy-owned identity and covering caller-independent preparation.
#[derive(Debug)]
pub struct PartitionedRelative<K, N: WordElement> {
    /// Frozen policy's abstract identity.
    pub key: K,
    /// Relative input on which that identity and subsequent joins are lawful.
    pub payload: RelativeState<N>,
}

/// Choose actual state identity together with a covering executable preparation.
///
/// A frozen policy must return deterministic keys of finite cardinality under
/// the run's bounded context vocabulary. Preparation must cover the whole input
/// and be idempotent at a site. Equal keys permit grouping only when the prepared
/// representation supports lawful admission of all execution shapes, including
/// code, owners, rollback snapshots and continuations. Unsupported grouping is
/// `MachineError`, never evidence of an unreachable input. The legacy arrival
/// class is an input to this choice and need not occur in the returned key.
/// Finite cardinality does not price key copies or comparisons: the key's
/// independent [`PolicyKey`] quotation must bound those operations, including
/// all heap contents, and remain valid while the key is retained.
pub trait MachinePartition<D: EvmNumericAlgebra, Tag> {
    /// Actual identity used by ordinary, procedure and body maps.
    type Key: PolicyKey;

    /// Prepare a complete machine input before choosing its identity.
    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError>;

    /// Prepare a relative input before choosing its identity.
    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError>;

    /// Prepare an already-owned input, retaining it on every refusal. The
    /// default stages through `prepare`; policies can avoid that copy when
    /// their preparation only selects identity or edits owned metadata.
    #[expect(
        clippy::result_large_err,
        reason = "refusal returns the original input without allocating after work or cancellation refusal"
    )]
    fn prepare_owned(
        &self,
        input: EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> PartitionResult<Self::Key, D::Element> {
        match self.prepare(&input, site, class, cx) {
            Ok(prepared) => Ok(prepared),
            Err(error) => Err(PartitionRefusal { input, error }),
        }
    }

    /// Owned relative preparation with the same covering/refusal contract.
    #[expect(
        clippy::result_large_err,
        reason = "refusal returns the original input without allocating after work or cancellation refusal"
    )]
    fn prepare_relative_owned(
        &self,
        input: RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> RelativePartitionResult<Self::Key, D::Element> {
        match self.prepare_relative(&input, site, class, cx) {
            Ok(prepared) => Ok(prepared),
            Err(error) => Err(PartitionRefusal { input, error }),
        }
    }

    /// Opt into portable exact graph certificates. A portable partition remains
    /// valid under caller-prefix rebinding and fresh-source renaming; admission
    /// preserves its partition invariant. Imported inputs are still prepared
    /// again and require full installed-input equality before being satisfied.
    /// Arbitrary custom keys need make no such promise to execute normally.
    fn supports_graph_replay(&self) -> bool {
        false
    }
}

/// Preserve complete execution shape and the selected legacy arrival class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StructuralPartition;

impl<D: EvmNumericAlgebra, Tag: PolicyKey> MachinePartition<D, Tag> for StructuralPartition {
    type Key = (MachineKey, Tag);

    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        let payload = input.copy_for_policy(cx)?;
        ownership::structural(payload, class, cx).map_err(|refused| refused.error)
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        cx.reserve(input.work_size().saturating_mul(3))?;
        let payload = input.clone();
        ownership::structural_relative(payload, class, cx).map_err(|refused| refused.error)
    }

    fn prepare_owned(
        &self,
        input: EvmMachinePayload<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, PartitionRefusal<EvmMachinePayload<D::Element>>>
    {
        ownership::structural(input, class, cx)
    }

    fn prepare_relative_owned(
        &self,
        input: RelativeState<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<
        PartitionedRelative<Self::Key, D::Element>,
        PartitionRefusal<RelativeState<D::Element>>,
    > {
        ownership::structural_relative(input, class, cx)
    }

    fn supports_graph_replay(&self) -> bool {
        true
    }
}

/// Erase every frame's optional jump context before computing actual identity.
/// Inputs differing only in those histories have the same prepared state/key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ForgetJumpHistory;

impl<D: EvmNumericAlgebra, Tag: PolicyKey> MachinePartition<D, Tag> for ForgetJumpHistory {
    type Key = (MachineKey, Tag);

    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        let payload = input.forget_jump_history(cx)?;
        ownership::structural(payload, class, cx).map_err(|refused| refused.error)
    }

    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        _: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        let payload = input.forget_jump_history(cx)?;
        ownership::structural_relative(payload, class, cx).map_err(|refused| refused.error)
    }

    fn supports_graph_replay(&self) -> bool {
        true
    }
}

/// An admission schedule decision, separate from the covering-join contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Acceleration {
    /// Cover both inputs without applying numerical widening.
    Join,
    /// A covering acceleration may discard additional precision for convergence.
    Widen,
}

/// Immutable scheduling information supplied at a semantic admission boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccelerationInput {
    /// Admission boundary being updated.
    pub site: PolicySite,
    /// The scheduler has established feedback reaching this boundary.
    pub feedback: bool,
    /// Strict semantic updates already published at this boundary.
    pub updates: usize,
    /// Frozen numeric algebra's default acceleration delay.
    pub default_delay: usize,
}

/// Select when to accelerate; this decision alone provides no coverage proof.
/// Frozen policies must choose a convergent schedule for claimed closure, or
/// allow the finite run budget to report its remaining obligations honestly.
pub trait AccelerationPolicy {
    /// Choose a schedule from the supplied immutable boundary information.
    fn decide(&self, input: AccelerationInput) -> Acceleration;
}

/// Accelerate established feedback after the algebra's configured delay.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeedbackAcceleration;

impl AccelerationPolicy for FeedbackAcceleration {
    fn decide(&self, input: AccelerationInput) -> Acceleration {
        if input.feedback && input.updates >= input.default_delay {
            Acceleration::Widen
        } else {
            Acceleration::Join
        }
    }
}

/// Request covering acceleration on every admission, including acyclic growth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EagerAcceleration;

impl AccelerationPolicy for EagerAcceleration {
    fn decide(&self, _: AccelerationInput) -> Acceleration {
        Acceleration::Widen
    }
}

/// Retain covering joins without requesting widening.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoAcceleration;

impl AccelerationPolicy for NoAcceleration {
    fn decide(&self, _: AccelerationInput) -> Acceleration {
        Acceleration::Join
    }
}

/// Own whole-state coverage metadata independently of scalar numeric algebra.
///
/// Admission receives staged metadata. `None` proves the entire incoming state
/// covered and requires no semantic metadata changes. `Some` must cover current
/// and incoming together; only after complete construction may callers publish
/// it and its metadata. A semantic metadata change returns `Some` even if the
/// executable payload is equal, so actual consumers are invalidated.
/// Errors/cancellation never discharge an obligation.
/// A metadata-only representation change must return `Some` even when the
/// resulting payload equals `current`, so the publication wakes its readers.
pub trait WholeStatePolicy<D: EvmNumericAlgebra> {
    /// Retained coverage proof/representation, cloned only for staging.
    type Metadata: Clone;

    /// Retain the first whole input under the shared work budget.
    fn initialize(
        &self,
        input: &EvmMachinePayload<D::Element>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self::Metadata, MachineError>;

    /// Conservative work quote for cloning/comparing retained metadata.
    fn metadata_work(&self, metadata: &Self::Metadata) -> usize;

    /// Whether a covering bound has replaced exact alternative identities.
    /// Callers use this to forget selectors and mark dependencies conservatively.
    fn has_bound(&self, metadata: &Self::Metadata) -> bool;

    /// Stage a whole input and its coverage metadata under the chosen schedule.
    fn admit(
        &self,
        current: &EvmMachinePayload<D::Element>,
        incoming: &EvmMachinePayload<D::Element>,
        metadata: &mut Self::Metadata,
        acceleration: Acceleration,
        _selectors: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError>;

    /// Retain an owned input after successful paid admission and publication.
    fn retain(&self, metadata: &mut Self::Metadata, incoming: EvmMachinePayload<D::Element>);
}

/// Opaque retained complete alternatives or one persistent whole-state bound.
#[derive(Clone)]
pub struct WholeStateMetadata<N: WordElement>(EvmEntryAlternatives<N>);

impl<N: WordElement> WholeStateMetadata<N> {
    /// Pay for and retain the first complete input.
    pub fn initialize<D: EvmNumericAlgebra<Element = N>>(
        input: &EvmMachinePayload<N>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self, MachineError> {
        cx.reserve(input.policy_work_size().saturating_add(1))?;
        input.pay_clone_with(cx.budget)?;
        let metadata = Self(EvmEntryAlternatives::new(input));
        cx.reserve(0)?;
        Ok(metadata)
    }

    /// Conservative retained-state traversal/copy quote.
    pub fn work_size(&self) -> usize {
        self.0.work_size()
    }

    /// Whether exact alternatives were replaced by a covering bound.
    pub fn has_bound(&self) -> bool {
        self.0.has_loop_bound()
    }

    /// Lawful correlated join, with widening only when explicitly requested.
    pub fn admit_correlated<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        current: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        acceleration: Acceleration,
        _selectors: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        cx.reserve(0)?;
        let result = self.0.join_entry(
            current,
            incoming,
            cx.environment,
            cx.domain,
            cx.budget,
            acceleration == Acceleration::Widen,
        );
        cx.reserve(0)?;
        result
    }

    /// Eagerly admit the reusable bounded whole-state representation.
    pub fn admit_bounded<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        current: &EvmMachinePayload<N>,
        incoming: &EvmMachinePayload<N>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<N>>, MachineError> {
        cx.reserve(0)?;
        let result = self.0.join_loop_scheduled(
            current,
            incoming,
            cx.environment,
            cx.domain,
            cx.budget,
            acceleration == Acceleration::Widen,
        );
        cx.reserve(0)?;
        result
    }

    /// Retain a paid owned input after successful admission/publication.
    pub fn retain(&mut self, incoming: EvmMachinePayload<N>) {
        self.0.retain(incoming);
    }
}

/// Preserve whole-input correlations until an explicit bound or typed refusal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CorrelatedJoin;

/// Use a bounded whole-state representation on the first merging admission.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoundedJoin;

impl<D: EvmNumericAlgebra> WholeStatePolicy<D> for CorrelatedJoin {
    type Metadata = WholeStateMetadata<D::Element>;

    fn initialize(
        &self,
        input: &EvmMachinePayload<D::Element>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self::Metadata, MachineError> {
        WholeStateMetadata::initialize(input, cx)
    }
    fn metadata_work(&self, metadata: &Self::Metadata) -> usize {
        metadata.work_size()
    }
    fn has_bound(&self, metadata: &Self::Metadata) -> bool {
        metadata.has_bound()
    }
    fn admit(
        &self,
        current: &EvmMachinePayload<D::Element>,
        incoming: &EvmMachinePayload<D::Element>,
        metadata: &mut Self::Metadata,
        acceleration: Acceleration,
        selectors: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        metadata.admit_correlated(current, incoming, acceleration, selectors, cx)
    }
    fn retain(&self, metadata: &mut Self::Metadata, incoming: EvmMachinePayload<D::Element>) {
        metadata.retain(incoming);
    }
}

impl<D: EvmNumericAlgebra> WholeStatePolicy<D> for BoundedJoin {
    type Metadata = WholeStateMetadata<D::Element>;

    fn initialize(
        &self,
        input: &EvmMachinePayload<D::Element>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self::Metadata, MachineError> {
        WholeStateMetadata::initialize(input, cx)
    }
    fn metadata_work(&self, metadata: &Self::Metadata) -> usize {
        metadata.work_size()
    }
    fn has_bound(&self, metadata: &Self::Metadata) -> bool {
        metadata.has_bound()
    }
    fn admit(
        &self,
        current: &EvmMachinePayload<D::Element>,
        incoming: &EvmMachinePayload<D::Element>,
        metadata: &mut Self::Metadata,
        acceleration: Acceleration,
        _: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        metadata.admit_bounded(current, incoming, acceleration, cx)
    }
    fn retain(&self, metadata: &mut Self::Metadata, incoming: EvmMachinePayload<D::Element>) {
        metadata.retain(incoming);
    }
}

/// Cover caller-independent relative inputs and correlated terminal projections.
/// All paths, including the first output and runtime retry, require admission.
/// Results cover both inputs and retain justified execution shapes; refusals
/// preserve the caller's pending obligation. The caller stages publication.
pub trait ProcedureSummaryPolicy<D: EvmNumericAlgebra> {
    /// Select runtime interpretation before detailed execution spends the ledger.
    /// This frozen choice never changes the original code kind or source.
    fn runtime_execution(&self) -> crate::analysis::RuntimeExecution {
        crate::analysis::RuntimeExecution::Native
    }

    /// Admit known callees into the relative solver before active recursion is
    /// discovered. The engine still owns body execution evidence and global
    /// closure obligations; this capability changes routing, not their proof.
    fn supports_early_known_admission(&self) -> bool {
        false
    }

    /// Cover a relative body/subscriber input under the selected schedule.
    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError>;

    /// Cover an output even when there is no previously published output.
    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError>;
}

/// Preserve finite projection guarantees while joining procedure summaries.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProjectedSummaries;

impl<D: EvmNumericAlgebra> ProcedureSummaryPolicy<D> for ProjectedSummaries {
    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        if site == PolicySite::RuntimeRetry {
            current.bounded_join(incoming, acceleration, cx)
        } else {
            current.covering_join(incoming, acceleration, cx)
        }
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        if let Some(current) = current {
            current.covering_join(incoming, acceleration, cx)
        } else {
            cx.reserve(incoming.work_size())?;
            let output = incoming.clone();
            cx.reserve(0)?;
            Ok(output)
        }
    }
}

/// Discard cross-value aliases/observations and bound byte/store representations.
/// This changes retained summaries independently of numeric widening decisions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoarseSummaries;

impl<D: EvmNumericAlgebra> ProcedureSummaryPolicy<D> for CoarseSummaries {
    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        _: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        current.bounded_join(incoming, acceleration, cx)
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        if let Some(current) = current {
            current.bounded_join(incoming, acceleration, cx)
        } else {
            incoming.coarsen(cx)
        }
    }
}

/// Freeze relative/output coordinates while retaining native scalar control
/// facts and common stack aliases. Runtime retries keep their established
/// coarse invariant and can therefore discard those facts after a refusal.
/// The constructors select call admission timing using the same representation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegionalSummaries {
    early_known_admission: bool,
}

impl RegionalSummaries {
    /// Use regional summaries with the existing unknown/recursive call routing.
    pub const fn new() -> Self {
        Self {
            early_known_admission: false,
        }
    }

    /// Use the same regional representation for known callees from their first
    /// admission. The engine must retain their actual body execution evidence.
    pub const fn with_early_known_admission() -> Self {
        Self {
            early_known_admission: true,
        }
    }
}

impl<D: EvmNumericAlgebra> ProcedureSummaryPolicy<D> for RegionalSummaries {
    fn supports_early_known_admission(&self) -> bool {
        self.early_known_admission
    }

    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        if site == PolicySite::RuntimeRetry {
            current.bounded_join(incoming, acceleration, cx)
        } else {
            current.regional_join(incoming, acceleration, cx)
        }
    }

    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        current
            .unwrap_or(incoming)
            .regional_join(incoming, acceleration, cx)
    }
}

/// Explicit opt-in whole-runtime process checkpoint. Detailed execution remains
/// the default provider; uncovered process rules remain typed frontiers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KnownProcessSummaries;

impl<D: EvmNumericAlgebra> ProcedureSummaryPolicy<D> for KnownProcessSummaries {
    fn runtime_execution(&self) -> crate::analysis::RuntimeExecution {
        crate::analysis::RuntimeExecution::Process
    }
    fn supports_early_known_admission(&self) -> bool {
        true
    }
    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        _site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        current.regional_join(incoming, acceleration, cx)
    }
    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        current
            .unwrap_or(incoming)
            .regional_join(incoming, acceleration, cx)
    }
}

/// An owned assembly whose independent components may borrow caller data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SemanticPolicies<
    K = StructuralPartition,
    J = CorrelatedJoin,
    A = FeedbackAcceleration,
    R = ProjectedSummaries,
> {
    /// Actual machine identity and covering preparation.
    pub partition: K,
    /// Whole-machine coverage and retained metadata.
    pub join: J,
    /// Independent acceleration schedule.
    pub acceleration: A,
    /// Relative and output summary representation/admission.
    pub summary: R,
}

impl<K, J, A, R> SemanticPolicies<K, J, A, R> {
    /// Compose semantic components without changing interpreter dispatch.
    pub const fn new(partition: K, join: J, acceleration: A, summary: R) -> Self {
        Self {
            partition,
            join,
            acceleration,
            summary,
        }
    }
}

impl Default for SemanticPolicies {
    fn default() -> Self {
        Self::new(
            StructuralPartition,
            CorrelatedJoin,
            FeedbackAcceleration,
            ProjectedSummaries,
        )
    }
}

impl<D, Tag, K, J, A, R> MachinePartition<D, Tag> for SemanticPolicies<K, J, A, R>
where
    D: EvmNumericAlgebra,
    K: MachinePartition<D, Tag>,
{
    type Key = K::Key;
    fn prepare(
        &self,
        input: &EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, MachineError> {
        self.partition.prepare(input, site, class, cx)
    }
    fn prepare_relative(
        &self,
        input: &RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<PartitionedRelative<Self::Key, D::Element>, MachineError> {
        self.partition.prepare_relative(input, site, class, cx)
    }
    fn prepare_owned(
        &self,
        input: EvmMachinePayload<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Partitioned<Self::Key, D::Element>, PartitionRefusal<EvmMachinePayload<D::Element>>>
    {
        self.partition.prepare_owned(input, site, class, cx)
    }
    fn prepare_relative_owned(
        &self,
        input: RelativeState<D::Element>,
        site: PolicySite,
        class: Tag,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<
        PartitionedRelative<Self::Key, D::Element>,
        PartitionRefusal<RelativeState<D::Element>>,
    > {
        self.partition
            .prepare_relative_owned(input, site, class, cx)
    }
    fn supports_graph_replay(&self) -> bool {
        self.partition.supports_graph_replay()
    }
}

impl<D, K, J, A, R> WholeStatePolicy<D> for SemanticPolicies<K, J, A, R>
where
    D: EvmNumericAlgebra,
    J: WholeStatePolicy<D>,
{
    type Metadata = J::Metadata;
    fn initialize(
        &self,
        input: &EvmMachinePayload<D::Element>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Self::Metadata, MachineError> {
        self.join.initialize(input, cx)
    }
    fn metadata_work(&self, metadata: &Self::Metadata) -> usize {
        self.join.metadata_work(metadata)
    }
    fn has_bound(&self, metadata: &Self::Metadata) -> bool {
        self.join.has_bound(metadata)
    }
    fn admit(
        &self,
        current: &EvmMachinePayload<D::Element>,
        incoming: &EvmMachinePayload<D::Element>,
        metadata: &mut Self::Metadata,
        acceleration: Acceleration,
        selectors: &BTreeSet<ExprId>,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<Option<EvmMachinePayload<D::Element>>, MachineError> {
        self.join
            .admit(current, incoming, metadata, acceleration, selectors, cx)
    }
    fn retain(&self, metadata: &mut Self::Metadata, incoming: EvmMachinePayload<D::Element>) {
        self.join.retain(metadata, incoming);
    }
}

impl<K, J, A: AccelerationPolicy, R> AccelerationPolicy for SemanticPolicies<K, J, A, R> {
    fn decide(&self, input: AccelerationInput) -> Acceleration {
        self.acceleration.decide(input)
    }
}

impl<D, K, J, A, R> ProcedureSummaryPolicy<D> for SemanticPolicies<K, J, A, R>
where
    D: EvmNumericAlgebra,
    R: ProcedureSummaryPolicy<D>,
{
    fn runtime_execution(&self) -> crate::analysis::RuntimeExecution {
        self.summary.runtime_execution()
    }
    fn supports_early_known_admission(&self) -> bool {
        self.summary.supports_early_known_admission()
    }

    fn admit_relative(
        &self,
        current: &RelativeState<D::Element>,
        incoming: &RelativeState<D::Element>,
        site: PolicySite,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<RelativeState<D::Element>, MachineError> {
        self.summary
            .admit_relative(current, incoming, site, acceleration, cx)
    }
    fn admit_output(
        &self,
        current: Option<&ProcedureOutput<D::Element>>,
        incoming: &ProcedureOutput<D::Element>,
        acceleration: Acceleration,
        cx: &mut PolicyContext<'_, D>,
    ) -> Result<ProcedureOutput<D::Element>, MachineError> {
        self.summary
            .admit_output(current, incoming, acceleration, cx)
    }
}

/// Aggregate bound for the four independent semantic protocols.
pub trait StatePolicy<D: EvmNumericAlgebra, Tag>:
    MachinePartition<D, Tag> + WholeStatePolicy<D> + AccelerationPolicy + ProcedureSummaryPolicy<D>
{
}

impl<D, Tag, S> StatePolicy<D, Tag> for S
where
    D: EvmNumericAlgebra,
    S: MachinePartition<D, Tag>
        + WholeStatePolicy<D>
        + AccelerationPolicy
        + ProcedureSummaryPolicy<D>,
{
}

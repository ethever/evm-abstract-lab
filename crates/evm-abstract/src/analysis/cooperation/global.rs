//! Shared, tracked observations of the interpreter's discovered program graph.
//!
//! Adjacency is an open discovery set, never a closed-world must proof. Native
//! inputs and summaries are sound bounds under their published assumptions;
//! missing providers and incomplete discovery do not establish unreachability.

use super::{
    ImmutableInputPolicy, Inputs, Point, ProcessMeet, ProcessRefinements, Refinements, StackMeet,
    navigation::{Callees, DescriptorPolicy, ProcedureEntries},
};
use crate::{
    analysis::{MachineEdgeKind, OutcomeKind, state::EvmPayloadSummary},
    domain::{EvmValue, WordElement},
    world::{EvmByteArraySummary, EvmStoreSummary},
};
use analysis_kernel::{
    Ascending, Capability, Channel, Descending, GenerationEntry, JoinPolicy, Kernel, KernelError,
    Meter, algebra::PublicationPolicy,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    marker::PhantomData,
    sync::Arc,
};

/// Stable relative-procedure identity, independent of a particular input scope.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProcedureId {
    /// Run-local relative table namespace.
    pub namespace: u64,
    /// Structural procedure index in that table.
    pub index: usize,
}

/// Stable identity of a caller continuation subscribed to a procedure output.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ContinuationId {
    /// Run-local relative table namespace.
    pub namespace: u64,
    /// Structural subscription index in that table.
    pub index: usize,
}

/// A discovered interpreter transition, including already-covered arrivals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    /// Structural source point.
    pub from: Point,
    /// Structural target point.
    pub to: Point,
    /// Native EVM transition classification.
    pub kind: MachineEdgeKind,
}

/// Which open adjacency set to observe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    /// Transitions entering this point.
    Predecessors,
    /// Transitions leaving this point.
    Successors,
}

/// A tracked per-point adjacency query. An empty value is known open discovery;
/// Pending means that the point has not yet been published on this route.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Neighbors {
    /// Structural point, including its relative-table namespace.
    pub point: Point,
    /// Incoming or outgoing transitions.
    pub direction: Direction,
}

/// All currently published points. The set may grow until the joint run closes.
pub struct Points;
impl Capability for Points {
    type Key = ();
    type Value = BTreeSet<Point>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_mul(4).saturating_add(1)
    }
}

/// Open directory of procedure keys accepted by [`Summaries`]. A ready empty
/// set means no procedure has been discovered yet, not that no call can occur.
pub struct Procedures;
impl Capability for Procedures {
    type Key = ();
    type Value = BTreeSet<ProcedureId>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_mul(2).saturating_add(1)
    }
}

/// Open directory of continuation keys accepted by [`Continuations`]. Read the
/// corresponding observation for its public caller point and callee procedure.
pub struct Subscriptions;
impl Capability for Subscriptions {
    type Key = ();
    type Value = BTreeSet<ContinuationId>;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_mul(2).saturating_add(1)
    }
}

/// Stable generation selectors, indexed by structural point in the global scope.
/// Resolve the handle with `ReadGeneration::current_generation` before querying
/// Inputs or Refinements; keeping a previous Subject is not a live selector.
pub struct Generations;
impl Capability for Generations {
    type Key = Point;
    type Value = GenerationEntry;
    fn key_work(_: &Point) -> usize {
        4
    }
    fn value_work(_: &GenerationEntry) -> usize {
        4
    }
}

/// Open predecessor/successor discovery. Every new transition publishes here,
/// even when admitting its native payload leaves the target input unchanged.
pub struct Topology;
impl Capability for Topology {
    type Key = Neighbors;
    type Value = BTreeSet<Edge>;
    fn key_work(_: &Neighbors) -> usize {
        5
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_mul(10).saturating_add(1)
    }
}

/// Actual ascending native return bound for one procedure outcome kind.
#[derive(Clone, Debug)]
pub struct SummaryOutput<N: WordElement> {
    /// Native return-data summary, retaining the run's numeric carrier.
    pub data: EvmByteArraySummary<N>,
    /// Native store summary, including rollback-sensitive effects.
    pub store: EvmStoreSummary<N>,
    /// Ascending version of this outcome kind.
    pub generation: usize,
}

/// Incremental native MAY snapshot. Missing outcome kinds are undiscovered,
/// never proved impossible. Adopt [`SummaryClosures`] through `closed_summary`
/// before using this snapshot as a complete bound for refinement.
#[derive(Clone, Debug)]
pub struct Summary<N: WordElement> {
    /// Monotone native snapshot observation revision.
    pub revision: usize,
    /// Native output-bound revision; completion alone does not change this.
    pub generation: usize,
    /// Native outcomes published so far.
    pub outputs: BTreeMap<OutcomeKind, SummaryOutput<N>>,
    retained_work: usize,
}
impl<N: WordElement> Summary<N> {
    pub(crate) fn new(
        revision: usize,
        outputs: BTreeMap<OutcomeKind, SummaryOutput<N>>,
        retained_work: usize,
    ) -> Self {
        Self {
            revision,
            generation: revision,
            outputs,
            retained_work,
        }
    }
    /// Cached retained-payload disposal quote, independent of snapshot size.
    pub fn work_size(&self) -> usize {
        self.retained_work
    }
}

/// Typed globally visible native procedure-output observations.
pub struct Summaries<N: WordElement>(PhantomData<N>);
impl<N: WordElement> Capability for Summaries<N> {
    type Key = ProcedureId;
    type Value = Arc<Summary<N>>;
    fn key_work(_: &ProcedureId) -> usize {
        2
    }
    fn value_work(_: &Self::Value) -> usize {
        2
    }
    fn drop_work(value: &Self::Value) -> usize {
        value.work_size()
    }
}

/// Certificate that the matching native output generation is a complete bound.
/// The kernel also requires the selected table proof's current source validity;
/// retaining this marker alone never retains that validity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SummaryClosure {
    /// Native output generation certified by the producing table proof.
    pub generation: usize,
}

/// Completion certificates, separate from incremental [`Summaries`]. Pending
/// means that a complete bound is not currently established. Only a required
/// read of this route adopts its source premises; discovery is observational.
pub struct SummaryClosures;
impl Capability for SummaryClosures {
    type Key = ProcedureId;
    type Value = SummaryClosure;
    fn key_work(_: &ProcedureId) -> usize {
        2
    }
    fn value_work(_: &SummaryClosure) -> usize {
        1
    }
}

/// A caller template dependency, including its actual ascending stack/store
/// bound. The private template preserves relative frame and continuation data.
#[derive(Clone, Debug)]
pub struct Continuation<N: WordElement> {
    /// Ascending caller-template revision.
    pub revision: usize,
    /// Caller body point whose result consumes the callee output.
    pub caller: Point,
    /// Procedure supplying the output.
    pub callee: ProcedureId,
    payload: Arc<EvmPayloadSummary<N>>,
    retained_work: usize,
}
impl<N: WordElement> Continuation<N> {
    pub(in crate::analysis) fn new(
        revision: usize,
        caller: Point,
        callee: ProcedureId,
        payload: EvmPayloadSummary<N>,
        retained_work: usize,
    ) -> Self {
        Self {
            revision,
            caller,
            callee,
            payload: Arc::new(payload),
            retained_work,
        }
    }
    /// Active native template stack, before applying any local narrowing.
    pub fn stack(&self) -> &[EvmValue<N>] {
        &self.payload.active().stack
    }
    /// Actual native transaction-store bound in the retained caller template.
    pub fn store(&self) -> &EvmStoreSummary<N> {
        &self.payload.store
    }
    /// Conservative traversal charge for the native template.
    pub fn work_size(&self) -> usize {
        self.retained_work
    }
}

/// Globally visible caller-template and callee-output dependency observations.
pub struct Continuations<N: WordElement>(PhantomData<N>);
impl<N: WordElement> Capability for Continuations<N> {
    type Key = ContinuationId;
    type Value = Arc<Continuation<N>>;
    fn key_work(_: &ContinuationId) -> usize {
        2
    }
    fn value_work(_: &Self::Value) -> usize {
        2
    }
    fn drop_work(value: &Self::Value) -> usize {
        value.work_size()
    }
}

/// Union policy for the interpreter's ascending discovery sets.
pub struct SetUnion<T>(PhantomData<T>);
impl<T> Default for SetUnion<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<T: Clone + Ord> JoinPolicy for SetUnion<T> {
    type Value = BTreeSet<T>;
    type Error = Infallible;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        old.len()
            .saturating_add(incoming.len())
            .saturating_add(1)
            .saturating_mul(((usize::BITS as usize) + 1) * 16)
    }
    fn join(&self, old: &Self::Value, incoming: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(old.union(incoming).cloned().collect())
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        left == right
    }
}

/// Single-assignment stable selector handles. Replacing the selected Scope is
/// performed only through the kernel's atomic generation replacement protocol.
pub struct EntryPolicy;
impl PublicationPolicy for EntryPolicy {
    type Value = GenerationEntry;
    type Error = Infallible;
    const DESCENDING: bool = false;
    fn work(&self, _: &GenerationEntry, _: &GenerationEntry) -> usize {
        8
    }
    fn combine(
        &self,
        old: &GenerationEntry,
        _: &GenerationEntry,
    ) -> Result<GenerationEntry, Infallible> {
        Ok(old.clone())
    }
    fn equivalent(&self, left: &GenerationEntry, right: &GenerationEntry) -> bool {
        left == right
    }
}

/// A versioned native observation whose semantic bound is owned by the engine.
pub trait Observation: Clone {
    /// Monotone observation revision; equal revisions denote identical bounds.
    fn revision(&self) -> usize;
    /// Cached disposal quote including last-handle native payload release.
    fn retained_work(&self) -> usize;
}
impl<N: WordElement> Observation for Arc<Summary<N>> {
    fn revision(&self) -> usize {
        self.revision
    }
    fn retained_work(&self) -> usize {
        self.work_size()
    }
}
impl<N: WordElement> Observation for Arc<Continuation<N>> {
    fn revision(&self) -> usize {
        self.revision
    }
    fn retained_work(&self) -> usize {
        self.work_size()
    }
}
impl Observation for SummaryClosure {
    fn revision(&self) -> usize {
        self.generation
    }
    fn retained_work(&self) -> usize {
        1
    }
}

/// Retain only the latest already-joined native observation. The interpreter
/// establishes coverage before publication; no historical snapshots are stored.
pub struct ObservationPolicy<T>(PhantomData<T>);
impl<T> Default for ObservationPolicy<T> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<T: Observation> JoinPolicy for ObservationPolicy<T> {
    type Value = T;
    type Error = Infallible;
    fn work(&self, old: &T, incoming: &T) -> usize {
        old.retained_work()
            .saturating_add(incoming.retained_work())
            .saturating_add(8)
    }
    fn join(&self, old: &T, incoming: &T) -> Result<T, Infallible> {
        Ok(if incoming.revision() > old.revision() {
            incoming.clone()
        } else {
            old.clone()
        })
    }
    fn equivalent(&self, left: &T, right: &T) -> bool {
        left.revision() == right.revision()
    }
}

/// Retain observations of the currently computed MAY bounds and wake readers on
/// growth. A snapshot proves neither completeness nor concrete witness existence;
/// a complete bound requires the separately guarded completion certificate.
pub struct SummaryPolicy<N: WordElement>(PhantomData<N>);
impl<N: WordElement> Default for SummaryPolicy<N> {
    fn default() -> Self {
        Self(PhantomData)
    }
}
impl<N: WordElement> PublicationPolicy for SummaryPolicy<N> {
    type Value = Arc<Summary<N>>;
    type Error = Infallible;
    const DESCENDING: bool = false;

    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        ObservationPolicy::default().work(old, incoming)
    }
    fn combine(
        &self,
        old: &Self::Value,
        incoming: &Self::Value,
    ) -> Result<Self::Value, Infallible> {
        ObservationPolicy::default().join(old, incoming)
    }
    fn equivalent(&self, old: &Self::Value, next: &Self::Value) -> bool {
        old.revision == next.revision
    }
    fn preserves_guarantees(&self, _: &Self::Value, _: &Self::Value) -> bool {
        true
    }
}

type SummaryChannel<N> = Channel<Summaries<N>, SummaryPolicy<N>>;
type ContinuationChannel<N> =
    Channel<Continuations<N>, Ascending<ObservationPolicy<Arc<Continuation<N>>>>>;

pub(crate) struct Shared<N: WordElement> {
    pub inputs: Channel<Inputs<N>, ImmutableInputPolicy<N>>,
    pub refinements: Channel<Refinements, Descending<StackMeet>>,
    pub process_refinements: Channel<ProcessRefinements, Descending<ProcessMeet>>,
    pub points: Channel<Points, Ascending<SetUnion<Point>>>,
    pub generations: Channel<Generations, EntryPolicy>,
    pub callees: Channel<Callees, Ascending<SetUnion<ProcedureId>>>,
    pub entries: Channel<ProcedureEntries, Ascending<DescriptorPolicy>>,
    pub procedures: Channel<Procedures, Ascending<SetUnion<ProcedureId>>>,
    pub subscriptions: Channel<Subscriptions, Ascending<SetUnion<ContinuationId>>>,
    pub topology: Channel<Topology, Ascending<SetUnion<Edge>>>,
    pub summaries: SummaryChannel<N>,
    pub closures: Channel<SummaryClosures, Ascending<ObservationPolicy<SummaryClosure>>>,
    pub continuations: ContinuationChannel<N>,
}
impl<N: WordElement> Shared<N> {
    pub fn new<J, M: Meter>(
        kernel: &mut Kernel<J>,
        meter: &mut M,
    ) -> Result<Self, KernelError<M::Stop>> {
        Ok(Self {
            inputs: kernel.channel(ImmutableInputPolicy::default(), meter)?,
            refinements: kernel.channel(Descending(StackMeet), meter)?,
            process_refinements: kernel.channel(Descending(ProcessMeet), meter)?,
            points: kernel.channel(Ascending(SetUnion::default()), meter)?,
            generations: kernel.channel(EntryPolicy, meter)?,
            callees: kernel.channel(Ascending(SetUnion::default()), meter)?,
            entries: kernel.channel(Ascending(DescriptorPolicy), meter)?,
            procedures: kernel.channel(Ascending(SetUnion::default()), meter)?,
            subscriptions: kernel.channel(Ascending(SetUnion::default()), meter)?,
            topology: kernel.channel(Ascending(SetUnion::default()), meter)?,
            summaries: kernel.channel(SummaryPolicy::default(), meter)?,
            closures: kernel.channel(Ascending(ObservationPolicy::default()), meter)?,
            continuations: kernel.channel(Ascending(ObservationPolicy::default()), meter)?,
        })
    }
}

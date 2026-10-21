//! Independent analyzers participating in the production EVM fixed point.
//!
//! Components use the kernel's [`Analyzer`](analysis_kernel::Analyzer), typed
//! [`Read`](analysis_kernel::Read), [`Accepts`](analysis_kernel::Accepts) and
//! [`Registered`](analysis_kernel::Registered) protocols. Each [`Subject`] names
//! one immutable set of scoped analysis assumptions and reaching executions.
//! Registered stack facts must hold for every concrete execution reaching that
//! subject under those assumptions. They may be independently proved invariants
//! absent from the EVM carrier; the published payload is only the EVM component's
//! bound. Applying them preserves the joint product's concrete coverage and
//! refines a detached execution view, never its global ascending anchor. Wider
//! inputs, including summary-induced growth, require a fresh seeded generation
//! and retirement of the previous local facts. Registration accepts this trusted
//! semantic contract; it is not a Rust proof that an arbitrary rule is sound.
//!
//! [`ExtraStores`] adds independently defined capability families after the two
//! builtin routes. Each factory uses the same kernel/meter and creates fresh
//! channels for that input universe. Client providers and consumers select their
//! own static routes with the kernel's Cons/Here/There vocabulary; no capability
//! type registry or alternate solver is involved. [`SharedStores`] supplies a
//! separate once-per-run client tail selected with [`SharedRoute`]; querying a
//! foreign Scope never switches a generation-owned ExtraStores tail.
//!
//! [`current_input`] resolves a public point through [`Generations`] and the
//! kernel's tracked current-generation selector, then reads the shared native
//! Inputs channel. Retain the structural Point for later queries, rather than
//! using an old Subject as a live selector. Native slot growth and adopted
//! foreign assumption growth retire dependent local scopes before their old
//! guarantees can be consumed again. Same-universe descending improvements and
//! selector movement wake readers without inventing a larger universe.
//!
//! [`Points`], [`Topology`], [`Procedures`] and [`Subscriptions`] are open discovery
//! observations. Value, ready-empty, Pending and explicit absence have distinct
//! meanings; an unfinished discovery set is never a completeness certificate.
//! Sound feedback either follows an independently valid invariant or covers all
//! executions in its declared frozen contributor universe. In particular, a
//! cyclic rule cannot use a missing predecessor/provider as a proof that an
//! undiscovered feasible path is absent. Native summary bounds and continuation
//! templates expose payloads and revisions separately from completion metadata.

//!
//! [`Summaries`] exposes incremental MAY observations. Required readers wait for
//! the first snapshot and wake on growth, without adopting a completion proof.
//! Missing outcomes cannot justify exclusion. [`closed_summary`] requires the
//! separately guarded [`SummaryClosures`] certificate and matching native output
//! generation before returning a complete bound. [`discover_closed_summary`]
//! supports optional precision with an independently valid fallback.

mod composition;
mod entry;
mod error;
mod facts;
mod global;
mod input;
mod navigation;
mod ports;
mod query;
mod refinement;

pub use composition::{
    Extension, ExtraStores, InputTasks, NoExtension, NoStores, NoTasks, PairedTasks, PerInput,
    SharedStores,
};
pub use entry::{
    analyze_world_cooperating, analyze_world_cooperating_with_control,
    analyze_world_cooperating_with_observer,
};
pub use error::CooperationError;
pub use facts::{Refinements, StackFacts, StackMeet};
pub use global::Topology;
pub use global::{
    Continuation, ContinuationId, Continuations, Direction, Edge, Generations, Neighbors, Points,
    ProcedureId, Procedures, Subscriptions, Summaries, Summary, SummaryClosure, SummaryClosures,
    SummaryOutput,
};
pub use input::{
    ImmutableInputPolicy, Input, InputAlreadyInitialized, Inputs, Point, PointKind, Subject,
};
pub use navigation::{Callees, ProcedureEntries, ProcedureInfo};
pub use ports::{
    CalleeRoute, ContinuationRoute, GenerationRoute, PointsRoute, Ports, ProcedureEntryRoute,
    ProcedureRoute, SharedRoute, SubscriptionRoute, SummaryClosureRoute, SummaryRoute,
    TopologyRoute,
};
pub use query::{
    ClosedSummaryResult, SummaryReadError, closed_summary, current_input, discover_closed_summary,
    discover_input,
};
pub use refinement::{InitializationError, LocalInput, RefinementError};

use analysis_kernel::{Context, Here, Nil, There};

pub use super::worklist::{MachineMeter as EvmMeter, runtime::Task as EvmTask};

/// Explicit route of the immutable production input capability.
pub type InputRoute = Here;
/// Explicit route of generation-local entailed stack guarantees.
pub type RefinementRoute = There<Here>;

/// The kernel's actual static capability context, parameterized over the host
/// task and meter. `Registered` retains its original sealed authority protocol.
pub type PortsContext<'r, 'k, J, M, N, Tail = Nil, Global = Nil> =
    Context<'r, 'k, J, M, Ports<'r, N, Tail, Global>>;

/// Actual production capability context. It uses the engine's shared task sum
/// and original root ledger; independently authored components can instead be
/// generic over only their required Read/Accepts/Charges capabilities.
pub type EvmContext<'r, 'k, 'm, N, T, Tail = Nil, Global = Nil> =
    PortsContext<'r, 'k, EvmTask<T>, EvmMeter<'m>, N, Tail, Global>;

pub(crate) use global::Shared;
pub(crate) use refinement::{
    apply_refinements, initialize, initialize_feedback, initialize_readonly, read_refinements,
};

#[cfg(test)]
mod tests;

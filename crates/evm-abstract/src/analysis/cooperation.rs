//! Independent analyzers participating in the production EVM fixed point.
//!
//! Components use the kernel's [`Analyzer`](analysis_kernel::Analyzer), typed
//! [`Read`](analysis_kernel::Read), [`Accepts`](analysis_kernel::Accepts) and
//! [`Registered`](analysis_kernel::Registered) protocols. Each [`Subject`] names
//! one immutable EVM input universe. Registered stack facts must be consequences
//! of that exact input; they refine a detached execution view, never its global
//! ascending anchor. Wider inputs, including summary-induced growth, require a
//! fresh seeded generation and retirement of the previous local facts.
//!
//! [`ExtraStores`] adds independently defined capability families after the two
//! builtin routes. Each factory uses the same kernel/meter and creates fresh
//! channels for that input universe. Client providers and consumers select their
//! own static routes with the kernel's Cons/Here/There vocabulary; no capability
//! type registry or alternate solver is involved.

mod composition;
mod entry;
mod error;
mod facts;
mod input;
mod refinement;

pub use composition::{
    Extension, ExtraStores, InputTasks, NoExtension, NoStores, NoTasks, PairedTasks, PerInput,
};
pub use entry::{
    analyze_world_cooperating, analyze_world_cooperating_with_control,
    analyze_world_cooperating_with_observer,
};
pub use error::CooperationError;
pub use facts::{Refinements, StackFacts, StackMeet};
pub use input::{
    ImmutableInputPolicy, Input, InputAlreadyInitialized, Inputs, Point, PointKind, Subject,
};
pub use refinement::{InitializationError, LocalInput, RefinementError};

use analysis_kernel::{Channel, Cons, Context, Descending, Here, Nil, There};

pub use super::worklist::{MachineMeter as EvmMeter, runtime::Task as EvmTask};

/// Explicit route of the immutable production input capability.
pub type InputRoute = Here;
/// Explicit route of generation-local entailed stack guarantees.
pub type RefinementRoute = There<Here>;

/// Typed stores shared by interpreter consumers and independent components.
/// Input cells are initialized before extension jobs can run. The second route
/// requires explicit registered initialization and reduction authority.
pub type Ports<N, Tail = Nil> = Cons<
    Channel<Inputs<N>, ImmutableInputPolicy<N>>,
    Cons<Channel<Refinements, Descending<StackMeet>>, Tail>,
>;

/// The kernel's actual static capability context, parameterized over the host
/// task and meter. `Registered` retains its original sealed authority protocol.
pub type PortsContext<'r, 'k, J, M, N, Tail = Nil> = Context<'r, 'k, J, M, Ports<N, Tail>>;

/// Actual production capability context. It uses the engine's shared task sum
/// and original root ledger; independently authored components can instead be
/// generic over only their required Read/Accepts/Charges capabilities.
pub type EvmContext<'r, 'k, 'm, N, T, Tail = Nil> =
    PortsContext<'r, 'k, EvmTask<T>, EvmMeter<'m>, N, Tail>;

pub(crate) use refinement::{apply_refinements, create_ports, initialize, read_refinements};

#[cfg(test)]
mod tests;

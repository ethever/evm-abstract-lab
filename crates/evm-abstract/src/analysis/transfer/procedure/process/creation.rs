//! Constructor/deployment rules for a process parent, without concrete addresses.
//!
//! The owner adapter supplies a versioned covering partition and executes these
//! plans in the existing Body/Resume table. A plan is not a closed constructor
//! summary: every child action, output and rollback dependency must be current.
//! Native parents still require an address-valued continuation implementation.

use crate::{
    analysis::{ProcessAction, ProcessOwner},
    resource::WorkBudget,
};
use alloy_primitives::B256;

#[path = "creation/completion.rs"]
mod completion;
#[path = "creation/prepare.rs"]
mod prepare;

pub use completion::{
    CompletionCase, CompletionReason, CreationResult, Deployment, ReturnedData, StoreDisposition,
    complete, runtime_is_valid,
};
pub use prepare::{CreationPlan, ParentException, Rejection, RejectionReason, prepare};

/// Both alternatives are retained when the EVM guard is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Admission {
    /// The successful guard is known to hold.
    Allowed,
    /// The successful guard is known not to hold.
    Rejected,
    /// Both the accepted and rejected guard are possible.
    Unknown,
}

impl Admission {
    fn may_allow(self) -> bool {
        self != Self::Rejected
    }

    fn may_reject(self) -> bool {
        self != Self::Allowed
    }
}

/// Creator nonce admission, independent of CREATE2's address derivation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum CreatorNonce {
    /// The exact protocol nonce, including the rejecting u64::MAX case.
    Exact(u64),
    /// Every protocol nonce; no hash/address is computed from a representative.
    Unknown,
}

/// Constructor source identity is independent of account runtime resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ConstructorCode {
    /// Known initcode retains its actual hash and InitCode origin.
    CapturedInitCode(B256),
    /// Every possible initcode, with no fabricated Program or runtime hash.
    AnyInitCode,
}

/// The owner adapter must use the active storage/logical owner as creator.
/// In particular, DELEGATECALL's code address is not the creator's address.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct CreationRequest {
    /// Only Create and Create2 are accepted.
    pub action: ProcessAction,
    /// Exact creator or its covering owner class; classes may alias.
    pub creator: ProcessOwner,
    /// A static CREATE faults this frame; it does not return zero and continue.
    pub is_static: bool,
    /// The EVM depth limit, separately from analysis depth/work refusal.
    pub depth: Admission,
    /// Creator nonce before increment; CREATE2 increments it too.
    pub nonce: CreatorNonce,
    /// Whether the actual endowment is affordable, with checked arithmetic.
    pub funding: Admission,
    /// Whether memory-selected initcode length is at most 49,152 bytes.
    pub initcode_size: Admission,
    /// Supplied source, never resolved by materialization in this module.
    pub code: ConstructorCode,
}

/// Collision observations must hold over the entire destination class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum Collision {
    /// Current nonempty runtime/delegation code forbids this creation.
    NonEmptyCode,
    /// A current nonzero nonce forbids this creation even with empty code.
    NonZeroNonce,
    /// Zero nonce and empty code are established on this guard.
    Available,
    /// Both collision and a collision-free allocation remain possible.
    Unknown,
}

impl Collision {
    fn may_enter(self) -> bool {
        matches!(self, Self::Available | Self::Unknown)
    }

    fn may_collide(self) -> bool {
        self != Self::Available
    }
}

/// One cell of the owner lane's current named-plus-residual partition.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Destination {
    /// Never interpreted as a concrete address when it is Residual.
    pub owner: ProcessOwner,
    /// Collision-free success and collision rejection remain separate guards.
    pub collision: Collision,
}

/// A guarded constructor child for the existing relative procedure table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConstructorEntry {
    /// Original CREATE/CREATE2 action for subscriptions and control receipts.
    pub action: ProcessAction,
    /// Caller is exactly the creating execution owner, including under delegation.
    pub creator: ProcessOwner,
    /// Target class; do not infer inequality when it and creator differ.
    pub destination: ProcessOwner,
    /// Preserved InitCode source; never reclassified as Runtime or Unknown.
    pub code: ConstructorCode,
    /// Source generation of the covering target vocabulary.
    pub vocabulary: u64,
    /// Child checkpoint includes this creator's nonce increment.
    pub nonce: CreatorNonce,
    /// Capabilities for arbitrary constructor code, including callbacks/recursion.
    pub actions: [ProcessAction; 6],
}

impl ConstructorEntry {
    /// Constructor calldata is empty; its call value is the creation endowment.
    pub const fn has_empty_calldata(self) -> bool {
        true
    }

    /// CALL/EXTCODE resolve the account as empty until deployment. They never
    /// execute this child's initcode, even when the target aliases destination.
    pub const fn account_runtime_is_empty(self) -> bool {
        true
    }
}

/// A planning refusal is analysis work, never an EVM gas-failure alternative.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CreationError {
    /// Work/cancellation remains uncompleted in the caller's cumulative ledger.
    #[error("creation planning work exhausted or cancelled")]
    Uncompleted,
    /// A non-creation action was supplied to the creation rule.
    #[error("creation rule received {0:?}")]
    Action(ProcessAction),
    /// Targets must be strictly sorted, unique and end with a residual cell.
    #[error("creation needs a covering named/residual destination partition")]
    Partition,
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), CreationError> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.exhausted()
        || work == usize::MAX
        || !budget.charge(work)
    {
        return Err(CreationError::Uncompleted);
    }
    Ok(())
}

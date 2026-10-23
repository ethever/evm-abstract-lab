//! Typed post-initcode deployment and rollback, before process-parent Resume.

use super::{ConstructorEntry, CreationError, ProcessOwner, WorkBudget, reserve};
use crate::analysis::OutcomeKind;

/// CREATE's address result must never be replaced with CALL's success boolean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationResult {
    /// The actual hash-derived deployment address in this covering owner cell.
    Address(ProcessOwner),
    /// Rejection, constructor REVERT or exceptional creation failure.
    Zero,
}

/// Caller-visible bytes are not the runtime returned by successful initcode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReturnedData {
    /// Successful deployment and exceptional failure both expose no bytes.
    Empty,
    /// Preserve the actual constructor revert data, without materializing it.
    ConstructorRevert,
}

/// A child checkpoint is after nonce increment; the outer checkpoint is before
/// it. The adapter must call Store.restore for sticky-touch behavior on rollback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreDisposition {
    /// Commit constructor/callback effects; preserve created/destruction/touch
    /// flags and deploy the covered runtime. Finalize only at transaction end.
    CommitConstructor,
    /// Restore child savepoint, preserving creator nonce increment and sticky
    /// metadata. An enclosing revert still restores its earlier checkpoint.
    RestorePostNonceCheckpoint,
}

/// Deployment code is an entire valid-code class, not an invented Program.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Deployment {
    /// The guarded collision-free allocation owner.
    pub owner: ProcessOwner,
    /// EIP-170 bound; empty runtime is included.
    pub maximum_runtime_size: usize,
    /// EIP-3541 rejects 0xef as a first runtime byte in supported forks.
    pub forbidden_first_byte: u8,
}

/// Why a guarded constructor completion is represented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionReason {
    /// RETURN/STOP/SELFDESTRUCT successfully finishes the constructor.
    Deployed,
    /// Invalid/oversize runtime, code-deposit OOG, or exceptional child failure.
    Exceptional,
    /// REVERT supplies revert data and rolls back the child.
    Reverted,
}

/// One alternative for delivery to a process parent in the native table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompletionCase {
    /// Address-valued success or zero-valued rejection.
    pub result: CreationResult,
    /// Correct EVM returndata class.
    pub returndata: ReturnedData,
    /// Existing Store lifecycle/rollback path to apply.
    pub store: StoreDisposition,
    /// Present only on the successful, collision-free guard.
    pub deployment: Option<Deployment>,
    /// Distinguishes creation outcomes from analysis Work refusal.
    pub reason: CompletionReason,
}

/// Return never requires runtime bytes to be materialized: its successful arm
/// covers every valid runtime, and its exceptional arm covers invalid code and
/// deposit OOG. The constructor body itself still owes all action dependencies.
pub fn complete(
    entry: ConstructorEntry,
    exit: OutcomeKind,
    budget: &mut WorkBudget,
) -> Result<[Option<CompletionCase>; 2], CreationError> {
    reserve(budget, 256)?;
    let failed = CompletionCase {
        result: CreationResult::Zero,
        returndata: ReturnedData::Empty,
        store: StoreDisposition::RestorePostNonceCheckpoint,
        deployment: None,
        reason: CompletionReason::Exceptional,
    };
    Ok(match exit {
        OutcomeKind::Return => [
            Some(CompletionCase {
                result: CreationResult::Address(entry.destination),
                returndata: ReturnedData::Empty,
                store: StoreDisposition::CommitConstructor,
                deployment: Some(Deployment {
                    owner: entry.destination,
                    maximum_runtime_size: 24_576,
                    forbidden_first_byte: 0xef,
                }),
                reason: CompletionReason::Deployed,
            }),
            Some(failed),
        ],
        OutcomeKind::Revert => [
            Some(CompletionCase {
                returndata: ReturnedData::ConstructorRevert,
                reason: CompletionReason::Reverted,
                ..failed
            }),
            None,
        ],
        OutcomeKind::Failure => [Some(failed), None],
    })
}

/// Constant-time membership check for already available witness bytes. This is
/// not a request to extract unknown runtime code during abstract completion.
pub fn runtime_is_valid(bytes: &[u8], budget: &mut WorkBudget) -> Result<bool, CreationError> {
    reserve(budget, 16)?;
    Ok(bytes.len() <= 24_576 && bytes.first() != Some(&0xef))
}

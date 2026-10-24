//! Guarded constructor admission. No hashing or byte materialization occurs.

use super::{
    ConstructorEntry, CreationError, CreationRequest, CreatorNonce, Destination, ProcessAction,
    ProcessOwner, WorkBudget, reserve,
};

/// Exceptional halt of the creating frame, before any constructor continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum ParentException {
    /// CREATE/CREATE2 in a static execution context.
    Static,
    /// EIP-3860 initcode size exceeds the EVM limit.
    InitCodeSize,
    /// Opcode/memory-expansion gas exhaustion faults the current frame.
    Gas,
}

/// Immediate zero-result rejection; no initcode child is entered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum RejectionReason {
    /// EVM invocation depth exceeded.
    Depth,
    /// Creator nonce already equals u64::MAX.
    MaxNonce,
    /// The endowment cannot be funded.
    InsufficientBalance,
    /// Nonzero target nonce or nonempty target code prevents overwrite.
    Collision,
    /// Insufficient child/initcode gas after creator nonce increment.
    ChildGas,
}

/// Always returns zero with empty returndata to the process parent.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Rejection {
    /// EVM reason, distinct from an analysis refusal.
    pub reason: RejectionReason,
    /// Collision is qualified by its destination; other rejections need no hash.
    pub destination: Option<ProcessOwner>,
    /// The creator nonce increment survives this rejection, if true.
    pub increments_nonce: bool,
}

/// Paid, bounded rule alternatives for one process-parent creation action.
#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct CreationPlan {
    /// Fault the current frame through existing exceptional completion.
    pub exceptions: Vec<ParentException>,
    /// Resume the process parent with zero and empty returndata.
    pub rejections: Vec<Rejection>,
    /// Enter InitCode children under collision-free/affordable/size guards.
    pub constructors: Vec<ConstructorEntry>,
}

/// Plan over all named destinations and their residual complement. The owner
/// adapter guarantees vocabulary completeness/currency; sortedness alone is
/// not a proof of that contract. Unknown salt/hash/nonce retains every cell.
pub fn prepare(
    request: CreationRequest,
    targets: &[Destination],
    vocabulary: u64,
    budget: &mut WorkBudget,
) -> Result<CreationPlan, CreationError> {
    // Pays the complete validation scan, all maximum-capacity vectors and
    // fixed-size per-cell action/entry copies before inspecting or allocating.
    reserve(
        budget,
        targets.len().saturating_mul(384).saturating_add(384),
    )?;
    if !matches!(
        request.action,
        ProcessAction::Create | ProcessAction::Create2
    ) {
        return Err(CreationError::Action(request.action));
    }
    if targets.last().map(|target| target.owner) != Some(ProcessOwner::Residual)
        || targets
            .windows(2)
            .any(|pair| pair[0].owner >= pair[1].owner)
    {
        return Err(CreationError::Partition);
    }
    let mut plan = CreationPlan {
        exceptions: Vec::with_capacity(3),
        rejections: Vec::with_capacity(targets.len().saturating_add(4)),
        constructors: Vec::with_capacity(targets.len()),
    };
    if request.is_static {
        plan.exceptions.push(ParentException::Static);
        return Ok(plan);
    }
    plan.exceptions.push(ParentException::Gas);
    if request.initcode_size.may_reject() {
        plan.exceptions.push(ParentException::InitCodeSize);
    }
    if !request.initcode_size.may_allow() {
        return Ok(plan);
    }
    for (reject, reason) in [
        (request.depth.may_reject(), RejectionReason::Depth),
        (
            request.nonce == CreatorNonce::Unknown
                || request.nonce == CreatorNonce::Exact(u64::MAX),
            RejectionReason::MaxNonce,
        ),
        (
            request.funding.may_reject(),
            RejectionReason::InsufficientBalance,
        ),
    ] {
        if reject {
            plan.rejections.push(Rejection {
                reason,
                destination: None,
                increments_nonce: false,
            });
        }
    }
    if !request.depth.may_allow()
        || !request.funding.may_allow()
        || request.nonce == CreatorNonce::Exact(u64::MAX)
    {
        return Ok(plan);
    }
    plan.rejections.push(Rejection {
        reason: RejectionReason::ChildGas,
        destination: None,
        increments_nonce: true,
    });
    for target in targets {
        reserve(budget, 0)?;
        if target.collision.may_collide() {
            plan.rejections.push(Rejection {
                reason: RejectionReason::Collision,
                destination: Some(target.owner),
                increments_nonce: true,
            });
        }
        if target.collision.may_enter() {
            plan.constructors.push(ConstructorEntry {
                action: request.action,
                creator: request.creator,
                destination: target.owner,
                code: request.code,
                vocabulary,
                nonce: request.nonce,
                actions: [
                    ProcessAction::Call,
                    ProcessAction::StaticCall,
                    ProcessAction::DelegateCall,
                    ProcessAction::CallCode,
                    ProcessAction::Create,
                    ProcessAction::Create2,
                ],
            });
        }
    }
    reserve(budget, 0)?;
    Ok(plan)
}

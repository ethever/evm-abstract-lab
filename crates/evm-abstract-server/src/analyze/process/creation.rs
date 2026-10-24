//! Copy actual recorded allocation/delivery alternatives, never run a planner in projection.
use super::{Pool, action, analysis, api, owner};
use analysis::process_creation as native;

fn descriptor(source: analysis::ProcessCreation) -> api::ProcessCreation {
    let analysis::ProcessCreation {
        action: primitive,
        creator,
        destination,
        epoch,
    } = source;
    api::ProcessCreation {
        action: action(primitive),
        creator: owner(creator),
        destination: owner(destination),
        epoch: api::ProcessDigest(epoch.0),
    }
}

pub(in crate::analyze) fn continuation(
    source: analysis::CreationContinuation,
) -> api::CreationContinuation {
    match source {
        analysis::CreationContinuation::Native { destination } => {
            api::CreationContinuation::Native(api::NativeCreation {
                destination: destination.to_string(),
            })
        }
        analysis::CreationContinuation::Process(source) => {
            api::CreationContinuation::Process(descriptor(source))
        }
    }
}

pub(super) fn transfer(
    source: &analysis::ProcessCreationTransfer,
    export: &mut Pool<'_>,
) -> Result<api::ProcessCreationTransfer, api::ApiError> {
    export.charge(
        5usize
            .saturating_add(source.exceptions.len())
            .saturating_add(source.rejections.len().saturating_mul(3)),
    )?;
    let mut exceptions = Vec::with_capacity(source.exceptions.len());
    for reason in &source.exceptions {
        export.charge(0)?;
        exceptions.push(match reason {
            native::ParentException::Static => api::ProcessParentException::Static,
            native::ParentException::InitCodeSize => api::ProcessParentException::InitCodeSize,
            native::ParentException::Gas => api::ProcessParentException::Gas,
        });
    }
    let mut rejections = Vec::with_capacity(source.rejections.len());
    for rejection in &source.rejections {
        export.charge(0)?;
        let reason = match rejection.reason {
            native::RejectionReason::Depth => api::ProcessRejectionReason::Depth,
            native::RejectionReason::MaxNonce => api::ProcessRejectionReason::MaxNonce,
            native::RejectionReason::InsufficientBalance => {
                api::ProcessRejectionReason::InsufficientBalance
            }
            native::RejectionReason::Collision => api::ProcessRejectionReason::Collision,
            native::RejectionReason::ChildGas => api::ProcessRejectionReason::ChildGas,
        };
        rejections.push(api::ProcessRejection {
            reason,
            destination: rejection.destination.map(owner),
            increments_nonce: rejection.increments_nonce,
        });
    }
    Ok(api::ProcessCreationTransfer {
        action: action(source.action),
        epoch: api::ProcessDigest(source.epoch.0),
        creator: owner(source.creator),
        exceptions,
        rejections,
    })
}

pub(in crate::analyze) fn completion(
    source: &native::CompletionCase,
    export: &mut Pool<'_>,
) -> Result<api::ProcessCompletionCase, api::ApiError> {
    export.charge(9)?;
    Ok(api::ProcessCompletionCase {
        result: match source.result {
            native::CreationResult::Address(target) => {
                api::ProcessCreationResult::Address(owner(target))
            }
            native::CreationResult::Zero => api::ProcessCreationResult::Zero,
        },
        returndata: match source.returndata {
            native::ReturnedData::Empty => api::ProcessReturnedData::Empty,
            native::ReturnedData::ConstructorRevert => api::ProcessReturnedData::ConstructorRevert,
        },
        store: match source.store {
            native::StoreDisposition::CommitConstructor => {
                api::ProcessStoreDisposition::CommitConstructor
            }
            native::StoreDisposition::RestorePostNonceCheckpoint => {
                api::ProcessStoreDisposition::RestorePostNonceCheckpoint
            }
        },
        deployment: source.deployment.map(|deployment| api::ProcessDeployment {
            owner: owner(deployment.owner),
            maximum_runtime_size: deployment.maximum_runtime_size,
            forbidden_first_byte: deployment.forbidden_first_byte,
        }),
        reason: match source.reason {
            native::CompletionReason::Deployed => api::ProcessCompletionReason::Deployed,
            native::CompletionReason::Exceptional => api::ProcessCompletionReason::Exceptional,
            native::CompletionReason::Reverted => api::ProcessCompletionReason::Reverted,
        },
    })
}

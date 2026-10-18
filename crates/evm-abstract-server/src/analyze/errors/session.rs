//! Project actual session input failures without hiding cancellation or fact identity.

use evm_abstract::{
    Fork,
    analysis::layers::{
        CatalogError, CodeId, CodeMode, FactError, FactKey, SessionError, SnapshotId,
    },
    world::SnapshotIdentity,
};
use evm_abstract_protocol as api;

fn fork(source: Fork) -> api::Fork {
    match source {
        Fork::Cancun => api::Fork::Cancun,
        Fork::Prague => api::Fork::Prague,
        Fork::Osaka => api::Fork::Osaka,
    }
}
fn code(source: CodeId) -> api::SessionCodeIdentity {
    api::SessionCodeIdentity {
        fork: fork(source.fork()),
        kind: match source.mode() {
            CodeMode::Runtime => api::CodeKind::Runtime,
            CodeMode::Initcode => api::CodeKind::Initcode,
        },
        hash: source.hash().to_string(),
    }
}
fn snapshot(source: &SnapshotId) -> api::SessionSnapshotDescription {
    match source.identity() {
        SnapshotIdentity::Offline { label } => {
            api::SessionSnapshotDescription::Offline(label.clone())
        }
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => api::SessionSnapshotDescription::Chain(api::SessionChainSnapshot {
            chain_id: super::super::value::word(*chain_id),
            block_hash: block_hash.to_string(),
        }),
    }
}
fn fact(source: FactKey) -> api::InitialFactLocation {
    match source {
        FactKey::Code(address) => api::InitialFactLocation::Code(address.to_string()),
        FactKey::Balance(address) => api::InitialFactLocation::Balance(address.to_string()),
        FactKey::Nonce(address) => api::InitialFactLocation::Nonce(address.to_string()),
        FactKey::Existence(address) => api::InitialFactLocation::Existence(address.to_string()),
        FactKey::Storage { address, slot } => {
            api::InitialFactLocation::Storage(api::StorageLocation {
                address: address.to_string(),
                slot: super::super::value::word(slot),
            })
        }
        FactKey::StorageComplete(address) => {
            api::InitialFactLocation::StorageComplete(address.to_string())
        }
    }
}
pub(in crate::analyze) fn error(source: SessionError) -> api::ApiError {
    let message = source.to_string();
    let detail = match source {
        SessionError::Cancelled(_) => return super::super::cancelled(),
        SessionError::Config(error) => return super::configuration(error),
        SessionError::World(error) => return super::world(error),
        SessionError::Catalog(CatalogError::MixedFork { catalog, program }) => {
            api::SessionFailure::CatalogFork(api::CatalogForkMismatch {
                catalog: fork(catalog),
                program: fork(program),
            })
        }
        SessionError::Catalog(CatalogError::HashCollision(id)) => {
            api::SessionFailure::CatalogHashCollision(code(id))
        }
        SessionError::Facts(FactError::Snapshot { expected, observed }) => {
            api::SessionFailure::Snapshot(api::SessionSnapshotMismatch {
                expected: snapshot(&expected),
                observed: snapshot(&observed),
            })
        }
        SessionError::Facts(FactError::Conflict { key }) => {
            api::SessionFailure::FactConflict(fact(key))
        }
        SessionError::MissingCode(id) => api::SessionFailure::MissingCode(code(id)),
        SessionError::InitcodeBinding(id) => api::SessionFailure::InitcodeBinding(code(id)),
        SessionError::EntryCode(address) => api::SessionFailure::EntryCode(address.to_string()),
        SessionError::Seed => api::SessionFailure::Seed,
    };
    api::ApiError {
        code: api::ApiErrorCode::InvalidRequest,
        message,
        details: api::ErrorDetails::Session(Box::new(detail)),
    }
}

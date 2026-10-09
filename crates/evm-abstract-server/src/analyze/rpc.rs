//! Sanitized RPC evidence retains typed transport facts and fixed snapshot context.
use super::value;
pub(super) mod causes;
use evm_abstract::{analysis, world::rpc};
use evm_abstract_protocol as api;
pub(super) fn failure(source: &rpc::Failure) -> api::RpcFailure {
    use api::RpcFailureKind as P;
    use rpc::RpcFailureKind as N;
    api::RpcFailure {
        kind: match source.kind {
            N::Cancelled => P::Cancelled,
            N::Configuration => P::Configuration,
            N::AcquisitionLimit => P::AcquisitionLimit,
            N::Transport => P::Transport,
            N::Http => P::Http,
            N::ResponseLimit => P::ResponseLimit,
            N::Read => P::Read,
            N::Json => P::Json,
            N::Remote => P::Remote,
            N::MissingResult => P::MissingResult,
            N::Response => P::Response,
            N::ChainMismatch => P::ChainMismatch,
            N::BlockMismatch => P::BlockMismatch,
            N::Code => P::Code,
            N::World => P::World,
        },
        chain_id: source.context.chain_id.map(value::word),
        block_hash: source.context.block_hash.map(|hash| hash.to_string()),
        method: source.context.method.into(),
        account: source.context.account.map(|address| address.to_string()),
        slot: source.context.slot.map(value::word),
        resource: source.resource.map(|resource| match resource {
            rpc::AcquisitionLimit::Accounts => api::AcquisitionResource::Accounts,
            rpc::AcquisitionLimit::Requests => api::AcquisitionResource::Requests,
        }),
        limit: source.limit,
        http_status: source.http_status,
        rpc_code: source.rpc_code,
        json_line: source.json_line,
        json_column: source.json_column,
        cause: source.cause.as_ref().map(causes::cause),
        message: source.message.clone(),
    }
}
pub(super) fn error(source: rpc::RpcError) -> api::ApiError {
    let failure = failure(&source.failure());
    api::ApiError {
        code: if failure.kind == api::RpcFailureKind::Cancelled {
            api::ApiErrorCode::Cancelled
        } else {
            api::ApiErrorCode::Rpc
        },
        message: failure.message.clone(),
        details: api::ErrorDetails::Rpc(Box::new(failure)),
    }
}
pub(super) fn acquisition(source: &analysis::RpcAcquisition) -> api::AcquisitionReport {
    let slot = |item: &analysis::RpcStorageSlot| api::StorageLocation {
        address: item.address.to_string(),
        slot: value::word(item.slot),
    };
    api::AcquisitionReport {
        rounds: source.rounds,
        fetched_accounts: source
            .fetched_accounts
            .iter()
            .map(ToString::to_string)
            .collect(),
        failed_accounts: source
            .failed_accounts
            .iter()
            .map(ToString::to_string)
            .collect(),
        fetched_storage: source.fetched_storage.iter().map(slot).collect(),
        failed_storage: source.failed_storage.iter().map(slot).collect(),
        failures: source
            .failures
            .iter()
            .map(|item| {
                let mut result = failure(&item.failure);
                result.account = Some(item.address.to_string());
                result.slot = item.slot.map(value::word);
                result
            })
            .collect(),
        requests: source.requests,
        states_created: source.states_created,
    }
}

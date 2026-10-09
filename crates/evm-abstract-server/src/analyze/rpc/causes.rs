//! Exhaustive cause projection shared by admission and late RPC frontiers.
use super::super::value;
use evm_abstract::{Fork, world::rpc as native};
use evm_abstract_protocol as api;
fn fork(source: Fork) -> api::Fork {
    match source {
        Fork::Cancun => api::Fork::Cancun,
        Fork::Prague => api::Fork::Prague,
        Fork::Osaka => api::Fork::Osaka,
    }
}
pub(in crate::analyze) fn code(source: &native::CodeFailure) -> api::BytecodeFailure {
    use api::BytecodeFailure as P;
    use native::CodeFailure as N;
    match source {
        N::OddHexLength(value) => P::OddHexLength(*value),
        N::InvalidHex { index, character } => P::InvalidHex(api::HexDigitFailure {
            index: *index,
            character: *character,
        }),
        N::UnsupportedEof => P::UnsupportedEof,
        N::DelegatedCode(address) => P::DelegatedCode(address.to_string()),
        N::InvalidDelegation(reason) => P::InvalidDelegation(match reason {
            native::DelegationFailure::InvalidLength => api::DelegationFailure::InvalidLength,
            native::DelegationFailure::InvalidMagic => api::DelegationFailure::InvalidMagic,
            native::DelegationFailure::UnsupportedVersion => {
                api::DelegationFailure::UnsupportedVersion
            }
        }),
    }
}
pub(in crate::analyze) fn world(source: &native::WorldFailure) -> api::WorldFailure {
    use api::WorldFailure as P;
    use native::WorldFailure as N;
    match source {
        N::MixedFork { world, account } => P::MixedFork(api::ForkMismatch {
            world: fork(*world),
            account: fork(*account),
        }),
        N::UnsupportedDelegation(rules) => P::UnsupportedDelegation(fork(*rules)),
        N::Conflict(address) => P::Conflict(address.to_string()),
        N::CodeHash {
            address,
            expected,
            observed,
        } => P::CodeHash(api::CodeHashMismatch {
            address: address.to_string(),
            expected: expected.to_string(),
            observed: observed.to_string(),
        }),
        N::UnknownCodeHash(address) => P::UnknownCodeHash(address.to_string()),
        N::InvalidAbsence(address) => P::InvalidAbsence(address.to_string()),
    }
}
fn header(source: native::HeaderValue) -> api::RpcHeaderValue {
    match source {
        native::HeaderValue::Absent => api::RpcHeaderValue::Absent,
        native::HeaderValue::Quantity(number) => api::RpcHeaderValue::Quantity(value::word(number)),
        native::HeaderValue::Hash(hash) => api::RpcHeaderValue::Hash(hash.to_string()),
        native::HeaderValue::Address(address) => api::RpcHeaderValue::Address(address.to_string()),
    }
}
fn field(source: native::HeaderField) -> api::RpcHeaderField {
    use api::RpcHeaderField as P;
    use native::HeaderField as N;
    match source {
        N::ParentHash => P::ParentHash,
        N::Number => P::Number,
        N::Timestamp => P::Timestamp,
        N::Miner => P::Miner,
        N::MixHash => P::MixHash,
        N::GasLimit => P::GasLimit,
        N::BaseFee => P::BaseFee,
        N::ExcessBlobGas => P::ExcessBlobGas,
        N::BlobGasUsed => P::BlobGasUsed,
    }
}
fn response(source: &native::ResponseReason) -> api::RpcResponseReason {
    use api::RpcResponseReason as P;
    use native::ResponseReason as N;
    match source {
        N::BlockNumber { expected, observed } => P::BlockNumber(api::WordMismatch {
            expected: value::word(*expected),
            observed: value::word(*observed),
        }),
        N::BlobHeaderFields {
            excess_blob_gas,
            blob_gas_used,
        } => P::BlobHeaderFields(api::BlobHeaderFields {
            excess_blob_gas: *excess_blob_gas,
            blob_gas_used: *blob_gas_used,
        }),
        N::FeeHistory {
            expected,
            observed,
            fees,
        } => P::FeeHistory(api::FeeHistoryMismatch {
            expected: value::word(*expected),
            observed: value::word(*observed),
            fees: *fees,
        }),
        N::Schema => P::Schema,
        N::Version => P::Version,
        N::Id { expected, observed } => P::Id(api::ResponseIdMismatch {
            expected: *expected,
            observed: *observed,
        }),
        N::ResultAndError => P::ResultAndError,
        N::NullError => P::NullError,
        N::HeaderChanged {
            field: changed,
            expected,
            observed,
        } => P::HeaderChanged(api::HeaderChange {
            field: field(*changed),
            expected: header(*expected),
            observed: header(*observed),
        }),
        N::MissingBlockNumber => P::MissingBlockNumber,
        N::StorageLength { expected, observed } => P::StorageLength(api::CountMismatch {
            expected: *expected,
            observed: *observed,
        }),
    }
}
pub(super) fn cause(source: &native::FailureCause) -> api::RpcFailureCause {
    use api::RpcFailureCause as P;
    use native::FailureCause as N;
    match source {
        N::Configuration(reason) => P::Configuration(match reason {
            native::ConfigurationReason::Limits => api::RpcConfigurationReason::Limits,
            native::ConfigurationReason::InitialAccounts => {
                api::RpcConfigurationReason::InitialAccounts
            }
            native::ConfigurationReason::StorageAccountMissing => {
                api::RpcConfigurationReason::StorageAccountMissing
            }
        }),
        N::Response(reason) => P::Response(response(reason)),
        N::Code(reason) => P::Code(code(reason)),
        N::World(reason) => P::World(world(reason)),
        N::ChainMismatch(number) => P::ChainMismatch(value::word(*number)),
        N::BlockMismatch(hash) => P::BlockMismatch(hash.to_string()),
        N::Transport(failure) => P::Transport(api::RpcTransportFailure {
            timeout: failure.timeout,
            connect: failure.connect,
            builder: failure.builder,
            request: failure.request,
            body: failure.body,
            decode: failure.decode,
            redirect: failure.redirect,
        }),
        N::Runtime(code) => P::Runtime(*code),
    }
}

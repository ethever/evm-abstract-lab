//! Admission and conversion of fully typed execution inputs.
use alloy_primitives::{Address, B256, U256, hex};
use evm_abstract::{
    analysis,
    domain::{
        AbstractValue, Profile,
        relational::{RelationLimits, SmtProvider},
    },
    world::{AddressInput, BlobHashes, ByteArray, EvmEnvironment, GasInput, rpc},
};
use evm_abstract_protocol as api;
use std::{collections::BTreeMap, time::Duration};

pub(super) fn invalid(
    kind: api::ValidationErrorKind,
    field: &str,
    message: impl Into<String>,
) -> api::ApiError {
    api::ApiError {
        code: if kind == api::ValidationErrorKind::Limits {
            api::ApiErrorCode::InvalidLimits
        } else if kind == api::ValidationErrorKind::Bytecode {
            api::ApiErrorCode::InvalidBytecode
        } else {
            api::ApiErrorCode::InvalidRequest
        },
        message: message.into(),
        details: api::ErrorDetails::Validation(api::ValidationFailure {
            kind,
            field: Some(field.into()),
            value: None,
        }),
    }
}
pub(super) fn address(value: &str, field: &str) -> Result<Address, api::ApiError> {
    value.parse().map_err(|_| {
        invalid(
            api::ValidationErrorKind::Address,
            field,
            "expected a twenty-byte hexadecimal address",
        )
    })
}
fn word(value: &str, field: &str) -> Result<U256, api::ApiError> {
    value.parse().map_err(|_| {
        invalid(
            api::ValidationErrorKind::Word,
            field,
            "expected an unsigned 256-bit decimal or hexadecimal word",
        )
    })
}
fn hash(value: &str, field: &str) -> Result<B256, api::ApiError> {
    value.parse().map_err(|_| {
        invalid(
            api::ValidationErrorKind::Block,
            field,
            "expected a 32-byte hexadecimal hash",
        )
    })
}
fn value(input: &api::WordInput, field: &str) -> Result<AbstractValue, api::ApiError> {
    match input {
        api::WordInput::Unknown => Ok(AbstractValue::top()),
        api::WordInput::Concrete(text) => word(text, field).map(AbstractValue::constant),
    }
}
fn optional(input: &Option<String>, field: &str) -> Result<AbstractValue, api::ApiError> {
    input
        .as_ref()
        .map(|text| word(text, field).map(AbstractValue::constant))
        .unwrap_or_else(|| Ok(AbstractValue::top()))
}
fn hashes(input: &[api::IndexedHash], field: &str) -> Result<BTreeMap<U256, B256>, api::ApiError> {
    let mut result = BTreeMap::new();
    for item in input {
        if result
            .insert(word(&item.index, field)?, hash(&item.hash, field)?)
            .is_some()
        {
            return Err(invalid(
                api::ValidationErrorKind::Environment,
                field,
                "duplicate observed hash index",
            ));
        }
    }
    Ok(result)
}
pub(super) fn environment(input: &api::EnvironmentInput) -> Result<EvmEnvironment, api::ApiError> {
    let caller = match &input.caller {
        api::AddressSetting::Unknown => AddressInput::unknown_caller(),
        api::AddressSetting::Concrete(text) => address(text, "caller")?.into(),
    };
    let origin = input
        .origin
        .as_ref()
        .map(|setting| match setting {
            api::AddressSetting::Unknown => Ok(AddressInput::unknown_origin()),
            api::AddressSetting::Concrete(text) => address(text, "origin").map(AddressInput::from),
        })
        .transpose()?;
    let calldata = match &input.calldata {
        api::CalldataInput::Unknown => ByteArray::unknown(),
        api::CalldataInput::Exact(text) => ByteArray::exact(&hex::decode(text).map_err(|_| {
            invalid(
                api::ValidationErrorKind::Environment,
                "calldata",
                "expected hexadecimal calldata bytes",
            )
        })?),
    };
    let environment = EvmEnvironment {
        caller,
        origin,
        calldata,
        value: value(&input.call_value, "call_value")?,
        is_static: input.is_static,
        gas: input
            .gas_upper_bound
            .as_ref()
            .map(|text| word(text, "gas_upper_bound").map(GasInput::UpperBound))
            .transpose()?
            .unwrap_or(GasInput::Unknown),
        gas_price: value(&input.gas_price, "gas_price")?,
        coinbase: input
            .coinbase
            .as_ref()
            .map(|text| address(text, "coinbase").map(AddressInput::from))
            .transpose()?
            .unwrap_or_else(AddressInput::unknown_coinbase),
        timestamp: optional(&input.timestamp, "timestamp")?,
        number: optional(&input.number, "number")?,
        prevrandao: optional(&input.prevrandao, "prevrandao")?,
        gas_limit: optional(&input.gas_limit, "gas_limit")?,
        chain_id: input
            .chain_id
            .as_ref()
            .map(|text| word(text, "chain_id").map(AbstractValue::constant))
            .transpose()?,
        base_fee: optional(&input.base_fee, "base_fee")?,
        blob_base_fee: optional(&input.blob_base_fee, "blob_base_fee")?,
        block_hashes: hashes(&input.block_hashes, "block_hashes")?,
        blob_hashes: BlobHashes {
            length: value(&input.blob_count, "blob_count")?,
            hashes: hashes(&input.blob_hashes, "blob_hashes")?,
        },
        ..EvmEnvironment::default()
    };
    environment.validate().map_err(super::errors::environment)?;
    Ok(environment)
}
pub(super) fn fork(input: api::Fork) -> evm_abstract::Fork {
    match input {
        api::Fork::Cancun => evm_abstract::Fork::Cancun,
        api::Fork::Prague => evm_abstract::Fork::Prague,
        api::Fork::Osaka => evm_abstract::Fork::Osaka,
    }
}
pub(super) fn config(
    input: &api::AnalysisLimits,
) -> Result<analysis::ExecutionConfig, api::ApiError> {
    // Server admission caps bound shared worker memory and work before acquisition starts.
    let bounded = [
        ("max_states", input.max_states, 4096),
        ("max_transfers", input.max_transfers, 100_000),
        ("max_constants", input.max_constants, 32),
        ("max_work", input.max_work, 100_000_000),
        ("max_call_depth", input.max_call_depth, 128),
        ("max_memory_bytes", input.max_memory_bytes, 1_048_576),
        ("reduction_rounds", input.reduction_rounds, 32),
        ("max_facts", input.max_facts, 16_384),
        ("max_constraints", input.max_constraints, 4096),
        ("max_expression_nodes", input.max_expression_nodes, 16_384),
        ("max_expression_depth", input.max_expression_depth, 128),
        ("rpc_max_accounts", input.rpc_max_accounts, 4096),
        ("rpc_max_requests", input.rpc_max_requests, 65_536),
        (
            "rpc_max_response_bytes",
            input.rpc_max_response_bytes,
            67_108_864,
        ),
    ];
    for (field, number, max) in bounded {
        if !(1..=max).contains(&number) {
            return Err(super::errors::limit(field, number as u64, 1, max as u64));
        }
    }
    if input.context_depth > 16 {
        return Err(super::errors::limit(
            "context_depth",
            input.context_depth as u64,
            0,
            16,
        ));
    }
    if !(1..=10_000_000).contains(&input.smt_rlimit) {
        return Err(super::errors::limit(
            "smt_rlimit",
            u64::from(input.smt_rlimit),
            1,
            10_000_000,
        ));
    }
    if !(1..=120_000).contains(&input.rpc_timeout_ms) {
        return Err(super::errors::limit(
            "rpc_timeout_ms",
            input.rpc_timeout_ms,
            1,
            120_000,
        ));
    }
    Ok(analysis::ExecutionConfig {
        analysis: analysis::Config {
            domain_profile: match input.domain_profile {
                api::DomainProfile::Product => Profile::Product,
                api::DomainProfile::ConstantsOnly => Profile::ConstantsOnly,
            },
            reduction_rounds: input.reduction_rounds,
            max_facts: input.max_facts,
            max_states: input.max_states,
            max_transfers: input.max_transfers,
            max_constants: input.max_constants,
            context_depth: input.context_depth,
            relations: RelationLimits {
                enabled: input.relations_enabled,
                max_constraints: input.max_constraints,
                max_nodes: input.max_expression_nodes,
                max_depth: input.max_expression_depth,
                rlimit: input.smt_rlimit,
                provider: match input.smt_provider {
                    api::SmtProvider::Z3 => SmtProvider::Z3,
                    api::SmtProvider::Bitwuzla => SmtProvider::Bitwuzla,
                    api::SmtProvider::Cvc5 => SmtProvider::Cvc5,
                },
            },
        },
        max_work: input.max_work,
        max_call_depth: input.max_call_depth,
        max_memory_bytes: input.max_memory_bytes,
        use_summaries: input.use_summaries,
    })
}
pub(super) fn rpc(
    input: &api::RpcInput,
    endpoint: &str,
    fork: evm_abstract::Fork,
    limits: &api::AnalysisLimits,
) -> Result<rpc::RpcInput, api::ApiError> {
    Ok(rpc::RpcInput {
        endpoint: endpoint.into(),
        fork,
        block: match &input.block {
            api::BlockSelector::Latest => rpc::RpcBlock::Latest,
            api::BlockSelector::Number(number) => rpc::RpcBlock::Number(*number),
            api::BlockSelector::Hash(text) => rpc::RpcBlock::Hash(hash(text, "block")?),
        },
        accounts: input
            .accounts
            .iter()
            .map(|account| {
                Ok(rpc::AccountRequest {
                    address: address(&account.address, "accounts.address")?,
                    slots: account
                        .slots
                        .iter()
                        .map(|slot| word(slot, "accounts.slots"))
                        .collect::<Result<_, _>>()?,
                })
            })
            .collect::<Result<_, api::ApiError>>()?,
        timeout: Duration::from_millis(limits.rpc_timeout_ms),
        max_response_bytes: limits.rpc_max_response_bytes,
        max_accounts: limits.rpc_max_accounts,
        max_requests: limits.rpc_max_requests,
    })
}

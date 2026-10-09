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
// Only the host representation bounds integer conversion; teaching inputs have
// no server-specific maximum. Positivity and semantic invariants stay native.
fn host_size(value: u64, field: &str) -> Result<usize, api::ApiError> {
    usize::try_from(value).map_err(|_| {
        invalid(
            api::ValidationErrorKind::Limits,
            field,
            "value does not fit this server's address space",
        )
    })
}

pub(super) fn config(
    input: &api::AnalysisLimits,
) -> Result<analysis::ExecutionConfig, api::ApiError> {
    for (field, value) in [
        ("rpc_max_accounts", input.rpc_max_accounts),
        ("rpc_max_requests", input.rpc_max_requests),
        ("rpc_max_response_bytes", input.rpc_max_response_bytes),
        ("rpc_timeout_ms", input.rpc_timeout_ms),
    ] {
        if value == 0 {
            return Err(invalid(
                api::ValidationErrorKind::Limits,
                field,
                "RPC allowances and timeout must be positive",
            ));
        }
    }
    Ok(analysis::ExecutionConfig {
        analysis: analysis::Config {
            domain_profile: match input.domain_profile {
                api::DomainProfile::Product => Profile::Product,
                api::DomainProfile::ConstantsOnly => Profile::ConstantsOnly,
            },
            reduction_rounds: host_size(input.reduction_rounds, "reduction_rounds")?,
            max_facts: host_size(input.max_facts, "max_facts")?,
            max_states: host_size(input.max_states, "max_states")?,
            max_transfers: host_size(input.max_transfers, "max_transfers")?,
            max_constants: host_size(input.max_constants, "max_constants")?,
            context_depth: host_size(input.context_depth, "context_depth")?,
            relations: RelationLimits {
                enabled: input.relations_enabled,
                max_constraints: host_size(input.max_constraints, "max_constraints")?,
                max_nodes: host_size(input.max_expression_nodes, "max_expression_nodes")?,
                max_depth: host_size(input.max_expression_depth, "max_expression_depth")?,
                rlimit: input.smt_rlimit,
                provider: match input.smt_provider {
                    api::SmtProvider::Z3 => SmtProvider::Z3,
                    api::SmtProvider::Bitwuzla => SmtProvider::Bitwuzla,
                    api::SmtProvider::Cvc5 => SmtProvider::Cvc5,
                },
            },
        },
        max_work: host_size(input.max_work, "max_work")?,
        max_call_depth: host_size(input.max_call_depth, "max_call_depth")?,
        max_memory_bytes: host_size(input.max_memory_bytes, "max_memory_bytes")?,
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
        max_response_bytes: host_size(limits.rpc_max_response_bytes, "rpc_max_response_bytes")?,
        max_accounts: host_size(limits.rpc_max_accounts, "rpc_max_accounts")?,
        max_requests: host_size(limits.rpc_max_requests, "rpc_max_requests")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> api::RpcInput {
        api::RpcInput {
            provider_id: "fixture".into(),
            address: Address::repeat_byte(0x11).to_string(),
            block: api::BlockSelector::Latest,
            accounts: Vec::new(),
        }
    }

    #[test]
    fn values_above_every_former_server_ceiling_reach_the_native_policy_unchanged() {
        let limits = api::AnalysisLimits {
            max_states: 4_097,
            max_transfers: 100_001,
            context_depth: 17,
            max_constants: 33,
            max_work: 100_000_001,
            max_call_depth: 129,
            max_memory_bytes: 1_048_577,
            reduction_rounds: 33,
            max_facts: 16_385,
            max_constraints: 4_097,
            max_expression_nodes: 16_385,
            max_expression_depth: 129,
            smt_rlimit: 10_000_001,
            rpc_max_accounts: 4_097,
            rpc_max_requests: 65_537,
            rpc_max_response_bytes: 67_108_865,
            rpc_timeout_ms: 120_001,
            ..api::AnalysisLimits::default()
        };
        let native = config(&limits).unwrap();
        assert_eq!(native.analysis.max_states, 4_097);
        assert_eq!(native.analysis.max_transfers, 100_001);
        assert_eq!(native.analysis.context_depth, 17);
        assert_eq!(native.analysis.max_constants, 33);
        assert_eq!(native.max_work, 100_000_001);
        assert_eq!(native.max_call_depth, 129);
        assert_eq!(native.max_memory_bytes, 1_048_577);
        assert_eq!(native.analysis.reduction_rounds, 33);
        assert_eq!(native.analysis.max_facts, 16_385);
        assert_eq!(native.analysis.relations.max_constraints, 4_097);
        assert_eq!(native.analysis.relations.max_nodes, 16_385);
        assert_eq!(native.analysis.relations.max_depth, 129);
        assert_eq!(native.analysis.relations.rlimit, 10_000_001);
        assert!(native.analysis.validate().is_ok());
        let rpc = rpc(
            &source(),
            "http://127.0.0.1:8545",
            evm_abstract::Fork::Osaka,
            &limits,
        )
        .unwrap();
        assert_eq!(rpc.max_accounts, 4_097);
        assert_eq!(rpc.max_requests, 65_537);
        assert_eq!(rpc.max_response_bytes, 67_108_865);
        assert_eq!(rpc.timeout, Duration::from_millis(120_001));
    }

    #[test]
    fn only_host_integer_representation_bounds_sizes() {
        assert_eq!(
            host_size(usize::MAX as u64, "max_work").unwrap(),
            usize::MAX
        );
        if usize::BITS < u64::BITS {
            assert_eq!(
                host_size(u64::MAX, "max_work").unwrap_err().code,
                api::ApiErrorCode::InvalidLimits
            );
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn counts_above_u32_and_at_the_host_limit_are_not_narrowed_or_clamped() {
        for count in [u64::from(u32::MAX) + 1, (1u64 << 53) + 1, u64::MAX] {
            let limits = api::AnalysisLimits {
                max_states: count,
                max_transfers: count,
                context_depth: count,
                max_constants: count,
                max_work: count,
                max_call_depth: count,
                max_memory_bytes: count,
                reduction_rounds: count,
                max_facts: count,
                max_constraints: count,
                max_expression_nodes: count,
                max_expression_depth: count,
                smt_rlimit: u32::MAX,
                rpc_max_accounts: count,
                rpc_max_requests: count,
                rpc_max_response_bytes: count,
                rpc_timeout_ms: count,
                ..api::AnalysisLimits::default()
            };
            let native = config(&limits).unwrap();
            assert_eq!(native.analysis.max_states as u64, count);
            assert_eq!(native.analysis.max_transfers as u64, count);
            assert_eq!(native.analysis.context_depth as u64, count);
            assert_eq!(native.analysis.max_constants as u64, count);
            assert_eq!(native.max_work as u64, count);
            assert_eq!(native.max_call_depth as u64, count);
            assert_eq!(native.max_memory_bytes as u64, count);
            assert_eq!(native.analysis.reduction_rounds as u64, count);
            assert_eq!(native.analysis.max_facts as u64, count);
            assert_eq!(native.analysis.relations.max_constraints as u64, count);
            assert_eq!(native.analysis.relations.max_nodes as u64, count);
            assert_eq!(native.analysis.relations.max_depth as u64, count);
            assert_eq!(native.analysis.relations.rlimit, u32::MAX);
            assert!(native.analysis.validate().is_ok());
            let rpc = rpc(
                &source(),
                "http://127.0.0.1:8545",
                evm_abstract::Fork::Osaka,
                &limits,
            )
            .unwrap();
            assert_eq!(rpc.max_accounts as u64, count);
            assert_eq!(rpc.max_requests as u64, count);
            assert_eq!(rpc.max_response_bytes as u64, count);
            assert_eq!(rpc.timeout, Duration::from_millis(count));
        }
    }

    #[test]
    fn zero_rpc_budgets_remain_invalid_before_network_acquisition() {
        for limits in [
            api::AnalysisLimits {
                rpc_max_accounts: 0,
                ..api::AnalysisLimits::default()
            },
            api::AnalysisLimits {
                rpc_max_requests: 0,
                ..api::AnalysisLimits::default()
            },
            api::AnalysisLimits {
                rpc_max_response_bytes: 0,
                ..api::AnalysisLimits::default()
            },
            api::AnalysisLimits {
                rpc_timeout_ms: 0,
                ..api::AnalysisLimits::default()
            },
        ] {
            let rpc = rpc(
                &source(),
                "http://127.0.0.1:1",
                evm_abstract::Fork::Osaka,
                &limits,
            )
            .unwrap();
            assert!(matches!(
                evm_abstract::world::rpc::load(&rpc),
                Err(evm_abstract::world::rpc::RpcError::Configuration {
                    reason: evm_abstract::world::rpc::ConfigurationReason::Limits,
                    ..
                })
            ));
        }
    }
}

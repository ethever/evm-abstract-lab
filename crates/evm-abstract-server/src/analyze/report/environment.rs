//! Project actual effective environments through the same bounded pools.
use super::{Pools, api, value};
use evm_abstract::{
    domain::{EvmValue, WordElement},
    world::{EvmEnvironmentValues, GasInput},
};

pub(super) fn project<N: WordElement>(
    environment: &EvmEnvironmentValues<N>,
    pools: &mut Pools<'_, N>,
) -> Result<api::EnvironmentSnapshot, api::ApiError> {
    pools.symbolic.charge(
        environment
            .block_hashes
            .len()
            .saturating_add(environment.blob_hashes.hashes.len()),
    )?;
    let unknown = EvmValue::<N>::top();
    let hashes =
        |table: &std::collections::BTreeMap<evm_abstract::U256, alloy_primitives::B256>| {
            table
                .iter()
                .map(|(index, hash)| api::IndexedHash {
                    index: value::word(*index),
                    hash: hash.to_string(),
                })
                .collect()
        };
    Ok(api::EnvironmentSnapshot {
        to: value::address(environment.to),
        caller: value::address(environment.caller),
        origin: value::address(environment.resolved_origin()),
        call_value: pools.symbolic.value(&environment.value)?,
        calldata: pools.buffer(&environment.calldata)?,
        is_static: environment.is_static,
        gas_upper_bound: match environment.gas {
            GasInput::Unknown => None,
            GasInput::UpperBound(bound) => Some(value::word(bound)),
        },
        gas_price: pools.symbolic.value(&environment.gas_price)?,
        coinbase: value::address(environment.coinbase),
        timestamp: pools.symbolic.value(&environment.timestamp)?,
        number: pools.symbolic.value(&environment.number)?,
        prevrandao: pools.symbolic.value(&environment.prevrandao)?,
        gas_limit: pools.symbolic.value(&environment.gas_limit)?,
        chain_id: pools
            .symbolic
            .value(environment.chain_id.as_ref().unwrap_or(&unknown))?,
        base_fee: pools.symbolic.value(&environment.base_fee)?,
        blob_base_fee: pools.symbolic.value(&environment.blob_base_fee)?,
        block_hashes: hashes(&environment.block_hashes),
        blob_hashes: hashes(&environment.blob_hashes.hashes),
        blob_count: pools.symbolic.value(&environment.blob_hashes.length)?,
    })
}

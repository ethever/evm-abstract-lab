//! User-visible EVM inputs keep unknown values and address aliases explicit.

use crate::{
    analysis::WorldAnalysis,
    domain::Value,
    world::{AddressInput, EvmEnvironment, GasInput, SnapshotIdentity},
};
use alloy_primitives::Address;
use std::{collections::BTreeSet, fmt::Write};

pub(super) fn owner_alias(analysis: &WorldAnalysis) -> Option<(Address, String)> {
    let input = analysis.entry().environment.to;
    input
        .as_concrete()
        .is_none()
        .then(|| (analysis.entry().address, input.to_string()))
}

pub(super) fn owner(analysis: &WorldAnalysis, address: Address) -> String {
    owner_alias(analysis)
        .filter(|(internal, _)| *internal == address)
        .map_or_else(|| address.to_string(), |(_, label)| label)
}

pub(super) fn collect_addresses(environment: &EvmEnvironment, addresses: &mut BTreeSet<Address>) {
    addresses.extend(
        [
            environment.to,
            environment.caller,
            environment.resolved_origin(),
            environment.coinbase,
        ]
        .into_iter()
        .filter_map(|input| input.as_concrete()),
    );
}

/// Full reports include every field; teaching reports add only concrete overrides
/// to the root inputs, so a symbolic block environment does not bury the graph.
pub(crate) fn write(
    output: &mut String,
    environment: &EvmEnvironment,
    identity: Option<&SnapshotIdentity>,
    full: bool,
    address: impl Fn(AddressInput) -> String,
) {
    output.push_str("\nEVM inputs\n");
    writeln!(
        output,
        "  to={} | caller={} | origin={}{}",
        address(environment.to),
        address(environment.caller),
        address(environment.resolved_origin()),
        if environment.origin.is_none() {
            " (same as caller)"
        } else {
            ""
        },
    )
    .unwrap();
    writeln!(
        output,
        "  value={} | static={}",
        environment.value, environment.is_static
    )
    .unwrap();
    let content = environment.calldata.exact_bytes_bounded(64).map_or_else(
        || {
            format!(
                "abstract bytes (default={})",
                environment.calldata.default_byte()
            )
        },
        |bytes| format!("0x{}", alloy_primitives::hex::encode(bytes)),
    );
    writeln!(
        output,
        "  calldata: length={} | content={content}",
        environment.calldata.len()
    )
    .unwrap();
    if full {
        super::text::bytes::write_bytes(
            output,
            "  ",
            "calldata observations",
            &environment.calldata,
        );
    }
    for (name, value) in [
        ("gas price", &environment.gas_price),
        ("timestamp", &environment.timestamp),
        ("block number", &environment.number),
        ("prevrandao", &environment.prevrandao),
        ("block gas limit", &environment.gas_limit),
        ("base fee", &environment.base_fee),
        ("blob base fee", &environment.blob_base_fee),
    ] {
        if full || value != &Value::top() {
            writeln!(output, "  {name}={value}").unwrap();
        }
    }
    if full || environment.coinbase != AddressInput::unknown_coinbase() {
        writeln!(output, "  coinbase={}", address(environment.coinbase)).unwrap();
    }
    let chain_id = environment
        .chain_id
        .as_ref()
        .map(ToString::to_string)
        .or_else(|| {
            if let Some(SnapshotIdentity::Chain { chain_id, .. }) = identity {
                Some(format!("{chain_id:#x} (RPC snapshot)"))
            } else {
                None
            }
        });
    if full || chain_id.is_some() {
        writeln!(
            output,
            "  chain id={}",
            chain_id.unwrap_or_else(|| "symbolic(chain_id)".to_owned())
        )
        .unwrap();
    }
    match environment.gas {
        GasInput::UpperBound(bound) => writeln!(output, "  gas: upper bound={bound:#x}").unwrap(),
        GasInput::Unknown if full => output.push_str("  gas: unknown\n"),
        GasInput::Unknown => {}
    }
    if full || !environment.block_hashes.is_empty() {
        writeln!(
            output,
            "  configured block hashes={} | other valid heights=unknown",
            environment.block_hashes.len()
        )
        .unwrap();
        for (number, hash) in &environment.block_hashes {
            writeln!(output, "    {number:#x}: {hash}").unwrap();
        }
    }
    if full
        || environment.blob_hashes.length != Value::top()
        || !environment.blob_hashes.hashes.is_empty()
    {
        writeln!(
            output,
            "  blob hashes: length={} | configured={} | other valid indices=unknown",
            environment.blob_hashes.length,
            environment.blob_hashes.hashes.len()
        )
        .unwrap();
        for (index, hash) in &environment.blob_hashes.hashes {
            writeln!(output, "    {index:#x}: {hash}").unwrap();
        }
    }
    if !full {
        output.push_str("  Other EVM environment inputs remain symbolic unless shown above; full inputs are available in the verbose report.\n");
    }
}

pub(crate) fn summary(environment: &EvmEnvironment, identity: Option<&SnapshotIdentity>) -> String {
    let mut output = String::new();
    write(&mut output, environment, identity, true, |input| {
        input.to_string()
    });
    output.trim().to_owned()
}

#[cfg(test)]
mod tests;

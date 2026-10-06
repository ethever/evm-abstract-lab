//! Cross-contract graph output names both executable code and state ownership.
//!
//! Stable machine-state identifiers join text, JSON and DOT views. Call depth
//! and caller remain visible so a shared implementation never looks like one
//! shared account, and frontiers remain visible even when analysis is partial.

use crate::{
    analysis::WorldAnalysis,
    world::{Existence, SnapshotIdentity, Store},
};
use alloy_primitives::{Address, B256, keccak256};
use serde::Serialize;
use std::fmt::Write;

mod text;
pub use text::render as text;

/// DOT graph preserving call/return/revert/failure edge labels and frontiers.
pub fn dot(analysis: &WorldAnalysis) -> String {
    let mut output = String::from("digraph world {\n  rankdir=LR;\n");
    writeln!(
        output,
        "  label={:?};",
        format!(
            "fork={} status={:?} identity={}\nfingerprint={}\nsummaries hits={} misses={} published={} imported={}",
            analysis.world().fork(),
            analysis.status(),
            identity(analysis.world().identity()),
            analysis.world().fingerprint(),
            analysis.summary_stats().hits,
            analysis.summary_stats().misses,
            analysis.summary_stats().published,
            analysis.summary_stats().imported_states,
        )
    )
    .unwrap();
    for state in analysis.states() {
        let frame = state.active();
        let label = format!(
            "S{} depth={} B{} mode={:?}\ncode={}\ncode_hash={}\naddress={}\ncaller={} static={}",
            state.id,
            state.key.frames.len(),
            frame.basic_block_index,
            frame.mode,
            frame.code_address,
            frame.code_hash,
            frame.address,
            frame.caller,
            frame.is_static
        );
        writeln!(output, "  S{} [label={label:?}, shape=box];", state.id).unwrap();
    }
    for edge in analysis.edges() {
        writeln!(
            output,
            "  S{} -> S{} [label={:?}];",
            edge.from,
            edge.to,
            format!("{:?}", edge.kind)
        )
        .unwrap();
    }
    for (index, frontier) in analysis.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  F{index} [label={:?}, shape=diamond, color=red];",
            format!("{:?} pc={:?}", frontier.reason, frontier.pc)
        )
        .unwrap();
        if let Some(from) = frontier.from {
            writeln!(output, "  S{from} -> F{index} [style=dashed];").unwrap();
        }
    }
    for (index, outcome) in analysis.outcomes().iter().enumerate() {
        let accounts = observations(&outcome.store)
            .iter()
            .map(|account| {
                format!(
                    "{} {:?} nonce={} code_size={:?}",
                    account.address, account.existence, account.nonce, account.code_size
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        writeln!(
            output,
            "  O{index} [label={:?}, shape=oval];",
            format!(
                "{:?} returndata_len={}\n{accounts}",
                outcome.kind,
                outcome.data.len()
            )
        )
        .unwrap();
        writeln!(
            output,
            "  S{} -> O{index} [label={:?}];",
            outcome.state,
            format!("{:?}", outcome.kind)
        )
        .unwrap();
    }
    for (index, record) in analysis.summaries().iter().enumerate() {
        writeln!(
            output,
            "  C{index} [label={:?}, shape=note, color=blue];",
            format!(
                "summary#{index} states={} edges={} outputs={}\ncode_hash={}\nreused_at={:?}",
                record.state_count,
                record.edge_count,
                record.outputs.len(),
                hash_label(record.input.code_hash),
                record.reused_at
            )
        )
        .unwrap();
        writeln!(
            output,
            "  C{index} -> S{} [style=dashed, color=blue, label=\"certified\"];",
            record.source_state
        )
        .unwrap();
        for state in &record.reused_at {
            writeln!(
                output,
                "  C{index} -> S{state} [style=dotted, color=blue, label=\"reused\"];"
            )
            .unwrap();
        }
    }
    output.push_str("}\n");
    output
}

/// JSON evidence retains the analysis schema and adds computed snapshot/account facts.
pub fn json(analysis: &WorldAnalysis) -> Result<serde_json::Value, serde_json::Error> {
    let mut result = serde_json::to_value(analysis)?;
    result["world"]["fingerprint"] = serde_json::to_value(analysis.world().fingerprint())?;
    let hashes: std::collections::BTreeMap<_, _> = analysis
        .world()
        .accounts()
        .keys()
        .map(|address| (*address, analysis.world().code_hash(*address)))
        .collect();
    result["world"]["code_hashes"] = serde_json::to_value(hashes)?;
    for (input, output) in analysis.outcomes().iter().zip(
        result["outcomes"]
            .as_array_mut()
            .expect("serialized outcomes are an array"),
    ) {
        output["store"]["account_observations"] = serde_json::to_value(observations(&input.store))?;
    }
    Ok(result)
}

fn identity(identity: &SnapshotIdentity) -> String {
    match identity {
        SnapshotIdentity::Offline { label } => format!("offline:{label:?}"),
        SnapshotIdentity::Chain {
            chain_id,
            block_hash,
        } => format!("chain:{chain_id:#x} block:{block_hash}"),
    }
}

fn hash_label(hash: Option<B256>) -> String {
    hash.map_or_else(|| "unknown".to_owned(), |hash| hash.to_string())
}

#[derive(Serialize)]
struct AccountObservation {
    address: Address,
    balance: crate::domain::Value,
    nonce: crate::domain::Value,
    existence: Existence,
    code_hash: Option<B256>,
    code_size: Option<usize>,
    created_in_transaction: Option<bool>,
    pending_destruction: Option<bool>,
}

fn observations(store: &Store) -> Vec<AccountObservation> {
    store
        .addresses()
        .into_iter()
        .map(|address| {
            let code = store.raw_account_code(address);
            let code_hash = if store.existence(address) == Existence::Absent {
                Some(B256::ZERO)
            } else {
                code.as_ref().map(keccak256)
            };
            AccountObservation {
                address,
                balance: store.read_balance(address),
                nonce: store.nonce(address),
                existence: store.existence(address),
                code_hash,
                code_size: code.map(|bytes| bytes.len()),
                created_in_transaction: store.created_in_transaction(address),
                pending_destruction: store.pending_destruction(address),
            }
        })
        .collect()
}

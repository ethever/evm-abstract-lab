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

/// Human-readable execution graph, outcomes, state effects and open frontiers.
pub fn text(analysis: &WorldAnalysis) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "world fork={} provenance={:?} identity={} fingerprint={}",
        analysis.world().fork(),
        analysis.world().provenance(),
        identity(analysis.world().identity()),
        analysis.world().fingerprint(),
    )
    .unwrap();
    let stats = analysis.summary_stats();
    writeln!(output, "summaries enabled={} hits={} misses={} published={} rejected_incomplete={} imported_states={}",
        analysis.config().use_summaries, stats.hits, stats.misses, stats.published,
        stats.rejected_incomplete, stats.imported_states).unwrap();
    writeln!(
        output,
        "entry={} caller={} static={} value={}",
        analysis.entry().address,
        analysis.entry().caller,
        analysis.entry().is_static,
        analysis.entry().value
    )
    .unwrap();
    writeln!(
        output,
        "status={:?} states={} transfers={} work={}",
        analysis.status(),
        analysis.states().len(),
        analysis.transfers(),
        analysis.work()
    )
    .unwrap();
    for state in analysis.states() {
        let frame = state.active();
        writeln!(
            output,
            "S{} | depth={} | code={} | address={} | caller={} | static={} | mode={:?} | code_hash={} | B{} | stack height={} | pcs={:?}",
            state.id,
            state.key.frames.len(),
            frame.code_address,
            frame.address,
            frame.caller,
            frame.is_static,
            frame.mode,
            frame.code_hash,
            frame.basic_block_index,
            frame.stack_height,
            state.executed_pcs
        )
        .unwrap();
        let payload = state.entry.active();
        writeln!(
            output,
            "  stack in {} value={} calldata_len={} memory_len={} returndata_len={}",
            super::stack(&payload.stack),
            payload.call_value,
            payload.calldata.len(),
            payload.memory.len(),
            payload.returndata.len()
        )
        .unwrap();
        writeln!(output, "  stack out {}", super::stack(&state.exit_stack)).unwrap();
    }
    for edge in analysis.edges() {
        writeln!(output, "S{} -> S{} {:?}", edge.from, edge.to, edge.kind).unwrap();
    }
    for outcome in analysis.outcomes() {
        writeln!(
            output,
            "outcome S{} {:?} returndata={:?}",
            outcome.state, outcome.kind, outcome.data
        )
        .unwrap();
        for ((address, slot), value) in outcome.store.slots() {
            writeln!(output, "  storage[{address}][{slot:#x}]={value}").unwrap();
        }
        for account in observations(&outcome.store) {
            writeln!(
                output,
                "  account {} balance={} nonce={} existence={:?} code_hash={} code_size={:?} created={:?} pending_destruction={:?}",
                account.address, account.balance, account.nonce, account.existence,
                hash_label(account.code_hash), account.code_size, account.created_in_transaction,
                account.pending_destruction,
            )
            .unwrap();
        }
        for (site, log) in outcome.store.possible_logs() {
            writeln!(
                output,
                "  possible_log address={} code={} pc=0x{:x} topics={} data={:?}",
                site.address,
                site.code_address,
                site.pc,
                super::stack(&log.topics),
                log.data
            )
            .unwrap();
        }
        if outcome.store.logs_unknown() {
            output.push_str("  logs_unknown=true\n");
        }
    }
    for (index, record) in analysis.summaries().iter().enumerate() {
        writeln!(
            output,
            "summary#{index} source=S{} states={} edges={} outputs={} code_hash={} reused_at={:?}",
            record.source_state,
            record.state_count,
            record.edge_count,
            record.outputs.len(),
            hash_label(record.input.code_hash),
            record.reused_at
        )
        .unwrap();
        for result in &record.outputs {
            writeln!(
                output,
                "  summary_output {:?} returndata_len={} code_identity={}",
                result.kind,
                result.data.len(),
                result.store.code_identity()
            )
            .unwrap();
        }
    }
    for diagnostic in analysis.diagnostics() {
        writeln!(output, "diagnostic {diagnostic:?}").unwrap();
    }
    for frontier in analysis.frontiers() {
        writeln!(output, "frontier {frontier:?}").unwrap();
    }
    output
}

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

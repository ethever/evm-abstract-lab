//! Complete world reports, grouped by identity, graph evidence and outcomes.

use super::{identity, observations};
use crate::{
    analysis::{FrameCode, FrameKey, WorldAnalysis},
    domain::Value,
    world::Store,
};
use std::fmt::{Display, Write};

mod bytes;
mod references;
mod table;

use bytes::write_bytes;
use references::References;
use table::write_table;

/// Render every recorded state, outcome and frontier with stable identity references.
///
/// References reduce repeated long addresses/hashes without truncating them.
/// Outcomes remain separate: equal returned bytes do not imply equal state effects.
pub fn render(analysis: &WorldAnalysis) -> String {
    let refs = References::new(analysis);
    let mut output = String::new();
    output.push_str("Analysis\n");
    writeln!(
        output,
        "  status={:?} | states={} | edges={} | outcomes={}",
        analysis.status(),
        analysis.states().len(),
        analysis.edges().len(),
        analysis.outcomes().len()
    )
    .unwrap();
    writeln!(
        output,
        "  fork={} | transfers={} | work={}",
        analysis.world().fork(),
        analysis.transfers(),
        analysis.work()
    )
    .unwrap();
    writeln!(
        output,
        "  entry={} | caller={} | static={} | value={}",
        refs.address(analysis.entry().address),
        refs.address(analysis.entry().caller),
        analysis.entry().is_static,
        analysis.entry().value
    )
    .unwrap();

    let spec = analysis
        .config()
        .domain()
        .expect("analysis carries validated configuration")
        .spec();
    writeln!(
        output,
        "  domain={:?} | schema=1 | reduction rounds={} | fact atoms={}",
        spec.profile(),
        spec.reduction_rounds(),
        spec.fact_limit()
    )
    .unwrap();
    output.push_str("\nSnapshot\n");
    writeln!(output, "  provenance={:?}", analysis.world().provenance()).unwrap();
    writeln!(
        output,
        "  identity={}",
        identity(analysis.world().identity())
    )
    .unwrap();
    writeln!(output, "  fingerprint={}", analysis.world().fingerprint()).unwrap();

    output.push_str("\nReferences\n\nAddresses\n");
    write_table(
        &mut output,
        "  ",
        &["Ref", "Address"],
        &refs
            .addresses
            .iter()
            .map(|(address, id)| vec![id.clone(), address.to_string()])
            .collect::<Vec<_>>(),
    );
    output.push_str("\nHashes\n");
    write_table(
        &mut output,
        "  ",
        &["Ref", "Hash"],
        &refs
            .hashes
            .iter()
            .map(|(hash, id)| vec![id.clone(), hash.to_string()])
            .collect::<Vec<_>>(),
    );

    output.push_str("\nStates\n");
    let states = analysis
        .states()
        .iter()
        .map(|state| {
            let mut row = vec![format!("S{}", state.id), state.key.frames.len().to_string()];
            row.extend(frame_cells(state.active(), &refs));
            row
        })
        .collect::<Vec<_>>();
    write_table(
        &mut output,
        "  ",
        &[
            "State",
            "Depth",
            "Block",
            "Stack height",
            "Code",
            "Address",
            "Caller",
            "Static",
            "Mode",
            "Code hash",
        ],
        &states,
    );

    output.push_str("\nState details\n");
    for state in analysis.states() {
        let frame = state.entry.active();
        writeln!(output, "  S{}", state.id).unwrap();
        write_table(
            &mut output,
            "    ",
            &["Field", "Value"],
            &[
                vec!["stack in".to_owned(), super::super::stack(&frame.stack)],
                vec![
                    "stack out".to_owned(),
                    super::super::stack(&state.exit_stack),
                ],
                vec!["call value".to_owned(), frame.call_value.to_string()],
                vec!["calldata length".to_owned(), length(frame.calldata.len())],
                vec!["memory length".to_owned(), length(frame.memory.len())],
                vec![
                    "returndata length".to_owned(),
                    length(frame.returndata.len()),
                ],
                vec!["pcs".to_owned(), format!("{:?}", state.executed_pcs)],
                vec![
                    "jump history".to_owned(),
                    format!("{:?}", frame.key.jump_history),
                ],
            ],
        );
    }

    output.push_str("\nTransitions\n");
    write_table(
        &mut output,
        "  ",
        &["From", "To", "Kind"],
        &analysis
            .edges()
            .iter()
            .map(|edge| {
                vec![
                    format!("S{}", edge.from),
                    format!("S{}", edge.to),
                    format!("{:?}", edge.kind),
                ]
            })
            .collect::<Vec<_>>(),
    );

    output.push_str("\nOutcomes\n");
    if analysis.outcomes().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, outcome) in analysis.outcomes().iter().enumerate() {
        writeln!(
            output,
            "  O{index} | S{} | {:?}",
            outcome.state, outcome.kind
        )
        .unwrap();
        write_bytes(&mut output, "    ", "returndata", &outcome.data);
        write_store(&mut output, &outcome.store, &refs);
        output.push('\n');
    }

    output.push_str("Call summaries\n");
    output.push_str("  Reuse completed callee analysis under equal input conditions; see docs/10-snapshots-summaries-creation.md.\n");
    let stats = analysis.summary_stats();
    writeln!(
        output,
        "  enabled={} | hits={} | misses={} | published={}",
        analysis.config().use_summaries,
        stats.hits,
        stats.misses,
        stats.published
    )
    .unwrap();
    writeln!(
        output,
        "  rejected_incomplete={} | imported_states={}",
        stats.rejected_incomplete, stats.imported_states
    )
    .unwrap();
    if analysis.summaries().is_empty() {
        output.push_str("  (no published summaries)\n");
    }
    for (index, record) in analysis.summaries().iter().enumerate() {
        let reused = record
            .reused_at
            .iter()
            .map(|id| format!("S{id}"))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            output,
            "  summary#{index} | source=S{} | reused_at=[{reused}]",
            record.source_state
        )
        .unwrap();
        write_table(
            &mut output,
            "    ",
            &["Field", "Value"],
            &[
                vec!["states".to_owned(), record.state_count.to_string()],
                vec!["edges".to_owned(), record.edge_count.to_string()],
                vec!["outputs".to_owned(), record.outputs.len().to_string()],
                vec![
                    "code_hash".to_owned(),
                    refs.hash_option(record.input.code_hash),
                ],
            ],
        );
        write_table(
            &mut output,
            "    ",
            &["Result", "Kind", "Returndata length", "Code identity"],
            &record
                .outputs
                .iter()
                .enumerate()
                .map(|(id, result)| {
                    vec![
                        id.to_string(),
                        format!("{:?}", result.kind),
                        length(result.data.len()),
                        refs.hash(result.store.code_identity()).to_owned(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }

    output.push_str("\nDiagnostics\n");
    write_table(
        &mut output,
        "  ",
        &["State", "PC", "Kind"],
        &analysis
            .diagnostics()
            .iter()
            .map(|diagnostic| {
                vec![
                    format!("S{}", diagnostic.state),
                    format!("0x{:x}", diagnostic.pc),
                    format!("{:?}", diagnostic.kind),
                ]
            })
            .collect::<Vec<_>>(),
    );

    output.push_str("\nFrontiers\n");
    if analysis.frontiers().is_empty() {
        output.push_str("  (none)\n");
    }
    for (index, frontier) in analysis.frontiers().iter().enumerate() {
        writeln!(
            output,
            "  F{index} | from={} | pc={} | reason={:?}",
            frontier
                .from
                .map_or_else(|| "none".to_owned(), |id| format!("S{id}")),
            frontier
                .pc
                .map_or_else(|| "none".to_owned(), |pc| format!("0x{pc:x}")),
            frontier.reason
        )
        .unwrap();
        if let Some(target) = &frontier.target {
            writeln!(
                output,
                "    target code_identity={}",
                refs.hash(target.code_identity)
            )
            .unwrap();
            let rows = target
                .frames
                .iter()
                .enumerate()
                .map(|(index, frame)| {
                    let mut row = vec![index.to_string()];
                    row.extend(frame_cells(frame, &refs));
                    row.push(format!("{:?}", frame.jump_history));
                    row
                })
                .collect::<Vec<_>>();
            write_table(
                &mut output,
                "    ",
                &[
                    "Frame",
                    "Block",
                    "Stack height",
                    "Code",
                    "Address",
                    "Caller",
                    "Static",
                    "Mode",
                    "Code hash",
                    "Jump history",
                ],
                &rows,
            );
        } else {
            output.push_str("    target=none\n");
        }
    }
    output
}

fn frame_cells(frame: &FrameKey, refs: &References) -> Vec<String> {
    vec![
        format!("B{}", frame.basic_block_index),
        frame.stack_height.to_string(),
        refs.address(frame.code_address).to_owned(),
        refs.address(frame.address).to_owned(),
        refs.address(frame.caller).to_owned(),
        frame.is_static.to_string(),
        match frame.mode {
            FrameCode::Precompile(address) => format!("Precompile({})", refs.address(address)),
            mode => format!("{mode:?}"),
        },
        refs.hash(frame.code_hash).to_owned(),
    ]
}

fn write_store(output: &mut String, store: &Store, refs: &References) {
    output.push_str("    Storage\n");
    write_table(
        output,
        "      ",
        &["Address", "Slot", "Value"],
        &store
            .slots()
            .iter()
            .map(|((address, slot), value)| {
                vec![
                    refs.address(*address).to_owned(),
                    format!("{slot:#x}"),
                    value.to_string(),
                ]
            })
            .collect::<Vec<_>>(),
    );
    output.push_str("    Account observations\n");
    write_table(
        output,
        "      ",
        &[
            "Address",
            "Balance",
            "Nonce",
            "Existence",
            "Code hash",
            "Code size",
            "Created",
            "Pending destruction",
        ],
        &observations(store)
            .into_iter()
            .map(|account| {
                vec![
                    refs.address(account.address).to_owned(),
                    account.balance.to_string(),
                    account.nonce.to_string(),
                    format!("{:?}", account.existence),
                    refs.hash_option(account.code_hash),
                    optional(account.code_size),
                    optional(account.created_in_transaction),
                    optional(account.pending_destruction),
                ]
            })
            .collect::<Vec<_>>(),
    );
    output.push_str("    Possible logs\n");
    if store.possible_logs().is_empty() {
        output.push_str("      (none)\n");
    }
    for (index, (site, log)) in store.possible_logs().iter().enumerate() {
        writeln!(
            output,
            "      L{index} | address={} | code={} | pc=0x{:x}",
            refs.address(site.address),
            refs.address(site.code_address),
            site.pc,
        )
        .unwrap();
        write_table(
            output,
            "        ",
            &["Field", "Value"],
            &[vec!["topics".to_owned(), super::super::stack(&log.topics)]],
        );
        write_bytes(output, "        ", "data", &log.data);
    }
    writeln!(output, "      logs_unknown={}", store.logs_unknown()).unwrap();
}

fn optional<T: Display>(value: Option<T>) -> String {
    value.map_or_else(|| "unknown".to_owned(), |value| value.to_string())
}

fn length(value: &Value) -> String {
    match value.constants() {
        Some(values) if values.len() == 1 => {
            format!("{} bytes", values.first().expect("nonempty constants"))
        }
        _ => format!("{value} bytes"),
    }
}

#[cfg(test)]
mod tests;

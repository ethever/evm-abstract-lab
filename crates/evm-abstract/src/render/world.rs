//! Cross-contract graph output names both executable code and state ownership.
//!
//! Stable machine-state identifiers join text, JSON and DOT views. Call depth
//! and caller remain visible so a shared implementation never looks like one
//! shared account, and frontiers remain visible even when analysis is partial.

use crate::analysis::WorldAnalysis;
use std::fmt::Write;

/// Human-readable execution graph, outcomes, state effects and open frontiers.
pub fn text(analysis: &WorldAnalysis) -> String {
    let mut output = String::new();
    writeln!(
        output,
        "world fork={} provenance={:?}",
        analysis.world().fork(),
        analysis.world().provenance()
    )
    .unwrap();
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
            "S{} depth={} code={} address={} caller={} static={} B{} height={} pcs={:?}",
            state.id,
            state.key.frames.len(),
            frame.code_address,
            frame.address,
            frame.caller,
            frame.is_static,
            frame.block,
            frame.stack_height,
            state.executed_pcs
        )
        .unwrap();
        let payload = state.entry.active();
        writeln!(
            output,
            "  in {} value={} calldata_len={} memory_len={} returndata_len={}",
            super::stack(&payload.stack),
            payload.call_value,
            payload.calldata.len(),
            payload.memory.len(),
            payload.returndata.len()
        )
        .unwrap();
        writeln!(output, "  out {}", super::stack(&state.exit_stack)).unwrap();
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
        for address in analysis.world().accounts().keys() {
            writeln!(
                output,
                "  balance[{address}]={}",
                outcome.store.read_balance(*address)
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
            "fork={} status={:?} provenance={}",
            analysis.world().fork(),
            analysis.status(),
            analysis.world().provenance()
        )
    )
    .unwrap();
    for state in analysis.states() {
        let frame = state.active();
        let label = format!(
            "S{} depth={} B{}\ncode={}\naddress={}\ncaller={} static={}",
            state.id,
            state.key.frames.len(),
            frame.block,
            frame.code_address,
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
        writeln!(
            output,
            "  O{index} [label={:?}, shape=oval];",
            format!("{:?} returndata_len={}", outcome.kind, outcome.data.len())
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
    output.push_str("}\n");
    output
}

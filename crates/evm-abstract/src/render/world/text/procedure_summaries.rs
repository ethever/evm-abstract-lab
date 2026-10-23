//! Provenance and closure of abstract interaction relations.
mod regional;

use crate::analysis::EvmWorldAnalysis;
use crate::domain::WordElement;
use evm_abstract_notation::Symbol;
use std::collections::BTreeMap;
use std::fmt::Write;

pub(in crate::render::world) fn write<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
) {
    if analysis.procedure_summaries().is_empty() {
        return;
    }
    output.push_str("\nProcedure summaries\n");
    output.push_str("  Relations for known bytecode calls, recursion and unknown interactions; regional flow has separate identities and granularity from native CFG/SSA.\n");
    let observations = analysis.procedure_summaries();
    let latest: BTreeMap<_, _> = observations
        .iter()
        .enumerate()
        .map(|(index, record)| (record.source_state, index))
        .collect();
    output.push_str("  Closure describes each observed input snapshot; final-source currency requires its source certificate.\n");
    for (index, record) in observations.iter().enumerate() {
        let superseded = latest[&record.source_state] != index;
        let scope = if superseded {
            "historical observation"
        } else if record.source_current {
            "final source certified"
        } else {
            "final source uncertified"
        };
        writeln!(
            output,
            "  relation {index} | {:?} | root={:?} | source={} | source_current={} | scope={scope} | closed={} | procedures={} | states={} | transfers={} | propagations={} | reused={}",
            record.kind, record.root_procedure, Symbol::State(record.source_state), record.source_current, record.closed, record.procedures.len(),
            record.state_count, record.transfers, record.propagations, record.reused,
        ).unwrap();
        for (index, procedure) in record.procedures.iter().enumerate() {
            writeln!(
                output,
                "    procedure {index} | {:?} | code={} | storage namespace={} | static={} | historically visited blocks={:?}",
                procedure.frame.mode, procedure.frame.code_address, procedure.frame.address,
                procedure.frame.is_static, procedure.covered_blocks,
            ).unwrap();
            writeln!(
                output,
                "      dependencies={:?} | input updates={} | output updates={} | outcomes={:?}",
                procedure.dependencies,
                procedure.input_updates,
                procedure.output_updates,
                procedure
                    .outputs
                    .iter()
                    .map(|output| output.kind)
                    .collect::<Vec<_>>(),
            )
            .unwrap();
        }
        regional::write(
            output,
            &record.regional_cfg,
            record.source_current,
            superseded,
        );
    }
}

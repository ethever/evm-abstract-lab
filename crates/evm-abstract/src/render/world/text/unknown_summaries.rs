//! Provenance and closure of abstract interaction relations.

use crate::analysis::WorldAnalysis;
use evm_abstract_notation::Symbol;
use std::fmt::Write;

pub(in crate::render::world) fn write(output: &mut String, analysis: &WorldAnalysis) {
    if analysis.unknown_summaries().is_empty() {
        return;
    }
    output.push_str("\nAbstract interaction summaries\n");
    output.push_str("  Numerical bounds for known/unknown interactions; summarized blocks are not expanded CFG traces.\n");
    for (index, record) in analysis.unknown_summaries().iter().enumerate() {
        writeln!(
            output,
            "  relation {index} | source={} | closed={} | procedures={} | states={} | transfers={} | propagations={} | reused={}",
            Symbol::State(record.source_state), record.closed, record.procedures.len(),
            record.state_count, record.transfers, record.propagations, record.reused,
        ).unwrap();
        for (index, procedure) in record.procedures.iter().enumerate() {
            writeln!(
                output,
                "    procedure {index} | {:?} | code={} | storage namespace={} | static={} | blocks={:?}",
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
    }
}

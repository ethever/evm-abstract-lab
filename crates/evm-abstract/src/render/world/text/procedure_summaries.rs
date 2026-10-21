//! Provenance and closure of abstract interaction relations.

use crate::analysis::EvmWorldAnalysis;
use crate::domain::WordElement;
use evm_abstract_notation::Symbol;
use std::fmt::Write;

pub(in crate::render::world) fn write<N: WordElement>(
    output: &mut String,
    analysis: &EvmWorldAnalysis<N>,
) {
    if analysis.procedure_summaries().is_empty() {
        return;
    }
    output.push_str("\nProcedure summaries\n");
    output.push_str("  Numerical bounds for recursive bytecode and unknown interactions; summarized blocks are not expanded CFG traces.\n");
    for (index, record) in analysis.procedure_summaries().iter().enumerate() {
        writeln!(
            output,
            "  relation {index} | {:?} | root={:?} | source={} | closed={} | procedures={} | states={} | transfers={} | propagations={} | reused={}",
            record.kind, record.root_procedure, Symbol::State(record.source_state), record.closed, record.procedures.len(),
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

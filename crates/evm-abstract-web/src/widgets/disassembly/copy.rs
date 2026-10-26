//! Clipboard exports retain exact source identity and complete operands.

use evm_abstract_protocol::{DisasmInstruction, ProgramInfo};

use crate::{
    app::{Selection, model::ReportViewId},
    report::ReportView,
};

pub(super) fn instruction(instruction: &DisasmInstruction) -> String {
    format!(
        "0x{:04x}  {}{}",
        instruction.pc,
        instruction.name,
        instruction
            .immediate
            .as_ref()
            .map_or_else(String::new, |value| format!(" {value}")),
    )
}

pub(super) fn code(source: &ProgramInfo) -> String {
    source
        .blocks
        .iter()
        .flat_map(|block| block.instructions.iter())
        .map(instruction)
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn reference(
    report: &ReportView<'_>,
    source: &ProgramInfo,
    selection: Selection,
) -> String {
    source_reference(source, report.identity(), selection.pc, selection.state)
}

pub(super) fn source_reference(
    source: &ProgramInfo,
    identity: ReportViewId,
    pc: Option<usize>,
    state: Option<usize>,
) -> String {
    let mut result = format!(
        "Source: {}\nHash: {}\nKind: {:?}\nFork: {:?}\nView: {:?}",
        source
            .code_address
            .as_deref()
            .unwrap_or("Supplied input program"),
        source.code_hash,
        source.kind,
        source.fork,
        identity,
    );
    if let Some(pc) = pc {
        result.push_str(&format!("\nPC: 0x{pc:x}"));
    }
    if let Some(state) = state {
        result.push_str(&format!("\nState: {state}"));
    }
    result
}

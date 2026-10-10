//! One native state's existing card semantics, with bounded preview traversal.
use super::{DISASM_ROWS, NodeText, NodeView, ReportIndex, SSA_COLUMNS, SSA_ROWS, abbreviate};
use crate::palette;
use egui::Color32;
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{
    AnalysisReport, BlockCoverage, CfgBlock, DisasmInstruction, InstructionProgress, Phi, SsaBlock,
    SsaInstruction,
};

const INLINE_ITEMS: usize = 8;
const INLINE_FRAMES: usize = 2;

pub(super) fn preview(
    report: &AnalysisReport,
    index: &ReportIndex,
    block: &CfgBlock,
    view: NodeView,
) -> NodeText {
    let ssa = index.ssa(report, block.id);
    let coverage = ssa.map_or(BlockCoverage::Unexecuted, |ssa| ssa.coverage);
    let frontier = index.has_frontier(block.id);
    let title = format!(
        "{}  ·  {}{}",
        Symbol::State(block.id),
        Symbol::Block(block.basic_block),
        if coverage == BlockCoverage::Current {
            String::new()
        } else {
            format!("  {coverage:?}")
        }
    );
    let mut detail = format!(
        "{} · depth {}{}",
        block
            .start_pc
            .map_or_else(|| "no bytecode".into(), |pc| format!("0x{pc:04x}")),
        block.frame_depth,
        if frontier { " · !" } else { "" }
    );
    let lines = match view {
        NodeView::Disassembly => disassembly_preview(block, coverage),
        NodeView::Ssa => {
            detail.push_str(if ssa.is_some_and(|ssa| ssa.incoming_complete) {
                " · incoming complete"
            } else {
                " · incoming open"
            });
            ssa_preview(ssa)
        }
    };
    NodeText::new(title, detail, lines, coverage, frontier)
}

pub(super) fn source_instruction(instruction: &DisasmInstruction) -> String {
    let operand = instruction
        .immediate
        .as_ref()
        .map_or(String::new(), |value| format!(" {}", abbreviate(value, 15)));
    format!("{:04x}  {}{operand}", instruction.pc, instruction.name)
}
fn disassembly_preview(block: &CfgBlock, coverage: BlockCoverage) -> Vec<(String, Color32)> {
    let mut lines: Vec<_> = block
        .instructions
        .iter()
        .take(DISASM_ROWS)
        .map(|instruction| {
            let color = if !instruction.valid {
                palette::ERROR
            } else if coverage == BlockCoverage::Current
                && block
                    .executed_pcs
                    .iter()
                    .take(DISASM_ROWS)
                    .any(|pc| *pc == instruction.pc)
            {
                palette::TEXT
            } else {
                palette::MUTED
            };
            (source_instruction(instruction), color)
        })
        .collect();
    if block.instructions.len() > DISASM_ROWS {
        lines.push((
            format!("+{} instructions", block.instructions.len() - DISASM_ROWS),
            palette::MUTED,
        ));
    }
    lines
}
fn ssa_preview(block: Option<&SsaBlock>) -> Vec<(String, Color32)> {
    let Some(block) = block else {
        return vec![("SSA unavailable for this state".into(), palette::WARNING)];
    };
    let mut lines = Vec::with_capacity(SSA_ROWS);
    if let Some(phi) = block.phis.first() {
        lines.push((
            abbreviate(&phi_text(phi, true), SSA_COLUMNS),
            palette::PURPLE,
        ));
    }
    lines.push((
        abbreviate(&effect_text(block, true), SSA_COLUMNS),
        palette::MUTED,
    ));
    // Stale and unexecuted receipts remain entry-only even if an old DTO kept
    // instructions. No history is promoted into current execution.
    let instructions = if block.coverage == BlockCoverage::Current {
        block.instructions.as_slice()
    } else {
        &[]
    };
    let capacity = SSA_ROWS - lines.len() - 1;
    let leading = if instructions.len() <= capacity {
        instructions.len()
    } else {
        capacity.saturating_sub(1)
    };
    for instruction in instructions.iter().take(leading) {
        lines.push(instruction_preview(instruction));
    }
    if instructions.len() > capacity
        && let Some(last) = instructions.last()
    {
        lines.push(instruction_preview(last));
    }
    let mut exit = abbreviate(&exit_text(block, true), SSA_COLUMNS);
    let hidden = block.phis.len().saturating_sub(1) + instructions.len().saturating_sub(capacity);
    if hidden > 0 {
        let suffix = format!(" · +{hidden} rows");
        exit = format!(
            "{}{suffix}",
            abbreviate(&exit, SSA_COLUMNS.saturating_sub(suffix.chars().count()))
        );
    }
    lines.push((exit, palette::MUTED));
    lines
}
fn instruction_preview(instruction: &SsaInstruction) -> (String, Color32) {
    (
        instruction_text(instruction, true),
        if instruction.fault {
            palette::ERROR
        } else if instruction.progress == InstructionProgress::Completed {
            palette::TEXT
        } else {
            palette::WARNING
        },
    )
}

pub(super) fn tooltip(
    report: &AnalysisReport,
    index: &ReportIndex,
    block: &CfgBlock,
    view: NodeView,
) -> String {
    let ssa = index.ssa(report, block.id);
    let coverage = ssa.map_or(BlockCoverage::Unexecuted, |ssa| ssa.coverage);
    let exit = match coverage {
        BlockCoverage::Current => format!(
            "Current exit stack (bottom → top): {}",
            block.exit_stack.join(", ")
        ),
        BlockCoverage::Stale => "Historical receipt is stale; no current exit is asserted.".into(),
        BlockCoverage::Unexecuted => "Unexecuted state; no exit was recorded.".into(),
    };
    let body = match view {
        NodeView::Disassembly => format!(
            "Disassembly\n{}",
            block
                .instructions
                .iter()
                .map(|instruction| format!(
                    "{:04x}  {}{}",
                    instruction.pc,
                    instruction.name,
                    instruction
                        .immediate
                        .as_ref()
                        .map_or(String::new(), |value| format!(" {value}"))
                ))
                .collect::<Vec<_>>()
                .join("\n")
        ),
        NodeView::Ssa => ssa_tooltip(report, index, block.id, ssa),
    };
    format!(
        "State {} · basic block {}\nFrame depth: {} · code: {}\nStorage owner: {}\nContext: {:?} · coverage: {:?}\nCurrent entry stack (bottom → top): {}\n{}\n\n{}",
        Symbol::State(block.id),
        Symbol::Block(block.basic_block),
        block.frame_depth,
        block.code_address,
        block.storage_address,
        block.context,
        coverage,
        block.entry_stack.join(", "),
        exit,
        body
    )
}
fn ssa_tooltip(
    report: &AnalysisReport,
    index: &ReportIndex,
    state: usize,
    block: Option<&SsaBlock>,
) -> String {
    let Some(block) = block else {
        return "No SSA block was reported for this native state.".into();
    };
    let mut full = vec![format!(
        "SSA · {:?} · {} incoming",
        block.coverage,
        if block.incoming_complete {
            "complete"
        } else {
            "open"
        }
    )];
    full.extend(block.phis.iter().map(|phi| phi_text(phi, false)));
    full.push(effect_text(block, false));
    if block.coverage == BlockCoverage::Current {
        full.extend(
            block
                .instructions
                .iter()
                .map(|instruction| instruction_text(instruction, false)),
        );
    }
    full.push(exit_text(block, false));
    for &position in index.outgoing(state) {
        let edge = &report.edges[position];
        if let Some(transition) = index.transition(report, edge.id) {
            full.push(format!(
                "{} → {} · {:?} · {}→{}{}\n  Arguments [{}] · {}",
                Symbol::Edge(edge.id),
                Symbol::State(edge.to),
                edge.kind,
                Symbol::Effect(transition.effect_input),
                Symbol::Effect(transition.effect_result),
                transition
                    .result
                    .map_or(String::new(), |result| format!(" · %{result}")),
                values(&transition.operands, false),
                frames(&transition.stacks, false)
            ));
        } else {
            full.push(format!(
                "{} → {} · deferred {:?}",
                Symbol::Edge(edge.id),
                Symbol::State(edge.to),
                index
                    .deferred(report, edge.id)
                    .map(|deferred| deferred.reason)
            ));
        }
    }
    full.join("\n")
}
fn phi_text(phi: &Phi, preview: bool) -> String {
    format!(
        "%{} = φ({})  {}/slot{}",
        phi.result,
        joined(&phi.inputs, preview, |input| format!(
            "{}:%{} ({})",
            Symbol::State(input.predecessor),
            input.value,
            Symbol::Edge(input.edge)
        )),
        Symbol::Frame(phi.frame),
        phi.slot
    )
}
fn effect_text(block: &SsaBlock, preview: bool) -> String {
    format!(
        "{} = effect φ({}){}",
        Symbol::Effect(block.effect),
        joined(&block.effect_inputs, preview, |input| format!(
            "{}:{}",
            Symbol::Edge(input.edge),
            Symbol::Effect(input.effect)
        )),
        if block.open_incoming.is_empty() {
            String::new()
        } else if preview {
            format!(" · {} open", block.open_incoming.len())
        } else {
            format!(
                " · open [{}]",
                block
                    .open_incoming
                    .iter()
                    .map(|edge| Symbol::Edge(*edge).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    )
}
fn exit_text(block: &SsaBlock, preview: bool) -> String {
    format!(
        "{} {} · {}",
        if block.coverage == BlockCoverage::Current {
            "exit"
        } else {
            "entry only"
        },
        Symbol::Effect(block.exit_effect),
        frames(&block.exit_frames, preview)
    )
}
fn instruction_text(instruction: &SsaInstruction, preview: bool) -> String {
    let results = if instruction.results.is_empty() {
        String::new()
    } else {
        format!("{} = ", values(&instruction.results, preview))
    };
    let immediate = instruction
        .immediate
        .as_ref()
        .map_or(String::new(), |value| {
            format!(
                " {}",
                if preview {
                    abbreviate(value, 18)
                } else {
                    value.clone()
                }
            )
        });
    let operands = if instruction.operands.is_empty() {
        String::new()
    } else {
        format!(" {}", values(&instruction.operands, preview))
    };
    let body = format!(
        "{:04x}  {results}{}{immediate}{operands}",
        instruction.pc, instruction.name
    );
    let phase = if preview && instruction.progress == InstructionProgress::Completed {
        String::new()
    } else {
        format!(" · {:?}", instruction.progress)
    };
    let effect = format!(
        " · {}→{}{phase}{}",
        Symbol::Effect(instruction.effect_input),
        instruction.effect_result.map_or_else(
            || "pending".into(),
            |effect| Symbol::Effect(effect).to_string()
        ),
        if instruction.fault { " · fault" } else { "" }
    );
    let body = if preview {
        abbreviate(&body, SSA_COLUMNS.saturating_sub(effect.chars().count()))
    } else {
        body
    };
    format!("{body}{effect}")
}
fn joined<T>(items: &[T], preview: bool, format: impl Fn(&T) -> String) -> String {
    let limit = if preview { INLINE_ITEMS } else { items.len() };
    let mut text = items
        .iter()
        .take(limit)
        .map(format)
        .collect::<Vec<_>>()
        .join(", ");
    if items.len() > limit {
        text.push_str(", …");
    }
    text
}
fn values(items: &[usize], preview: bool) -> String {
    joined(items, preview, |value| format!("%{value}"))
}
fn frames(stacks: &[Vec<usize>], preview: bool) -> String {
    let limit = if preview { INLINE_FRAMES } else { stacks.len() };
    let mut text = stacks
        .iter()
        .take(limit)
        .enumerate()
        .map(|(frame, stack)| {
            format!(
                "{notation_0} [{}]",
                values(stack, preview),
                notation_0 = Symbol::Frame(frame)
            )
        })
        .collect::<Vec<_>>()
        .join("  ");
    if stacks.len() > limit {
        text.push_str("  …");
    }
    text
}

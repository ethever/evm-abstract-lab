//! One native state's existing card semantics, with bounded preview traversal.
use super::{DISASM_ROWS, NodeText, NodeView, ReportIndex, SSA_COLUMNS, SSA_ROWS, abbreviate};
use crate::{
    palette,
    ssa_display::{SsaDisplay, operation_parts, source_text},
};
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
    display: SsaDisplay,
    selected_pc: Option<usize>,
) -> NodeText {
    let ssa = index.ssa(report, block.id);
    let coverage = ssa.map_or(BlockCoverage::Unexecuted, |ssa| ssa.coverage);
    let frontier = index.has_frontier(block.id);
    let abstract_environment = index
        .details(report, block.id)
        .and_then(|state| state.entry.frames.last())
        .is_some_and(|frame| frame.kind == evm_abstract_protocol::CodeKind::AbstractEnvironment);
    let location = if abstract_environment {
        "Abstract environment".into()
    } else {
        Symbol::Block(block.basic_block).to_string()
    };
    let title = format!(
        "{}  ·  {}{}",
        Symbol::State(block.id),
        location,
        if coverage == BlockCoverage::Current {
            String::new()
        } else {
            format!("  {coverage:?}")
        }
    );
    let mut detail = format!(
        "{} · depth {}{}",
        block.start_pc.map_or_else(
            || if abstract_environment {
                "abstract environment"
            } else {
                "no bytecode"
            }
            .into(),
            |pc| format!("0x{pc:04x}")
        ),
        block.frame_depth,
        if frontier { " · !" } else { "" }
    );
    let mut lines = match view {
        NodeView::Disassembly => disassembly_preview(block, coverage),
        NodeView::Ssa => {
            detail.push_str(if ssa.is_some_and(|ssa| ssa.incoming_complete) {
                " · incoming complete"
            } else {
                " · incoming open"
            });
            ssa_preview(ssa, display, selected_pc)
        }
    };
    if abstract_environment {
        lines.insert(
            0,
            ("Conservative interaction summaries".into(), palette::PURPLE),
        );
    }
    NodeText::new(title, detail, lines, coverage, frontier)
        .with_identity(vec![Symbol::State(block.id).to_string(), location])
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
fn ssa_preview(
    block: Option<&SsaBlock>,
    display: SsaDisplay,
    selected_pc: Option<usize>,
) -> Vec<(String, Color32)> {
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
    // Scan receipts once, retaining only the bounded preview plus its last and
    // selected operation. Hidden stack receipts do not consume the quota.
    let mut shown = Vec::with_capacity(capacity);
    let mut count = 0;
    let mut last = None;
    let mut selected = None;
    for (position, instruction) in instructions.iter().enumerate() {
        let is_selected = selected_pc == Some(instruction.pc);
        if !display.instruction_visible(block.coverage, instruction, is_selected) {
            continue;
        }
        count += 1;
        let row = (position, instruction);
        if shown.len() < capacity {
            shown.push(row);
        }
        last = Some(row);
        if is_selected {
            selected = Some(row);
        }
    }
    if count > capacity
        && let Some(last) = last
    {
        shown[capacity - 1] = last;
    }
    if let Some(selected) = selected
        && !shown.iter().any(|(position, _)| *position == selected.0)
    {
        // The last observed operation remains present, while the selected
        // native PC takes one existing preview slot rather than growing it.
        let replacement = shown.len().saturating_sub(2);
        shown[replacement] = selected;
        shown.sort_by_key(|(position, _)| *position);
    }
    lines.extend(
        shown
            .iter()
            .map(|(_, instruction)| instruction_preview(instruction, display)),
    );
    let mut exit = abbreviate(&exit_text(block, true), SSA_COLUMNS);
    let omitted = block.phis.len().saturating_sub(1) + count.saturating_sub(shown.len());
    let filtered = instructions.len().saturating_sub(count);
    if omitted > 0 || filtered > 0 {
        let mut suffix = String::new();
        if omitted > 0 {
            suffix.push_str(&format!(" · +{omitted} preview rows"));
        }
        if filtered > 0 {
            suffix.push_str(&format!(" · {filtered} details hidden"));
        }
        exit = format!(
            "{}{suffix}",
            abbreviate(&exit, SSA_COLUMNS.saturating_sub(suffix.chars().count()))
        );
    }
    lines.push((exit, palette::MUTED));
    lines
}
fn instruction_preview(instruction: &SsaInstruction, display: SsaDisplay) -> (String, Color32) {
    (
        instruction_text(instruction, true, display),
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
        full.extend(block.instructions.iter().map(|instruction| {
            format!(
                "{}\n  {}",
                instruction_text(instruction, false, SsaDisplay::Full),
                source_text(instruction),
            )
        }));
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
fn instruction_text(instruction: &SsaInstruction, preview: bool, display: SsaDisplay) -> String {
    let results = if instruction.results.is_empty() {
        String::new()
    } else {
        format!("{} = ", values(&instruction.results, preview))
    };
    let (name, arguments) = operation_parts(&instruction.operation);
    let operands = if arguments.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            joined(&arguments, preview, |operand| {
                let text = operand.text();
                if preview { abbreviate(&text, 18) } else { text }
            })
        )
    };
    let body = format!("{:04x}  {results}{name}{operands}", instruction.pc);
    let phase =
        if !display.show_completed() && instruction.progress == InstructionProgress::Completed {
            String::new()
        } else {
            format!(" · {:?}", instruction.progress)
        };
    let effect = if display.show_effect(instruction) {
        format!(
            " · {}→{}{phase}{}",
            Symbol::Effect(instruction.effect_input),
            instruction.effect_result.map_or_else(
                || "pending".into(),
                |effect| Symbol::Effect(effect).to_string()
            ),
            if instruction.fault { " · fault" } else { "" }
        )
    } else {
        phase
    };
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

//! Content and measurement for CFG cards. Previews are bounded, while full
//! source/SSA rows remain in the hover detail and linked source panes.

use egui::{Color32, FontId, Painter, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, BlockCoverage, CfgBlock, InstructionProgress, SsaBlock, SsaInstruction,
};

use super::{HEADER_HEIGHT, LINE_HEIGHT, PAD};
use crate::palette;

const DISASM_ROWS: usize = 4;
// A common merge has a phi, entry effect, four instructions and an exit. Keep
// those definitions together before eliding genuinely longer block bodies.
const SSA_ROWS: usize = 8;
const SSA_COLUMNS: usize = 76;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum NodeView {
    Disassembly,
    #[default]
    Ssa,
}

pub(super) struct NodeText {
    pub title: String,
    pub detail: String,
    pub lines: Vec<(String, Color32)>,
    pub coverage: BlockCoverage,
    pub frontier: bool,
    pub size: Vec2,
    pub tooltip: String,
}

pub(super) fn node_text(
    painter: &Painter,
    report: &AnalysisReport,
    block: &CfgBlock,
    view: NodeView,
) -> NodeText {
    let ssa = report.ssa.blocks.iter().find(|ssa| ssa.state == block.id);
    let coverage = ssa.map_or(BlockCoverage::Unexecuted, |ssa| ssa.coverage);
    let frontier = report
        .frontiers
        .iter()
        .any(|frontier| frontier.from == Some(block.id));
    let title = format!(
        "S{}  ·  B{}{}",
        block.id,
        block.basic_block,
        if coverage == BlockCoverage::Current {
            String::new()
        } else {
            format!("  {coverage:?}")
        }
    );
    let mut detail = format!(
        "{} · frame {}{}",
        block
            .start_pc
            .map_or_else(|| "no bytecode".into(), |pc| format!("0x{pc:04x}")),
        block.frame_depth,
        if frontier { " · !" } else { "" }
    );
    let (lines, tooltip) = match view {
        NodeView::Disassembly => disassembly(block, coverage),
        NodeView::Ssa => {
            detail.push_str(if ssa.is_some_and(|ssa| ssa.incoming_complete) {
                " · incoming complete"
            } else {
                " · incoming open"
            });
            ssa_content(report, block.id, ssa)
        }
    };
    let mut width =
        measured_width(painter, &title, 12.0).max(measured_width(painter, &detail, 10.0));
    for (line, _) in &lines {
        width = width.max(measured_width(painter, line, 11.0));
    }
    let size = Vec2::new(
        (width + PAD * 2.0).max(104.0),
        HEADER_HEIGHT + lines.len() as f32 * LINE_HEIGHT + PAD,
    );
    NodeText {
        title,
        detail,
        lines,
        coverage,
        frontier,
        size,
        tooltip,
    }
}

fn disassembly(block: &CfgBlock, coverage: BlockCoverage) -> (Vec<(String, Color32)>, String) {
    let full: Vec<_> = block
        .instructions
        .iter()
        .map(|instruction| {
            format!(
                "{:04x}  {}{}",
                instruction.pc,
                instruction.name,
                instruction
                    .immediate
                    .as_ref()
                    .map_or(String::new(), |value| format!(" {value}"))
            )
        })
        .collect();
    let mut lines: Vec<_> = block
        .instructions
        .iter()
        .take(DISASM_ROWS)
        .map(|instruction| {
            let operand = instruction
                .immediate
                .as_ref()
                .map_or(String::new(), |value| format!(" {}", abbreviate(value, 15)));
            (
                format!("{:04x}  {}{operand}", instruction.pc, instruction.name),
                if !instruction.valid {
                    palette::ERROR
                } else if coverage == BlockCoverage::Current
                    && block.executed_pcs.contains(&instruction.pc)
                {
                    palette::TEXT
                } else {
                    palette::MUTED
                },
            )
        })
        .collect();
    if block.instructions.len() > DISASM_ROWS {
        lines.push((
            format!("+{} instructions", block.instructions.len() - DISASM_ROWS),
            palette::MUTED,
        ));
    }
    (lines, format!("Disassembly\n{}", full.join("\n")))
}

fn ssa_content(
    report: &AnalysisReport,
    state: usize,
    block: Option<&SsaBlock>,
) -> (Vec<(String, Color32)>, String) {
    let Some(block) = block else {
        return (
            vec![("SSA unavailable for this state".into(), palette::WARNING)],
            "No SSA block was reported for this native state.".into(),
        );
    };
    let mut lines = Vec::new();
    let mut full = vec![format!(
        "SSA · {:?} · {} incoming",
        block.coverage,
        if block.incoming_complete {
            "complete"
        } else {
            "open"
        }
    )];
    for (index, phi) in block.phis.iter().enumerate() {
        let value = format!(
            "%{} = φ({})  f{}/slot{}",
            phi.result,
            phi.inputs
                .iter()
                .map(|input| format!("S{}:%{} (e{})", input.predecessor, input.value, input.edge))
                .collect::<Vec<_>>()
                .join(", "),
            phi.frame,
            phi.slot
        );
        if index == 0 {
            lines.push((abbreviate(&value, SSA_COLUMNS), palette::PURPLE));
        }
        full.push(value);
    }
    let effect = format!(
        "μ{} = effect φ({}){}",
        block.effect,
        block
            .effect_inputs
            .iter()
            .map(|input| format!("e{}:μ{}", input.edge, input.effect))
            .collect::<Vec<_>>()
            .join(", "),
        if block.open_incoming.is_empty() {
            String::new()
        } else {
            format!(" · open {:?}", block.open_incoming)
        }
    );
    lines.push((abbreviate(&effect, SSA_COLUMNS), palette::MUTED));
    full.push(effect);
    // A stale/unexecuted block is entry-only, even if a malformed or older DTO
    // retains instruction fields. Never present those fields as current SSA.
    let instructions = if block.coverage == BlockCoverage::Current {
        block.instructions.as_slice()
    } else {
        &[]
    };
    let capacity = SSA_ROWS - lines.len() - 1;
    for (index, instruction) in instructions.iter().enumerate() {
        let text = instruction_text(instruction, false);
        full.push(text);
        // Keep a terminal pending/call/fault phase in the preview even for a
        // long block. Phis cannot consume the entire instruction budget.
        if instructions.len() <= capacity
            || index < capacity.saturating_sub(1)
            || index + 1 == instructions.len()
        {
            lines.push((
                instruction_text(instruction, true),
                if instruction.fault {
                    palette::ERROR
                } else if instruction.progress == InstructionProgress::Completed {
                    palette::TEXT
                } else {
                    palette::WARNING
                },
            ));
        }
    }
    let exit = format!(
        "{} μ{} · {}",
        if block.coverage == BlockCoverage::Current {
            "exit"
        } else {
            "entry only"
        },
        block.exit_effect,
        frames(&block.exit_frames)
    );
    lines.push((abbreviate(&exit, SSA_COLUMNS), palette::MUTED));
    full.push(exit);
    let hidden = block.phis.len().saturating_sub(1) + instructions.len().saturating_sub(capacity);
    if hidden > 0 {
        // Keep the preview bounded while making omissions explicit.
        if let Some((line, _)) = lines.last_mut() {
            *line = format!("{} · +{hidden} rows", abbreviate(line, SSA_COLUMNS - 12));
        }
    }
    for edge in report.edges.iter().filter(|edge| edge.from == state) {
        if let Some(transition) = report
            .ssa
            .transitions
            .iter()
            .find(|transition| transition.edge == edge.id)
        {
            full.push(format!(
                "e{} → S{} · {:?} · μ{}→μ{}{}\n  Arguments [{}] · {}",
                edge.id,
                edge.to,
                edge.kind,
                transition.effect_input,
                transition.effect_result,
                transition
                    .result
                    .map_or(String::new(), |result| format!(" · %{result}")),
                values(&transition.operands),
                frames(&transition.stacks)
            ));
        } else {
            full.push(format!(
                "e{} → S{} · deferred {:?}",
                edge.id,
                edge.to,
                report
                    .ssa
                    .deferred_edges
                    .iter()
                    .find(|deferred| deferred.edge == edge.id)
                    .map(|deferred| deferred.reason)
            ));
        }
    }
    (lines, full.join("\n"))
}

fn instruction_text(instruction: &SsaInstruction, preview: bool) -> String {
    let results = if instruction.results.is_empty() {
        String::new()
    } else {
        format!("{} = ", values(&instruction.results))
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
        format!(" {}", values(&instruction.operands))
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
        " · μ{}→{}{phase}{}",
        instruction.effect_input,
        instruction
            .effect_result
            .map_or_else(|| "pending".into(), |effect| format!("μ{effect}")),
        if instruction.fault { " · fault" } else { "" }
    );
    let body = if preview {
        abbreviate(&body, SSA_COLUMNS.saturating_sub(effect.chars().count()))
    } else {
        body
    };
    format!("{body}{effect}")
}

fn values(values: &[usize]) -> String {
    values
        .iter()
        .map(|value| format!("%{value}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn frames(stacks: &[Vec<usize>]) -> String {
    stacks
        .iter()
        .enumerate()
        .map(|(frame, stack)| format!("f{frame} [{}]", values(stack)))
        .collect::<Vec<_>>()
        .join("  ")
}
fn abbreviate(text: &str, columns: usize) -> String {
    if text.chars().count() <= columns {
        text.into()
    } else {
        format!(
            "{}…",
            text.chars()
                .take(columns.saturating_sub(1))
                .collect::<String>()
        )
    }
}
fn measured_width(painter: &Painter, text: &str, size: f32) -> f32 {
    painter
        .layout_no_wrap(text.into(), FontId::monospace(size), palette::TEXT)
        .size()
        .x
        .ceil()
}

#[cfg(test)]
mod tests;

//! Source-block summaries count native instances without combining their SSA.
use super::{DisplayNode, NodeText, NodeView, ReportIndex, abbreviate, leaf};
use crate::palette;
use evm_abstract_notation::Symbol;
use evm_abstract_protocol::{AnalysisReport, BlockCoverage, CfgBlock, FrameSnapshot};

const MEMBERS_IN_TOOLTIP: usize = 12;
const SHARED_SOURCE_ROWS: usize = 3;

#[derive(Default)]
struct CountRange {
    minimum: Option<usize>,
    maximum: usize,
}
impl CountRange {
    fn add(&mut self, count: usize) {
        self.minimum = Some(self.minimum.map_or(count, |value| value.min(count)));
        self.maximum = self.maximum.max(count);
    }
    fn text(&self) -> String {
        match self.minimum {
            None => "unavailable".into(),
            Some(minimum) if minimum == self.maximum => minimum.to_string(),
            Some(minimum) => format!("{minimum}–{}", self.maximum),
        }
    }
}
#[derive(Default)]
struct Summary {
    current: usize,
    stale: usize,
    unexecuted: usize,
    frontiers: usize,
    phis: CountRange,
    operations: CountRange,
    open_incoming: usize,
    missing_ssa: usize,
    storage_varies: bool,
    depth_varies: bool,
    code_varies: bool,
    caller_varies: bool,
    address_varies: bool,
    mode_varies: bool,
    static_varies: bool,
    missing_frames: usize,
    shared_source: bool,
}
impl Summary {
    fn collect(report: &AnalysisReport, index: &ReportIndex, node: &DisplayNode) -> Self {
        let mut summary = Self {
            shared_source: true,
            ..Self::default()
        };
        let first = index.cfg(report, node.id);
        let first_frame = active_frame(report, index, node.id);
        for &state in &node.members {
            let ssa = index.ssa(report, state);
            match ssa.map_or(BlockCoverage::Unexecuted, |block| block.coverage) {
                BlockCoverage::Current => summary.current += 1,
                BlockCoverage::Stale => summary.stale += 1,
                BlockCoverage::Unexecuted => summary.unexecuted += 1,
            }
            summary.frontiers += index.frontier_count(state);
            if let Some(ssa) = ssa {
                summary.phis.add(ssa.phis.len());
                summary
                    .operations
                    .add(if ssa.coverage == BlockCoverage::Current {
                        ssa.instructions.len()
                    } else {
                        0
                    });
                summary.open_incoming += usize::from(!ssa.incoming_complete);
            } else {
                summary.missing_ssa += 1;
            }
            if let (Some(first), Some(block)) = (first, index.cfg(report, state)) {
                summary.storage_varies |= first.storage_address != block.storage_address;
                summary.depth_varies |= first.frame_depth != block.frame_depth;
                summary.code_varies |= first.code_address != block.code_address;
                // Only the source prefix shown below is compared. The immutable
                // program/block identity supplies the common source namespace.
                summary.shared_source &= first.instructions.len() == block.instructions.len()
                    && first
                        .instructions
                        .iter()
                        .take(SHARED_SOURCE_ROWS)
                        .eq(block.instructions.iter().take(SHARED_SOURCE_ROWS));
            } else {
                summary.shared_source = false;
            }
            match (first_frame, active_frame(report, index, state)) {
                (Some(first), Some(frame)) => {
                    summary.caller_varies |= first.caller != frame.caller;
                    summary.address_varies |= first.address_value != frame.address_value;
                    summary.mode_varies |= first.kind != frame.kind;
                    summary.static_varies |= first.is_static != frame.is_static;
                }
                _ => summary.missing_frames += 1,
            }
        }
        summary
    }
    fn coverage(&self) -> BlockCoverage {
        if self.stale > 0 || (self.current > 0 && self.unexecuted > 0) {
            BlockCoverage::Stale
        } else if self.unexecuted > 0 {
            BlockCoverage::Unexecuted
        } else {
            BlockCoverage::Current
        }
    }
    fn coverage_text(&self) -> String {
        format!(
            "Current {} · Stale {} · Unexecuted {}",
            self.current, self.stale, self.unexecuted
        )
    }
    fn differences(&self) -> Vec<&'static str> {
        [
            (self.storage_varies, "storage"),
            (self.depth_varies, "depth"),
            (self.code_varies, "code account"),
            (self.caller_varies, "caller"),
            (self.address_varies, "ADDRESS"),
            (self.mode_varies, "frame mode"),
            (self.static_varies, "static"),
        ]
        .into_iter()
        .filter_map(|(varies, label)| varies.then_some(label))
        .collect()
    }
    fn roles(&self, first: Option<&CfgBlock>) -> String {
        let differences = self.differences();
        let mut text = if differences.is_empty() {
            first.map_or("Frame roles unavailable".into(), |first| {
                format!(
                    "Storage {} · depth {}",
                    short_address(&first.storage_address),
                    first.frame_depth
                )
            })
        } else {
            format!("Varies: {}", differences.join(" · "))
        };
        if self.missing_frames > 0 {
            text.push_str(" · frame details unavailable");
        }
        text
    }
}
fn active_frame<'a>(
    report: &'a AnalysisReport,
    index: &ReportIndex,
    state: usize,
) -> Option<&'a FrameSnapshot> {
    index.details(report, state)?.entry.frames.last()
}
fn title(node: &DisplayNode) -> String {
    format!(
        "{}:{} · {} states",
        node.program
            .map_or("No program".into(), |program| Symbol::Program(program)
                .to_string()),
        Symbol::Block(node.basic_block),
        node.members.len()
    )
}
fn short_address(address: &str) -> String {
    if address.is_ascii() && address.len() > 18 {
        format!("{}…{}", &address[..8], &address[address.len() - 6..])
    } else {
        abbreviate(address, 18)
    }
}

pub(super) fn preview(
    report: &AnalysisReport,
    index: &ReportIndex,
    node: &DisplayNode,
    view: NodeView,
) -> NodeText {
    let summary = Summary::collect(report, index, node);
    let first = index.cfg(report, node.id);
    let mut lines = vec![(
        summary.coverage_text(),
        if summary.coverage() == BlockCoverage::Current {
            palette::MUTED
        } else {
            palette::WARNING
        },
    )];
    if view == NodeView::Ssa {
        lines.push((
            format!(
                "Per-state SSA: {} phis · {} recorded ops{}",
                summary.phis.text(),
                summary.operations.text(),
                if summary.open_incoming > 0 {
                    format!(" · {} open", summary.open_incoming)
                } else {
                    String::new()
                }
            ),
            palette::MUTED,
        ));
        lines.push(("Select an instance for SSA".into(), palette::BLUE));
        if summary.missing_ssa > 0 {
            lines.push((
                format!("SSA unavailable in {} instances", summary.missing_ssa),
                palette::WARNING,
            ));
        }
    }
    lines.push((
        abbreviate(&summary.roles(first), super::SSA_COLUMNS),
        palette::MUTED,
    ));
    if let Some(block) = first.filter(|_| summary.shared_source) {
        lines.extend(
            block
                .instructions
                .iter()
                .take(SHARED_SOURCE_ROWS)
                .map(|instruction| {
                    (
                        leaf::source_instruction(instruction),
                        if instruction.valid {
                            palette::MUTED
                        } else {
                            palette::ERROR
                        },
                    )
                }),
        );
        if block.instructions.len() > SHARED_SOURCE_ROWS {
            lines.push((
                format!(
                    "+{} source instructions",
                    block.instructions.len() - SHARED_SOURCE_ROWS
                ),
                palette::MUTED,
            ));
        }
    } else {
        lines.push((
            "Source records differ; inspect an instance".into(),
            palette::WARNING,
        ));
    }
    let detail = format!(
        "{} · {} frontiers · {}",
        node.start_pc
            .map_or("no bytecode".into(), |pc| format!("0x{pc:04x}")),
        summary.frontiers,
        if summary.shared_source {
            "shared bytecode"
        } else {
            "source differs"
        }
    );
    NodeText::new(
        title(node),
        detail,
        lines,
        summary.coverage(),
        summary.frontiers > 0,
    )
}

pub(super) fn tooltip(
    report: &AnalysisReport,
    index: &ReportIndex,
    node: &DisplayNode,
    view: NodeView,
) -> String {
    // Group-level counts were measured once for the card. Hover only formats
    // this fixed-size member sample, even for a block with thousands of states.
    let mut lines=vec![title(node),"This card groups source locations, not machine payloads or SSA definitions. Select an instance for its exact SSA, stacks and effects.".into()];
    if view == NodeView::Ssa {
        lines.push("SSA names and effects remain independent in every instance.".into());
    }
    lines.push("Native instances:".into());
    for &state in node.members.iter().take(MEMBERS_IN_TOOLTIP) {
        let coverage = index
            .ssa(report, state)
            .map_or(BlockCoverage::Unexecuted, |block| block.coverage);
        if let Some(block) = index.cfg(report, state) {
            lines.push(format!(
                "{notation_0} · {coverage:?} · depth {} · storage {} · {} history items",
                block.frame_depth,
                block.storage_address,
                block.context.len(),
                notation_0 = Symbol::State(state)
            ));
        } else {
            lines.push(format!(
                "{notation_0} · unavailable CFG record",
                notation_0 = Symbol::State(state)
            ));
        }
    }
    if node.members.len() > MEMBERS_IN_TOOLTIP {
        lines.push(format!(
            "… {} more states. Open Instances to select any native state.",
            node.members.len() - MEMBERS_IN_TOOLTIP
        ));
    }
    lines.join("\n")
}

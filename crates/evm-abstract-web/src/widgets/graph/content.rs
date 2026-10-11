//! Bounded CFG card previews. Native instance details are formatted only when
//! hovered; folding source blocks never merges SSA names or effect histories.

use evm_abstract_notation::Symbol;
mod aggregate;
mod leaf;

use egui::{Color32, FontId, Painter, Vec2};
use evm_abstract_protocol::{AnalysisReport, BlockCoverage};

use super::{HEADER_HEIGHT, LINE_HEIGHT, PAD, index::ReportIndex, projection::DisplayNode};
use crate::{app::Selection, palette, ssa_display::SsaDisplay};

const DISASM_ROWS: usize = 4;
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
    /// Bounded identity rows for semantic zoom. Folded cards name their source
    /// group and native members rather than inventing a single group state.
    pub compact: Vec<String>,
    pub detail: String,
    pub lines: Vec<(String, Color32)>,
    pub coverage: BlockCoverage,
    pub frontier: bool,
    pub size: Vec2,
    // Compatibility for leaf-content regressions; production never stores a
    // potentially large tooltip on every card.
    #[cfg(test)]
    pub tooltip: String,
}
impl NodeText {
    fn new(
        title: String,
        detail: String,
        lines: Vec<(String, Color32)>,
        coverage: BlockCoverage,
        frontier: bool,
    ) -> Self {
        Self {
            compact: vec![title.clone()],
            title,
            detail,
            lines,
            coverage,
            frontier,
            size: Vec2::ZERO,
            #[cfg(test)]
            tooltip: String::new(),
        }
    }
    fn with_identity(mut self, rows: Vec<String>) -> Self {
        self.compact = rows;
        self
    }
    fn measured(mut self, painter: &Painter) -> Self {
        let mut width = measured_width(painter, &self.title, 12.0).max(measured_width(
            painter,
            &self.detail,
            10.0,
        ));
        for (line, _) in &self.lines {
            width = width.max(measured_width(painter, line, 11.0));
        }
        self.size = Vec2::new(
            (width + PAD * 2.0).max(104.0),
            HEADER_HEIGHT + self.lines.len() as f32 * LINE_HEIGHT + PAD,
        );
        self
    }

    /// A selected row can come from outside the bounded preview. Keep its
    /// native instruction visible within the geometry reserved by both views;
    /// its complete text remains available in the native-state tooltip.
    fn within(mut self, painter: &Painter, size: Vec2) -> Self {
        let width = size.x - PAD * 2.0;
        for (line, _) in &mut self.lines {
            if measured_width(painter, line, 11.0) <= width {
                continue;
            }
            let mut lower = 0;
            let mut upper = line.chars().count();
            while lower < upper {
                let columns = lower + (upper - lower).div_ceil(2);
                if measured_width(painter, &abbreviate(line, columns), 11.0) <= width {
                    lower = columns;
                } else {
                    upper = columns - 1;
                }
            }
            *line = abbreviate(line, lower);
        }
        self.size = size;
        self
    }
}

#[cfg(test)]
pub(super) fn node_preview(
    painter: &Painter,
    report: &AnalysisReport,
    index: &ReportIndex,
    node: &DisplayNode,
    view: NodeView,
) -> NodeText {
    node_preview_with_display(
        painter,
        report,
        index,
        node,
        view,
        SsaDisplay::default(),
        Selection::default(),
    )
}

pub(super) fn node_preview_with_display(
    painter: &Painter,
    report: &AnalysisReport,
    index: &ReportIndex,
    node: &DisplayNode,
    view: NodeView,
    display: SsaDisplay,
    selection: Selection,
) -> NodeText {
    let text = if node.members.len() > 1 {
        aggregate::preview(report, index, node, view)
    } else if let Some(block) = index.cfg(report, node.id) {
        let selected_pc = (selection.state == Some(block.id))
            .then_some(selection.pc)
            .flatten();
        if view == NodeView::Ssa {
            let compact = leaf::preview(report, index, block, view, SsaDisplay::Compact, None)
                .measured(painter);
            let full =
                leaf::preview(report, index, block, view, SsaDisplay::Full, None).measured(painter);
            let size = compact.size.max(full.size);
            let text = if selected_pc.is_some() {
                leaf::preview(report, index, block, view, display, selected_pc)
            } else {
                match display {
                    SsaDisplay::Compact => compact,
                    SsaDisplay::Full => full,
                }
            };
            return text.within(painter, size);
        }
        leaf::preview(report, index, block, view, display, selected_pc)
    } else {
        NodeText::new(
            Symbol::State(node.id).to_string(),
            "Unavailable state".into(),
            vec![],
            BlockCoverage::Unexecuted,
            false,
        )
    };
    text.measured(painter)
}
pub(super) fn node_tooltip(
    report: &AnalysisReport,
    index: &ReportIndex,
    node: &DisplayNode,
    view: NodeView,
) -> String {
    if node.members.len() > 1 {
        aggregate::tooltip(report, index, node, view)
    } else if let Some(block) = index.cfg(report, node.id) {
        leaf::tooltip(report, index, block, view)
    } else {
        format!(
            "Native state {} is unavailable in this report.",
            Symbol::State(node.id)
        )
    }
}

/// Existing leaf tests exercise full details through this convenience wrapper.
/// Production builds its ReportIndex once and calls preview/tooltip separately.
#[cfg(test)]
pub(super) fn node_text(
    painter: &Painter,
    report: &AnalysisReport,
    block: &evm_abstract_protocol::CfgBlock,
    view: NodeView,
) -> NodeText {
    let index = ReportIndex::new(report);
    let mut text =
        leaf::preview(report, &index, block, view, SsaDisplay::default(), None).measured(painter);
    text.tooltip = leaf::tooltip(report, &index, block, view);
    text
}

fn abbreviate(text: &str, columns: usize) -> String {
    if columns == 0 {
        return String::new();
    }
    let mut chars = text.chars();
    let mut prefix: String = chars.by_ref().take(columns).collect();
    if chars.next().is_some() {
        prefix.pop();
        prefix.push('…');
    }
    prefix
}
fn measured_width(painter: &Painter, text: &str, size: f32) -> f32 {
    painter
        .layout_job(crate::notation::job(
            text,
            FontId::monospace(size),
            palette::TEXT,
        ))
        .size()
        .x
        .ceil()
}

#[cfg(test)]
mod overview_tests;
#[cfg(test)]
mod tests;

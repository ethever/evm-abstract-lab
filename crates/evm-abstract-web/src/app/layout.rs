//! Responsive panes size code from its content and give the remaining width to
//! the graph. Long automatic code panes stop at 28% each so the graph stays
//! readable; short panes retain their natural width. Manual divider choices and
//! active tabs persist independently for each width class.

use egui::{Stroke, Ui};
use egui_tiles::{
    Behavior, Container, Linear, LinearDir, ResizeState, Shares, Tile, TileId, Tiles, Tree,
    UiResponse,
};
use evm_abstract_protocol::AnalysisReport;

use super::Selection;
use crate::{palette, widgets};

const PANE_MIN: f32 = 180.0;
const GRAPH_AUTO_MIN: f32 = 320.0;
const GAP: f32 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pane {
    Disassembly,
    Graph,
    Ssa,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum WidthClass {
    Wide,
    Medium,
    Narrow,
}

impl WidthClass {
    pub(super) fn for_width(width: f32) -> Self {
        if width >= 1120.0 {
            Self::Wide
        } else if width >= 720.0 {
            Self::Medium
        } else {
            Self::Narrow
        }
    }
}

pub(super) struct PaneLayout {
    wide: Tree<Pane>,
    medium: Tree<Pane>,
    narrow: Tree<Pane>,
    // Logical widths, rather than percentages, survive window resizing. If a
    // smaller viewport constrains them, keep the preference for later growth.
    wide_manual: Option<[f32; 2]>,
    medium_manual: Option<f32>,
}

impl Default for PaneLayout {
    fn default() -> Self {
        let mut tiles = Tiles::default();
        let disassembly = tiles.insert_pane(Pane::Disassembly);
        let graph = tiles.insert_pane(Pane::Graph);
        let ssa = tiles.insert_pane(Pane::Ssa);
        let columns = Linear::new(LinearDir::Horizontal, vec![disassembly, graph, ssa]);
        let root = tiles.insert_container(columns);
        let wide = Tree::new("workspace_wide", root, tiles);

        let mut tiles = Tiles::default();
        let disassembly = tiles.insert_pane(Pane::Disassembly);
        let ssa = tiles.insert_pane(Pane::Ssa);
        let code = tiles.insert_tab_tile(vec![disassembly, ssa]);
        let graph = tiles.insert_pane(Pane::Graph);
        let root = tiles.insert_container(Linear::new(LinearDir::Horizontal, vec![code, graph]));
        let medium = Tree::new("workspace_medium", root, tiles);
        let narrow = Tree::new_tabs(
            "workspace_narrow",
            vec![Pane::Disassembly, Pane::Graph, Pane::Ssa],
        );
        Self {
            wide,
            medium,
            narrow,
            wide_manual: None,
            medium_manual: None,
        }
    }
}

impl PaneLayout {
    pub(super) fn show(&mut self, ui: &mut Ui, panes: &mut AnalysisPanes<'_>) {
        let width = ui.available_width();
        match WidthClass::for_width(width) {
            WidthClass::Wide => {
                let available = width - 2.0 * GAP;
                let desired = self.wide_manual.unwrap_or_else(|| {
                    [
                        widgets::disassembly_width(
                            ui,
                            panes.report,
                            panes.program,
                            *panes.selection,
                        ),
                        widgets::ssa_width_cached(
                            ui,
                            panes.report,
                            panes.program,
                            *panes.selection,
                            panes.ssa_cache,
                        ),
                    ]
                    .map(|natural| natural.min(available * 0.28))
                });
                let graph_min = if self.wide_manual.is_some() {
                    PANE_MIN
                } else {
                    GRAPH_AUTO_MIN.max(available * 0.40)
                };
                let [left, right] = bounded_sides(desired, available - graph_min);
                let before = set_widths(&mut self.wide, &[left, available - left - right, right]);
                crate::framework::pane_tree(ui, &mut self.wide, panes);
                let columns = columns(&self.wide);
                if columns.shares != before {
                    let widths = columns.shares.split(&columns.children, available);
                    self.wide_manual = Some([widths[0], widths[2]]);
                }
            }
            WidthClass::Medium => {
                let desired =
                    self.medium_manual
                        .unwrap_or_else(|| match active_code_tab(&self.medium) {
                            Pane::Ssa => widgets::ssa_width_cached(
                                ui,
                                panes.report,
                                panes.program,
                                *panes.selection,
                                panes.ssa_cache,
                            ),
                            _ => widgets::disassembly_width(
                                ui,
                                panes.report,
                                panes.program,
                                *panes.selection,
                            ),
                        });
                let available = width - GAP;
                let graph_min = if self.medium_manual.is_some() {
                    PANE_MIN
                } else {
                    GRAPH_AUTO_MIN
                };
                let code = desired.clamp(PANE_MIN, available - graph_min);
                let before = set_widths(&mut self.medium, &[code, available - code]);
                crate::framework::pane_tree(ui, &mut self.medium, panes);
                let columns = columns(&self.medium);
                if columns.shares != before {
                    self.medium_manual =
                        Some(columns.shares.split(&columns.children, available)[0]);
                }
            }
            WidthClass::Narrow => crate::framework::pane_tree(ui, &mut self.narrow, panes),
        }
    }
}

fn columns(tree: &Tree<Pane>) -> &Linear {
    match tree
        .tiles
        .get_container(tree.root.expect("responsive tree has a root"))
    {
        Some(Container::Linear(columns)) => columns,
        _ => unreachable!("wide and medium trees have horizontal roots"),
    }
}

fn set_widths(tree: &mut Tree<Pane>, widths: &[f32]) -> Shares {
    let Some(Tile::Container(Container::Linear(columns))) = tree
        .tiles
        .get_mut(tree.root.expect("responsive tree has a root"))
    else {
        unreachable!("wide and medium trees have horizontal roots");
    };
    for (child, width) in columns.children.iter().zip(widths) {
        columns.shares[*child] = *width;
    }
    // Only divider interactions mutate these shares during Tree::ui. Comparing
    // them afterwards distinguishes a user's resize from automatic measurement.
    columns.shares.clone()
}

fn active_code_tab(tree: &Tree<Pane>) -> Pane {
    let code = columns(tree).children[0];
    let Some(Container::Tabs(tabs)) = tree.tiles.get_container(code) else {
        return Pane::Disassembly;
    };
    tabs.active
        .or_else(|| tabs.children.first().copied())
        .and_then(|id| tree.tiles.get_pane(&id).copied())
        .unwrap_or(Pane::Disassembly)
}

/// Share the constrained code budget fairly, preserving a short pane's natural
/// width before limiting the other. Overflow remains in each table's scroller.
fn bounded_sides(desired: [f32; 2], budget: f32) -> [f32; 2] {
    let [left, right] = desired.map(|width| width.max(PANE_MIN));
    if left + right <= budget {
        [left, right]
    } else {
        let half = budget * 0.5;
        if left <= half {
            [left, budget - left]
        } else if right <= half {
            [budget - right, right]
        } else {
            [half, half]
        }
    }
}

/// Borrows the same selection and graph across all responsive arrangements.
/// Hidden tabs keep their independent source scroll and graph navigation state.
pub(super) struct AnalysisPanes<'a> {
    pub(super) report: &'a AnalysisReport,
    pub(super) program: Option<usize>,
    pub(super) selection: &'a mut Selection,
    pub(super) graph: &'a mut widgets::Graph,
    pub(super) disasm_focus: &'a mut Selection,
    pub(super) ssa_focus: &'a mut Selection,
    pub(super) ssa_cache: &'a mut widgets::SsaCache,
}

impl AnalysisPanes<'_> {
    pub(super) fn show(&mut self, ui: &mut Ui, pane: Pane) {
        match pane {
            Pane::Disassembly => {
                widgets::disassembly(
                    ui,
                    self.report,
                    self.program,
                    self.selection,
                    self.disasm_focus,
                );
            }
            Pane::Graph => self.graph.show(ui, self.report, self.selection),
            Pane::Ssa => widgets::ssa_cached(
                ui,
                self.report,
                self.program,
                self.selection,
                self.ssa_focus,
                self.ssa_cache,
            ),
        }
    }
}

impl Behavior<Pane> for AnalysisPanes<'_> {
    fn pane_ui(&mut self, ui: &mut Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        self.show(ui, *pane);
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        match pane {
            Pane::Disassembly => "Disassembly",
            Pane::Graph => "Control flow",
            Pane::Ssa => "SSA",
        }
        .into()
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        GAP
    }

    fn resize_stroke(&self, _style: &egui::Style, state: ResizeState) -> Stroke {
        // The default idle stroke uses extreme_bg_color, which our theme also
        // uses for pane backgrounds. Keep the visual line independent of it.
        match state {
            ResizeState::Idle => Stroke::new(1.0, palette::MUTED),
            ResizeState::Hovering => Stroke::new(2.0, palette::BLUE),
            ResizeState::Dragging => Stroke::new(2.0, palette::ACCENT),
        }
    }

    fn min_size(&self) -> f32 {
        PANE_MIN
    }

    fn simplification_options(&self) -> egui_tiles::SimplificationOptions {
        egui_tiles::SimplificationOptions {
            all_panes_must_have_tabs: false,
            ..egui_tiles::SimplificationOptions::default()
        }
    }

    fn is_tile_draggable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        // The responsive arrangements are intentional; only dividers and tabs
        // are user-adjustable, avoiding inaccessible dropped panes on resize.
        false
    }
}

#[cfg(test)]
mod sizing_tests;
#[cfg(test)]
mod tests;

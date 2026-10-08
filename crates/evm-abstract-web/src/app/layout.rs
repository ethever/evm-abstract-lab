//! Responsive pane arrangements. Each width class retains its own divider
//! shares and active tab, so resizing the browser never resets manual choices.

use egui::{Stroke, Ui};
use egui_tiles::{Behavior, Linear, LinearDir, ResizeState, TileId, Tiles, Tree, UiResponse};
use evm_abstract_protocol::AnalysisReport;

use super::Selection;
use crate::{palette, widgets};

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
}

impl Default for PaneLayout {
    fn default() -> Self {
        let mut tiles = Tiles::default();
        let disassembly = tiles.insert_pane(Pane::Disassembly);
        let graph = tiles.insert_pane(Pane::Graph);
        let ssa = tiles.insert_pane(Pane::Ssa);
        let mut columns = Linear::new(LinearDir::Horizontal, vec![disassembly, graph, ssa]);
        for (tile, share) in [(disassembly, 0.27), (graph, 0.40), (ssa, 0.33)] {
            columns.shares[tile] = share;
        }
        let root = tiles.insert_container(columns);
        let wide = Tree::new("workspace_wide", root, tiles);

        let mut tiles = Tiles::default();
        let disassembly = tiles.insert_pane(Pane::Disassembly);
        let ssa = tiles.insert_pane(Pane::Ssa);
        let code = tiles.insert_tab_tile(vec![disassembly, ssa]);
        let graph = tiles.insert_pane(Pane::Graph);
        let root = tiles.insert_container(Linear::new_binary(
            LinearDir::Horizontal,
            [code, graph],
            0.48,
        ));
        let medium = Tree::new("workspace_medium", root, tiles);
        let narrow = Tree::new_tabs(
            "workspace_narrow",
            vec![Pane::Disassembly, Pane::Graph, Pane::Ssa],
        );
        Self {
            wide,
            medium,
            narrow,
        }
    }
}

impl PaneLayout {
    pub(super) fn show(&mut self, ui: &mut Ui, panes: &mut AnalysisPanes<'_>) {
        let tree = match WidthClass::for_width(ui.available_width()) {
            WidthClass::Wide => &mut self.wide,
            WidthClass::Medium => &mut self.medium,
            WidthClass::Narrow => &mut self.narrow,
        };
        crate::framework::pane_tree(ui, tree, panes);
    }
}

/// Borrows the same selection and graph across all responsive arrangements.
/// Hidden tabs keep their independent source scroll and graph navigation state.
pub(super) struct AnalysisPanes<'a> {
    pub(super) report: &'a AnalysisReport,
    pub(super) selection: &'a mut Selection,
    pub(super) graph: &'a mut widgets::Graph,
    pub(super) disasm_focus: &'a mut Selection,
    pub(super) ssa_focus: &'a mut Selection,
}

impl AnalysisPanes<'_> {
    pub(super) fn show(&mut self, ui: &mut Ui, pane: Pane) {
        match pane {
            Pane::Disassembly => {
                widgets::disassembly(ui, self.report, self.selection, self.disasm_focus);
            }
            Pane::Graph => self.graph.show(ui, self.report, self.selection),
            Pane::Ssa => widgets::ssa(ui, self.report, self.selection, self.ssa_focus),
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
        5.0
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
        180.0
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
mod tests;

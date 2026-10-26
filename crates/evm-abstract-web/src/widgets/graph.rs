//! CFG views over an immutable native report. Source blocks fold only for
//! display; selecting an instance always uses its original state and SSA IDs.

mod content;
mod controls;
mod edges;
mod groups;
mod index;
mod layout;
mod minimum;
pub(crate) use minimum::control::{ControlOrigin, GraphIntent, GraphScope, ProjectionTarget};
mod navigation;
mod paint;
mod projection;
mod render;
mod scene;
mod source;
mod termination;
#[cfg(test)]
mod tests;
mod visibility;

use crate::app::model::ReportViewId;
use crate::report::ReportView;
use crate::{app::Selection, palette, ssa_display::SsaDisplay};
#[cfg(test)]
use content::node_text;
use content::{NodeText, NodeView};
use edges::edge_label;
use egui::{Painter, Pos2, Rect, Vec2};
use index::ReportIndex;
use layout::Placement;
use projection::DisplayGraph;
pub(crate) use projection::GraphMode;
use std::collections::{BTreeMap, BTreeSet};
use visibility::Visibility;

/// Renderer events carry original Native coordinates; the shell validates them
/// against its Presentation before updating canonical focus/history.
#[derive(Clone, Copy)]
pub(crate) enum NativeIntent {
    Select(Selection),
    Open(Selection),
}

const PAD: f32 = 6.0;
const HEADER_HEIGHT: f32 = 33.0;
const LINE_HEIGHT: f32 = 14.0;
/// Preferences and camera survive replacement of the report-derived scene.
pub(crate) struct Graph {
    pub(crate) zoom: f32,
    content: NodeView,
    ssa_display: SsaDisplay,
    mode: GraphMode,
    whole_scope: bool,
    neighborhood_hops: usize,
    pan: Vec2,
    fitted: bool,
    fit_pending: bool,
    entry_pending: bool,
    camera_restored: bool,
    focus_pending: bool,
    cancel_wheel: bool,
    viewport: Option<Vec2>,
    scene: Scene,
    minimum_layouts: Vec<minimum::LayoutMemory>,
}

struct Scene {
    identity: ReportViewId,
    minimum: Option<minimum::Scene>,
    ssa_selection: Selection,
    preview_dirty: BTreeSet<usize>,
    program_scope: Option<usize>,
    scope_initialized: bool,
    native_context: Option<usize>,
    local_center: Option<usize>,
    index: Option<ReportIndex>,
    display: DisplayGraph,
    ending_summary: termination::TerminationSummary,
    display_dirty: bool,
    shown_states: usize,
    native_edges: usize,
    deferred_edges: usize,
    node_positions: BTreeMap<usize, usize>,
    edge_positions: BTreeMap<usize, usize>,
    edge_labels: BTreeMap<usize, String>,
    native_edge_to_display: BTreeMap<usize, usize>,
    highlighted_edges: BTreeSet<usize>,
    highlighted_state: Option<usize>,
    highlight_dirty: bool,
    node_visibility: Visibility,
    edge_visibility: Visibility,
    instance_group: Option<usize>,
    instance_filter: String,
    content_anchor: Option<(usize, Vec2)>,
    nodes: BTreeMap<usize, NodeText>,
    placement: Placement,
    layout_error: Option<evm_abstract_layout::LayoutError>,
    detail_level: paint::DetailLevel,
    layout_pending: bool,
}

impl Default for Scene {
    fn default() -> Self {
        Self {
            identity: ReportViewId::default(),
            minimum: None,
            ssa_selection: Selection::default(),
            preview_dirty: BTreeSet::new(),
            program_scope: None,
            scope_initialized: false,
            native_context: None,
            local_center: None,
            index: None,
            display: DisplayGraph::default(),
            ending_summary: termination::TerminationSummary::default(),
            display_dirty: true,
            shown_states: 0,
            native_edges: 0,
            deferred_edges: 0,
            node_positions: BTreeMap::new(),
            edge_positions: BTreeMap::new(),
            edge_labels: BTreeMap::new(),
            native_edge_to_display: BTreeMap::new(),
            highlighted_edges: BTreeSet::new(),
            highlighted_state: None,
            highlight_dirty: true,
            node_visibility: Visibility::default(),
            edge_visibility: Visibility::default(),
            instance_group: None,
            instance_filter: String::new(),
            content_anchor: None,
            nodes: BTreeMap::new(),
            placement: Placement::default(),
            layout_error: None,
            detail_level: paint::DetailLevel::Preview,
            layout_pending: true,
        }
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            zoom: 1.0,
            content: NodeView::Disassembly,
            ssa_display: SsaDisplay::default(),
            mode: GraphMode::default(),
            whole_scope: false,
            neighborhood_hops: 2,
            pan: Vec2::ZERO,
            fitted: false,
            fit_pending: false,
            entry_pending: true,
            camera_restored: false,
            focus_pending: false,
            cancel_wheel: false,
            viewport: None,
            scene: Scene::default(),
            minimum_layouts: Vec::new(),
        }
    }
}

impl Graph {
    pub(crate) fn camera(&self) -> (f32, Vec2) {
        (self.zoom, self.pan)
    }

    /// A history location owns its camera. Restore only validated screen-space
    /// coordinates; report activation and scope selection must happen first.
    pub(crate) fn restore_camera(&mut self, zoom: f32, pan: Vec2) -> bool {
        if !zoom.is_finite() || zoom <= 0.0 || !pan.is_finite() {
            return false;
        }
        self.zoom = zoom;
        self.pan = pan;
        self.fitted = false;
        self.fit_pending = false;
        self.entry_pending = false;
        self.focus_pending = false;
        self.cancel_wheel = true;
        self.camera_restored = true;
        true
    }

    pub(crate) fn set_program_scope(&mut self, program: Option<usize>) {
        if self.scene.program_scope != program {
            self.scene.program_scope = program;
            self.scene.scope_initialized = true;
            self.scene.native_context = None;
            self.invalidate_scene();
        }
    }

    pub(crate) fn focus_selection(&mut self) {
        self.focus_pending = true;
    }

    /// The visible scene has actually been measured and laid out. Hidden graph
    /// tabs are never built merely to finish the result's timing measurement.
    pub(crate) fn prepared(&self) -> bool {
        (self.scene.minimum.is_some() || (self.scene.index.is_some() && !self.scene.layout_pending))
            && self.viewport.is_some()
    }

    /// Bind an immutable graph namespace. Refreshes replace only derived data;
    /// unrelated reports additionally request their initial fit.
    pub(crate) fn activate(&mut self, identity: ReportViewId) {
        if self.scene.identity == identity {
            return;
        }
        self.minimum_layouts
            .retain(|layout| layout.same_report(identity));
        let initial_fit = self.fit_pending
            && self.scene.index.is_none()
            && self.scene.minimum.is_none()
            && self.viewport.is_none();
        let replacement = !self.scene.identity.same_report(identity);
        let same_model = self.scene.identity.namespace() == identity.namespace()
            && self.scene.identity.is_minimum() == identity.is_minimum();
        let program_scope = (!replacement && same_model)
            .then_some(self.scene.program_scope)
            .flatten();
        let scope_initialized = !replacement && same_model && self.scene.scope_initialized;
        let native_context = (!replacement && same_model)
            .then_some(self.scene.native_context)
            .flatten();
        self.focus_pending = false;
        self.scene = Scene {
            identity,
            program_scope,
            scope_initialized,
            native_context,
            ..Scene::default()
        };
        if replacement {
            self.camera_restored = false;
            self.zoom = 1.0;
            self.pan = Vec2::ZERO;
            self.viewport = None;
            self.entry_pending = !initial_fit;
            self.fit_pending = initial_fit;
            self.fitted = initial_fit;
        }
    }

    pub(crate) fn content_label(&self) -> &'static str {
        match self.content {
            NodeView::Disassembly => "Disasm",
            NodeView::Ssa => "SSA",
        }
    }

    /// SSA detail choices change cached text only. Both bounded presentations
    /// reserve the same natural geometry, so native routes and camera stay put.
    pub(crate) fn set_ssa_display(&mut self, display: SsaDisplay, selection: Selection) {
        if self.ssa_display != display {
            self.ssa_display = display;
            if self.content == NodeView::Ssa {
                self.scene.nodes.clear();
                self.scene.preview_dirty.clear();
            }
        }
        if !self.scene.ssa_selection.same_native(selection)
            && self.ssa_display == SsaDisplay::Compact
            && self.content == NodeView::Ssa
        {
            for selection in [self.scene.ssa_selection, selection] {
                if let (Some(state), Some(_)) = (selection.state, selection.pc)
                    && let Some(id) = self.scene.display.state_to_node.get(&state)
                    && self.scene.node_positions.get(id).is_some_and(|position| {
                        self.scene.display.nodes[*position].members.as_slice() == [state]
                    })
                {
                    self.scene.preview_dirty.insert(*id);
                }
            }
        }
        self.scene.ssa_selection = selection;
    }

    pub(crate) fn set_mode(&mut self, mode: GraphMode) {
        if self.mode != mode {
            self.mode = mode;
            self.invalidate_scene();
        }
    }

    fn invalidate_scene(&mut self) {
        self.scene.display_dirty = true;
        self.scene.nodes.clear();
        self.scene.preview_dirty.clear();
        self.scene.content_anchor = None;
        self.scene.instance_group = None;
        self.scene.layout_error = None;
        self.scene.layout_pending = true;
        // Same-source context changes are discovered during scene preparation,
        // after the shell has already restored its validated history camera.
        if !self.camera_restored {
            self.fit_pending = false;
            self.entry_pending = true;
            self.fitted = false;
        }
    }

    fn set_content(&mut self, content: NodeView, selected: Option<usize>) {
        if content == self.content {
            return;
        }
        self.scene.content_anchor = self.viewport.and_then(|viewport| {
            let screen = Rect::from_min_size(Pos2::ZERO, viewport);
            let selected = selected
                .and_then(|state| self.scene.display.state_to_node.get(&state))
                .and_then(|id| self.scene.placement.nodes.get(id).map(|rect| (*id, rect)))
                .filter(|(_, rect)| {
                    screen.contains(Pos2::ZERO + self.pan + rect.center().to_vec2() * self.zoom)
                });
            let center = (viewport * 0.5 - self.pan) / self.zoom;
            selected
                .or_else(|| {
                    self.scene
                        .placement
                        .nodes
                        .iter()
                        .min_by(|(_, a), (_, b)| {
                            (a.center().to_vec2() - center)
                                .length_sq()
                                .total_cmp(&(b.center().to_vec2() - center).length_sq())
                        })
                        .map(|(id, rect)| (*id, rect))
                })
                .map(|(id, rect)| (id, self.pan + rect.center().to_vec2() * self.zoom))
        });
        self.content = content;
        self.scene.nodes.clear();
        self.scene.preview_dirty.clear();
        self.scene.layout_pending = true;
        self.fitted = false;
    }

    fn queue_fit(&mut self) {
        self.fitted = true;
        self.fit_pending = true;
        self.entry_pending = false;
        self.focus_pending = false;
    }

    fn update_viewport(&mut self, report: &ReportView<'_>, size: Vec2, painter: &Painter) -> bool {
        if !size.is_finite() || size.min_elem() <= 1.0 {
            return false;
        }
        self.viewport = Some(size);
        if self.scene.layout_pending {
            let sizes = self
                .scene
                .nodes
                .iter()
                .map(|(id, node)| (*id, node.size))
                .collect();
            let input = self.scene.index.as_ref().map_or_else(
                evm_abstract_layout::Input::default,
                |index| {
                    layout::prepare(
                        report,
                        index,
                        &self.scene.display,
                        &sizes,
                        &self.scene.edge_labels,
                        painter,
                    )
                },
            );
            match layout::scene(&input) {
                Ok(placement) => {
                    self.scene.placement = placement;
                    self.scene.layout_error = None;
                }
                Err(error) => {
                    self.scene.placement = Placement::default();
                    self.scene.layout_error = Some(error);
                    self.fit_pending = false;
                }
            }
            self.rebuild_visibility();
            self.scene.layout_pending = false;
            if let Some((id, position)) = self.scene.content_anchor.take()
                && let Some(rect) = self.scene.placement.nodes.get(&id)
            {
                self.pan = position - rect.center().to_vec2() * self.zoom;
            }
        }
        if self.fit_pending {
            self.fit(size);
            self.fit_pending = false;
            self.fitted = true;
        }
        true
    }

    pub(crate) fn zoom_at(&mut self, pointer: Vec2, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 || (factor - 1.0).abs() <= f32::EPSILON {
            return;
        }
        let old = self.zoom;
        // Fit can legitimately make a large graph smaller than 8%. An
        // arbitrary lower clamp would turn its next zoom-out into zoom-in.
        let zoom = (old * factor).min(2.5);
        let ratio = zoom / old;
        if zoom <= 0.0 || !ratio.is_finite() || ratio <= 0.0 {
            return;
        }
        let pan = pointer - (pointer - self.pan) * ratio;
        if !pan.is_finite() {
            return;
        }
        self.fitted = false;
        self.zoom = zoom;
        self.pan = pan;
    }

    fn screen_rect(&self, canvas: Rect, world: Rect) -> Rect {
        Rect::from_min_size(
            canvas.min + self.pan + world.min.to_vec2() * self.zoom,
            world.size() * self.zoom,
        )
    }

    fn fit(&mut self, size: Vec2) {
        self.fit_bounds(size, self.scene.placement.bounds);
    }

    fn fit_bounds(&mut self, size: Vec2, bounds: Rect) {
        if bounds.is_finite() {
            self.zoom = layout::fit_scale(bounds.size(), size).min(1.0);
            self.pan = size * 0.5 - bounds.center().to_vec2() * self.zoom;
        }
    }

    /// Fit a small scope when it stays readable, otherwise start at its entry.
    /// Whole-graph Fit remains explicit and may intentionally hide text at LOD.
    fn readable_entry(&mut self, size: Vec2, bounds: Rect, anchor: Option<Rect>) {
        let Some(anchor) = anchor.filter(|rect| rect.is_finite()) else {
            return;
        };
        const READABLE_SCALE: f32 = 0.9;
        self.fitted = false;
        if bounds.is_finite() && layout::fit_scale(bounds.size(), size) >= READABLE_SCALE {
            self.fit_bounds(size, bounds);
            return;
        }
        self.zoom = ((size.x - 24.0).max(1.0) / anchor.width()).clamp(READABLE_SCALE, 1.0);
        let width = anchor.width() * self.zoom;
        let x = if width > size.x - 24.0 {
            12.0 - anchor.min.x * self.zoom
        } else {
            size.x * 0.5 - anchor.center().x * self.zoom
        };
        self.restore_camera(self.zoom, Vec2::new(x, 16.0 - anchor.min.y * self.zoom));
    }

    fn open_native_state(
        &mut self,
        report: &ReportView<'_>,
        selection: &mut Selection,
        state: usize,
    ) {
        *selection = Selection {
            state: Some(state),
            pc: None,
            value: None,
        };
        self.scene.program_scope = self
            .scene
            .index
            .as_ref()
            .and_then(|index| index.cfg(report, state))
            .and_then(|state| state.program);
        self.scene.scope_initialized = true;
        self.scene.native_context = Some(state);
        self.invalidate_scene();
    }

    fn grid(&self, painter: &Painter, canvas: Rect) {
        let spacing = 24.0 * self.zoom.max(0.5);
        let offset = Vec2::new(
            self.pan.x.rem_euclid(spacing),
            self.pan.y.rem_euclid(spacing),
        );
        let mut x = canvas.left() + offset.x;
        while x < canvas.right() {
            let mut y = canvas.top() + offset.y;
            while y < canvas.bottom() {
                painter.circle_filled(Pos2::new(x, y), 0.6, palette::BORDER.gamma_multiply(0.4));
                y += spacing;
            }
            x += spacing;
        }
    }
}

/// Lightweight deterministic layout entry used by the native identity regression.
#[cfg(test)]
pub(crate) fn layout(report: &ReportView<'_>) -> BTreeMap<usize, Pos2> {
    let sizes = report
        .cfg
        .iter()
        .map(|block| (block.id, Vec2::new(140.0, 70.0)))
        .collect();
    layout::for_report(report, &sizes)
        .nodes
        .into_iter()
        .map(|(id, rect)| (id, rect.min))
        .collect()
}

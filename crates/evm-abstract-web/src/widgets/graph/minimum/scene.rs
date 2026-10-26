//! Projection is prepared once per immutable scope. First-use geometry waits
//! for the real canvas; saved geometry survives resize and Navigator Back.
use super::{
    GraphScope, Selection,
    control::{Control, GraphWindow, ProjectionError},
    geometry::{self, Density},
    text::CardText,
};
use crate::{app::model::ReportViewId, widgets::graph::Visibility};
use egui::{Painter, Rect, Vec2};
use evm_abstract_protocol::AnalysisReport;

#[derive(Clone, Copy)]
pub(in crate::widgets::graph) struct LayoutMemory {
    identity: ReportViewId,
    scope: GraphScope,
    window: GraphWindow,
    density: Option<Density>,
}
impl LayoutMemory {
    pub(in crate::widgets::graph) fn same_report(&self, identity: ReportViewId) -> bool {
        self.identity.same_report(identity)
    }
}

#[derive(Debug, thiserror::Error)]
pub(super) enum SceneError {
    #[error("{0}")]
    Projection(#[from] ProjectionError),
    #[error("{0}")]
    Layout(#[from] evm_abstract_layout::LayoutError),
}

#[derive(Clone, Copy)]
pub(super) enum Picker {
    Origins(usize),
    Targets(usize),
    CardTargets(usize),
    Incoming(Option<usize>),
}

pub(in crate::widgets::graph) struct Scene {
    pub(super) scope: GraphScope,
    pub(super) window: GraphWindow,
    pub(super) control: Control,
    base_text: Vec<CardText>,
    pub(super) text: Vec<CardText>,
    pub(super) context_caption: String,
    pub(in crate::widgets::graph) placement: super::super::Placement,
    pub(super) core_bounds: Rect,
    pub(super) selected: Option<usize>,
    pub(super) error: Option<SceneError>,
    pub(super) nodes_visible: Visibility,
    pub(super) edges_visible: Visibility,
    pub(super) picker: Option<Picker>,
    pub(in crate::widgets::graph) geometry_pending: bool,
    pub(super) target_sets: Vec<Vec<usize>>,
}

impl Scene {
    fn new(report: &AnalysisReport, scope: GraphScope, window: GraphWindow) -> Self {
        let mut scene = Self {
            scope,
            window,
            control: Control::default(),
            base_text: Vec::new(),
            text: Vec::new(),
            context_caption: String::new(),
            placement: Default::default(),
            core_bounds: Rect::NOTHING,
            selected: None,
            error: None,
            nodes_visible: Default::default(),
            edges_visible: Default::default(),
            picker: None,
            geometry_pending: true,
            target_sets: Vec::new(),
        };
        match Control::scoped_window(report, scope, window) {
            Ok(control) => {
                debug_assert_eq!(control.scope, Some(scope));
                debug_assert_eq!(control.window, window);
                scene.control = control;
            }
            Err(error) => {
                scene.error = Some(error.into());
                return scene;
            }
        }
        scene.base_text = scene
            .control
            .cards
            .iter()
            .map(|card| CardText::new(report, card))
            .collect();
        scene.text = scene.base_text.clone();
        scene.context_caption = super::text::context_caption(report, scope, &scene.control);
        scene.target_sets = scene
            .control
            .cards
            .iter()
            .map(|card| card.target_set.into_iter().collect())
            .collect();
        for frontier in &scene.control.frontiers {
            if frontier.direction == super::control::FrontierDirection::Outgoing
                && !scene.target_sets[frontier.card].contains(&frontier.target_set)
            {
                scene.target_sets[frontier.card].push(frontier.target_set);
            }
        }
        scene
    }

    #[cfg(test)]
    pub(in crate::widgets::graph) fn node_geometry(
        &self,
        canvas: Rect,
        pan: Vec2,
        zoom: f32,
        core: bool,
    ) -> Vec<(String, Rect)> {
        let ids: Vec<_> = if core {
            self.control.core.clone()
        } else {
            (0..self.text.len()).collect()
        };
        ids.into_iter()
            .filter_map(|id| {
                let world = self.placement.nodes.get(&id)?;
                let rect = Rect::from_min_size(
                    canvas.min + pan + world.min.to_vec2() * zoom,
                    world.size() * zoom,
                );
                Some((self.text.get(id)?.title.clone(), rect))
            })
            .collect()
    }

    pub(super) fn contains(&self, selection: &Selection) -> bool {
        self.scope.summary() == selection.summary
            && self
                .control
                .cards
                .iter()
                .flat_map(|card| &card.origins)
                .any(|origin| {
                    origin.target.procedure == selection.procedure
                        && selection.node.is_none_or(|node| {
                            origin.target.origin.node == node
                                && origin.target.origin.location == selection.control
                        })
                })
    }

    fn arrange(
        &mut self,
        painter: &Painter,
        canvas: Vec2,
        cached: Option<Density>,
    ) -> Option<Density> {
        if self.error.is_some() {
            self.geometry_pending = false;
            return None;
        }
        let choices = cached.map_or_else(
            || {
                geometry::candidates(
                    canvas,
                    self.control.core.len().saturating_sub(1),
                    self.window == GraphWindow::Entry,
                )
            },
            |density| vec![density],
        );
        let mut chosen = None;
        'density: for mut density in choices {
            for _ in 0..5 {
                let text = geometry::text(&self.base_text, self.control.anchor, density);
                match geometry::place(&self.control, &text, painter) {
                    Ok(placement) => {
                        let bounds = geometry::core_bounds(&self.control, &placement);
                        self.text = text;
                        self.placement = placement;
                        self.core_bounds = bounds;
                        chosen = Some(density);
                        if cached.is_some() || geometry::fits(bounds, canvas) {
                            break 'density;
                        }
                        if bounds.width() > canvas.x - 16.0 {
                            density = density.fit_width(bounds, canvas);
                        } else {
                            continue 'density;
                        }
                    }
                    Err(error) => {
                        self.error = Some(error.into());
                        break 'density;
                    }
                }
            }
        }
        self.nodes_visible = Visibility::new(&self.placement.nodes);
        let bounds = self
            .placement
            .edges
            .iter()
            .map(|(id, route)| {
                let mut bounds = route.label;
                for point in &route.path {
                    bounds.extend_with(*point);
                }
                (*id, bounds)
            })
            .collect();
        self.edges_visible = Visibility::new(&bounds);
        self.geometry_pending = false;
        chosen
    }
}

impl super::Graph {
    pub(super) fn ensure_minimum_scope(
        &mut self,
        report: &AnalysisReport,
        scope: GraphScope,
        requested: Option<GraphWindow>,
    ) {
        let identity = self.scene.identity;
        let position = self
            .minimum_layouts
            .iter()
            .position(|item| item.identity == identity && item.scope == scope);
        let window = if matches!(scope, GraphScope::Overview { .. }) {
            GraphWindow::FullSource
        } else {
            requested
                .or_else(|| position.map(|id| self.minimum_layouts[id].window))
                .unwrap_or(GraphWindow::Entry)
        };
        if let Some(position) = position {
            if self.minimum_layouts[position].window != window {
                self.minimum_layouts[position].window = window;
                self.minimum_layouts[position].density = None;
            }
        } else {
            self.minimum_layouts.push(LayoutMemory {
                identity,
                scope,
                window,
                density: None,
            });
        }
        if self
            .scene
            .minimum
            .as_ref()
            .is_none_or(|scene| scene.scope != scope || scene.window != window)
        {
            self.scene.minimum = Some(Scene::new(report, scope, window));
            if !self.fit_pending && !self.camera_restored {
                self.entry_pending = true;
            }
        }
    }

    pub(super) fn prepare_minimum_geometry(&mut self, painter: &Painter, canvas: Vec2) {
        let scene = self.scene.minimum.as_mut().unwrap();
        if !scene.geometry_pending {
            return;
        }
        let memory = self
            .minimum_layouts
            .iter_mut()
            .find(|item| item.identity == self.scene.identity && item.scope == scene.scope)
            .unwrap();
        memory.density = scene.arrange(painter, canvas, memory.density);
    }
}

//! Immutable scoped cards and layout, rebuilt only for a new publication/scope.
use super::{
    GraphScope, Selection,
    control::{Control, ProjectionError},
};
use crate::app::model::ReportViewId;
use crate::widgets::graph::{Visibility, layout};
use egui::{FontId, Ui};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::AnalysisReport;

/// Geometry preferences belong to an exact immutable view/scope. Resizing or
/// returning through Navigator must not change the layout under a saved camera.
#[derive(Clone, Copy)]
pub(in crate::widgets::graph) struct LayoutMemory {
    identity: ReportViewId,
    scope: GraphScope,
    point_limit: Option<usize>,
    child_width: f32,
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
    Layout(#[from] geometry::LayoutError),
}

#[derive(Clone, Copy)]
pub(super) enum Picker {
    Origins(usize),
    Targets(usize),
}

pub(in crate::widgets::graph) struct Scene {
    pub(super) scope: GraphScope,
    pub(super) control: Control,
    pub(super) text: Vec<super::text::CardText>,
    pub(super) context_caption: String,
    pub(super) placement: super::super::Placement,
    pub(super) selected: Option<usize>,
    pub(super) error: Option<SceneError>,
    pub(super) nodes_visible: Visibility,
    pub(super) edges_visible: Visibility,
    pub(super) picker: Option<Picker>,
}

impl Scene {
    pub(super) fn new(
        report: &AnalysisReport,
        scope: GraphScope,
        ui: &Ui,
        layout: LayoutMemory,
    ) -> Self {
        let mut scene = Self {
            scope,
            control: Control::default(),
            text: Vec::new(),
            context_caption: String::new(),
            placement: Default::default(),
            selected: None,
            error: None,
            nodes_visible: Default::default(),
            edges_visible: Default::default(),
            picker: None,
        };
        let projected = if let Some(limit) = layout.point_limit {
            Control::scoped_with_limit(report, scope, limit)
        } else {
            Control::scoped(report, scope)
        };
        match projected {
            Ok(control) => {
                debug_assert_eq!(control.scope, Some(scope));
                scene.control = control;
            }
            Err(error) => {
                scene.error = Some(error.into());
                return scene;
            }
        }
        scene.text = scene
            .control
            .cards
            .iter()
            .map(|card| super::text::CardText::new(report, card))
            .collect();
        for (id, text) in scene.text.iter_mut().enumerate() {
            if Some(id) != scene.control.anchor {
                text.size.x = layout.child_width;
            }
        }
        scene.context_caption = super::text::context_caption(report, scope, &scene.control);
        let input = geometry::Input {
            nodes: scene
                .control
                .cards
                .iter()
                .enumerate()
                .map(|(id, card)| geometry::Node {
                    id,
                    size: geometry::Size::new(
                        f64::from(scene.text[id].size.x),
                        f64::from(scene.text[id].size.y),
                    ),
                    program: None,
                    block: id,
                    frame_depth: None,
                    boundary: card.boundary,
                })
                .collect(),
            edges: scene
                .control
                .edges
                .iter()
                .enumerate()
                .map(|(id, edge)| {
                    let measured = ui
                        .painter()
                        .layout_no_wrap(edge.label.clone(), FontId::monospace(10.0), edge.color)
                        .size();
                    geometry::Edge {
                        id,
                        from: edge.from,
                        to: edge.to,
                        kind: layout::kind(edge.kind),
                        label: geometry::Label {
                            text: edge.label.clone(),
                            size: if edge.label.is_empty() {
                                geometry::Size::new(1.0, 1.0)
                            } else {
                                geometry::Size::new(
                                    f64::from(measured.x + 8.0),
                                    f64::from(measured.y + 4.0),
                                )
                            },
                        },
                    }
                })
                .collect(),
        };
        match layout::scene(&input) {
            Ok(placement) => scene.placement = placement,
            Err(error) => scene.error = Some(error.into()),
        }
        scene.nodes_visible = Visibility::new(&scene.placement.nodes);
        let edge_bounds = scene
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
        scene.edges_visible = Visibility::new(&edge_bounds);
        scene
    }

    pub(super) fn contains(&self, selection: &Selection) -> bool {
        if self.scope.summary() != selection.summary {
            return false;
        }
        self.control
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
}

impl super::Graph {
    pub(super) fn ensure_minimum_scope(
        &mut self,
        report: &AnalysisReport,
        scope: GraphScope,
        ui: &Ui,
    ) {
        if self
            .scene
            .minimum
            .as_ref()
            .is_none_or(|scene| scene.scope != scope)
        {
            let identity = self.scene.identity;
            let layout = self
                .minimum_layouts
                .iter()
                .find(|layout| layout.identity == identity && layout.scope == scope)
                .copied()
                .unwrap_or_else(|| {
                    let narrow = ui.available_width() < 600.0;
                    let layout = LayoutMemory {
                        identity,
                        scope,
                        point_limit: narrow.then_some(3),
                        child_width: if narrow {
                            ((ui.available_width() - 48.0) * 0.5).clamp(120.0, 200.0)
                        } else {
                            300.0
                        },
                    };
                    self.minimum_layouts.push(layout);
                    layout
                });
            self.scene.minimum = Some(Scene::new(report, scope, ui, layout));
            if !self.fit_pending && !self.camera_restored {
                self.entry_pending = true;
            }
        }
    }
}

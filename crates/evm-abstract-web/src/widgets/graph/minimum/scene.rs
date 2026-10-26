//! Immutable scoped cards and layout, rebuilt only for a new publication/scope.
use super::{
    GraphScope, Selection,
    control::{Control, ProjectionError},
};
use crate::widgets::graph::{Visibility, layout};
use egui::{FontId, Ui};
use evm_abstract_layout as geometry;
use evm_abstract_protocol::AnalysisReport;

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
    pub(super) placement: super::super::Placement,
    pub(super) selected: Option<usize>,
    pub(super) error: Option<SceneError>,
    pub(super) nodes_visible: Visibility,
    pub(super) edges_visible: Visibility,
    pub(super) picker: Option<Picker>,
}

impl Scene {
    pub(super) fn new(report: &AnalysisReport, scope: GraphScope, ui: &Ui) -> Self {
        let mut scene = Self {
            scope,
            control: Control::default(),
            placement: Default::default(),
            selected: None,
            error: None,
            nodes_visible: Default::default(),
            edges_visible: Default::default(),
            picker: None,
        };
        match Control::scoped(report, scope) {
            Ok(control) => {
                debug_assert_eq!(control.scope, Some(scope));
                scene.control = control;
            }
            Err(error) => {
                scene.error = Some(error.into());
                return scene;
            }
        }
        let input = geometry::Input {
            nodes: scene
                .control
                .cards
                .iter()
                .enumerate()
                .map(|(id, card)| geometry::Node {
                    id,
                    size: geometry::Size::new(300.0, 88.0),
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

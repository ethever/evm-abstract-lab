//! One compact renderer toolbar. View policy is explicit and independent of the
//! semantic context; these controls emit no synthetic application action.
use super::{
    Graph, GraphScope, Selection,
    control::{FrontierDirection, GraphWindow},
    scene::Picker,
};
use crate::controls::{self, Density};
use egui::Ui;

pub(super) struct Action {
    pub entry: bool,
    pub window: Option<GraphWindow>,
    pub scope: GraphScope,
}

impl Graph {
    pub(super) fn minimum_controls(
        &mut self,
        ui: &mut Ui,
        scope: GraphScope,
        selection: &Selection,
    ) -> Action {
        let mut action = Action {
            entry: false,
            window: None,
            scope,
        };
        let scene = self.scene.minimum.as_ref().unwrap();
        let counts = scene.control.counts;
        let current = scene.window;
        let incoming = scene
            .control
            .frontiers
            .iter()
            .any(|frontier| frontier.direction == FrontierDirection::Incoming);
        let hidden = scene.control.hidden_set;
        let model = scene.control.model_set;
        let mut picker = None;
        Density::Compact.scope(ui, |ui| {
            ui.horizontal(|ui| {
                let menu = ui.menu_button("Graph", |ui| {
                    if controls::action(ui, "Entry", Density::Compact).clicked() {
                        action.entry = true;
                        ui.close();
                    }
                    if controls::action(ui, "Fit graph", Density::Compact).clicked() {
                        self.request_fit(ui, true);
                        ui.close();
                    }
                    if controls::action_enabled(
                        ui,
                        selection.node.is_some(),
                        "Focus selected",
                        Density::Compact,
                    )
                    .clicked()
                    {
                        self.focus_selection();
                        ui.close();
                    }
                    ui.separator();
                    for (label, window) in [
                        ("Entry view", GraphWindow::Entry),
                        ("Nearby", GraphWindow::Nearby { point_limit: 12 }),
                        ("Full source CFG", GraphWindow::FullSource),
                    ] {
                        let response =
                            ui.selectable_label(!self.whole_scope && current == window, label);
                        controls::observe(
                            ui,
                            &response,
                            label,
                            "graph-window",
                            Some(!self.whole_scope && current == window),
                        );
                        if response.clicked() {
                            self.whole_scope = false;
                            action.window = Some(window);
                            action.scope = selection.node.map_or(
                                GraphScope::Entry {
                                    summary: selection.summary,
                                },
                                |node| {
                                    super::ProjectionTarget {
                                        summary: selection.summary,
                                        procedure: selection.procedure,
                                        origin: super::ControlOrigin {
                                            node,
                                            location: selection.control,
                                        },
                                    }
                                    .scope()
                                },
                            );
                            ui.close();
                        }
                    }
                    if crate::framework::checkbox(ui, &mut self.whole_scope, "Model overview")
                        .changed()
                    {
                        action.scope = if self.whole_scope {
                            GraphScope::Overview {
                                summary: scope.summary(),
                            }
                        } else {
                            selection.node.map_or(
                                GraphScope::Entry {
                                    summary: selection.summary,
                                },
                                |node| {
                                    super::ProjectionTarget {
                                        summary: selection.summary,
                                        procedure: selection.procedure,
                                        origin: super::ControlOrigin {
                                            node,
                                            location: selection.control,
                                        },
                                    }
                                    .scope()
                                },
                            )
                        };
                        if !self.whole_scope {
                            action.window = Some(GraphWindow::Entry);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if controls::action_enabled(
                        ui,
                        hidden.is_some(),
                        "Hidden control",
                        Density::Compact,
                    )
                    .clicked()
                    {
                        picker = hidden.map(Picker::Targets);
                        ui.close();
                    }
                    if controls::action_enabled(ui, incoming, "Incoming links", Density::Compact)
                        .clicked()
                    {
                        picker = Some(Picker::Incoming(None));
                        ui.close();
                    }
                    if controls::action_enabled(
                        ui,
                        model.is_some(),
                        "Model relations",
                        Density::Compact,
                    )
                    .clicked()
                    {
                        picker = model.map(Picker::Targets);
                        ui.close();
                    }
                    ui.separator();
                    ui.label(format!(
                        "{} / {} context receipts · {} other-context receipts",
                        counts.displayed_receipts,
                        counts.context_receipts,
                        counts.other_context_receipts
                    ));
                    ui.label(format!(
                        "{} incoming · {} outgoing omitted flows",
                        counts.incoming_flows, counts.outgoing_flows
                    ));
                    ui.label(format!(
                        "{} summarized model relations · {} retained topology",
                        counts.summarized_model_relations, counts.retained_topology
                    ));
                    ui.label(format!(
                        "{} calls · {} deliveries · {} unexpanded points",
                        counts.calls, counts.deliveries, counts.unexpanded_points
                    ));
                });
                controls::observe(ui, &menu.response, "Graph", "menu", None);
                ui.label(
                    egui::RichText::new(format!(
                        "{} · {} / {} points",
                        match current {
                            GraphWindow::Entry => "Entry",
                            GraphWindow::Nearby { .. } => "Nearby",
                            GraphWindow::FullSource => "Full source",
                        },
                        counts.displayed_points,
                        counts.total_points
                    ))
                    .small(),
                );
            });
        });
        if let Some(picker) = picker {
            self.scene.minimum.as_mut().unwrap().picker = Some(picker);
        }
        action
    }
}

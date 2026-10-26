//! Separate receipt selection and target navigation; neither action discards
//! the other, including pending/historical unknown-jump hubs.
use super::{
    Graph, Selection,
    paint::{self, CardPaint, paint_card},
};
use crate::{palette, widgets::graph::paint as lod};
use egui::{Align2, FontId, Painter, Rect, Sense, Ui};

#[derive(Clone, Copy)]
pub(super) enum CardAction {
    Receipts(usize),
    Targets(usize),
}

impl Graph {
    pub(super) fn minimum_cards(
        &self,
        ui: &mut Ui,
        painter: &Painter,
        canvas: Rect,
        visible: Rect,
        level: lod::DetailLevel,
        selection: &Selection,
    ) -> Option<CardAction> {
        let scene = self.scene.minimum.as_ref().unwrap();
        let scope = scene.scope;
        let mut clicked = None;
        for id in scene.nodes_visible.query(visible) {
            let card = &scene.control.cards[id];
            let text = &scene.text[id];
            let Some(world) = scene.placement.nodes.get(&id) else {
                continue;
            };
            let rect = self.screen_rect(canvas, *world);
            if !rect.intersects(canvas) {
                continue;
            }
            let actions = paint::actions(rect, card, self.zoom, level);
            let hit = ui.interact(
                actions.body.intersect(canvas),
                ui.id().with(("minimum_control", scope.summary(), id)),
                Sense::click(),
            );
            hit.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    super::card_selected(card, selection),
                    &text.title,
                )
            });
            crate::controls::observe(
                ui,
                &hit,
                &text.title,
                "graph-node",
                Some(super::card_selected(card, selection)),
            );
            paint_card(
                painter,
                rect,
                CardPaint {
                    card,
                    text,
                    selected: super::card_selected(card, selection),
                    hovered: hit.hovered() || hit.has_focus(),
                    zoom: self.zoom,
                    level,
                },
            );
            if hit.clicked() {
                clicked = Some(CardAction::Receipts(id));
            }
            hit.on_hover_ui(|ui| {
                ui.label(format!("{}\n{}\n{}", text.title, text.detail, card.hover));
                ui.label(format!(
                    "{} independent receipts · zoom in for Receipts and Targets actions",
                    card.origins.len()
                ));
            });
            if let Some(footer) = actions.receipts {
                let label = format!("Receipts ({})", card.origins.len());
                let hit = ui.interact(
                    footer.intersect(canvas),
                    ui.id().with(("control_receipts", scope.summary(), id)),
                    Sense::click(),
                );
                hit.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), &label)
                });
                crate::controls::observe(ui, &hit, &label, "graph-receipts", None);
                if footer.intersects(canvas) {
                    painter.with_clip_rect(rect.intersect(canvas)).text(
                        footer.center(),
                        Align2::CENTER_CENTER,
                        label,
                        FontId::proportional(11.0 * self.zoom),
                        palette::MUTED,
                    );
                }
                if hit.clicked() {
                    clicked = Some(CardAction::Receipts(id));
                }
            }
            if let Some(footer) = actions.targets {
                let hit = ui.interact(
                    footer.intersect(canvas),
                    ui.id().with(("control_targets", scope.summary(), id)),
                    Sense::click(),
                );
                hit.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        ui.is_enabled(),
                        "Open targets",
                    )
                });
                crate::controls::observe(ui, &hit, "Targets →", "graph-targets", None);
                if footer.intersects(canvas) {
                    painter.with_clip_rect(rect.intersect(canvas)).text(
                        footer.center(),
                        Align2::CENTER_CENTER,
                        "Targets →",
                        FontId::proportional(11.0 * self.zoom),
                        palette::BLUE,
                    );
                }
                if hit.clicked() {
                    clicked = Some(CardAction::Targets(id));
                }
            }
        }
        clicked
    }
}

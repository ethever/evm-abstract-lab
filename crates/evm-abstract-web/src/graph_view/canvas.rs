//! Content-neutral graph canvas. Callers retain their typed entity identities
//! and translate the returned layout key before changing a workspace selection.

use super::{Placement, fit_scale, paint_path, scene, screen_rect, zoom_at};
use crate::{controls, controls::Density, palette};
use egui::{Align2, Color32, FontId, Sense, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_layout as geometry;

#[derive(Clone, PartialEq)]
pub(crate) struct Card {
    pub key: usize,
    pub title: String,
    pub lines: Vec<String>,
    pub total_lines: usize,
    pub markers: Vec<(String, Color32)>,
    pub boundary: bool,
}

#[derive(Clone, PartialEq)]
pub(crate) struct Connection {
    pub key: usize,
    pub from: usize,
    pub to: usize,
    pub label: String,
    pub kind: geometry::EdgeKind,
    pub color: Color32,
}

#[derive(Clone, Default, PartialEq)]
pub(crate) struct Scene {
    pub cards: Vec<Card>,
    pub connections: Vec<Connection>,
}

pub(crate) struct Canvas {
    identity: Option<egui::Id>,
    content: Scene,
    placement: Placement,
    error: Option<String>,
    zoom: f32,
    pan: Vec2,
    fit: bool,
    cancel_wheel: bool,
    focus: bool,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            identity: None,
            content: Scene::default(),
            placement: Placement::default(),
            error: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            cancel_wheel: false,
            focus: false,
        }
    }
}

impl Canvas {
    pub(crate) fn focus_selected(&mut self) {
        self.focus = true;
    }
    pub(crate) fn prepare_input(&mut self, input: &mut egui::RawInput) {
        if std::mem::take(&mut self.cancel_wheel) {
            input.events.insert(
                0,
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::ZERO,
                    phase: egui::TouchPhase::Cancel,
                    modifiers: egui::Modifiers::NONE,
                },
            );
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut Ui,
        identity: egui::Id,
        title: &str,
        content: &Scene,
        selected: Option<usize>,
    ) -> Option<usize> {
        if self.identity != Some(identity) || self.content != *content {
            self.identity = Some(identity);
            self.content.clone_from(content);
            let input = measure(ui, &self.content);
            match scene(&input) {
                Ok(placement) => {
                    self.placement = placement;
                    self.error = None;
                }
                Err(error) => {
                    self.placement = Placement::default();
                    self.error = Some(error.to_string());
                }
            }
            self.fit = true;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong(title);
            if controls::action(ui, format!("Fit {title}"), Density::Compact).clicked() {
                self.fit = true;
                self.cancel_wheel = true;
            }
            if controls::action_enabled(
                ui,
                selected.is_some(),
                format!("Focus {title}"),
                Density::Compact,
            )
            .clicked()
            {
                self.focus = true;
            }
            ui.label(format!(
                "{} blocks · {} edges · {:.0}%",
                self.content.cards.len(),
                self.content.connections.len(),
                self.zoom * 100.0
            ));
        });
        if let Some(error) = &self.error {
            ui.colored_label(palette::ERROR, format!("Graph layout: {error}"));
            return None;
        }
        if self.content.cards.is_empty() {
            crate::widgets::empty(ui, "No recovered blocks in this stage");
            return None;
        }
        let (response, painter) = ui.allocate_painter(
            ui.available_size().max(Vec2::splat(1.0)),
            Sense::click_and_drag(),
        );
        controls::observe(ui, &response, title, "graph-canvas", None);
        let canvas = response.rect;
        if self.fit && self.placement.bounds.is_finite() {
            self.zoom = fit_scale(self.placement.bounds.size(), canvas.size()).min(1.0);
            self.pan = canvas.size() * 0.5 - self.placement.bounds.center().to_vec2() * self.zoom;
            self.fit = false;
        }
        if self.focus
            && let Some(rect) = selected.and_then(|key| self.placement.nodes.get(&key))
        {
            self.pan = canvas.size() * 0.5 - rect.center().to_vec2() * self.zoom;
            self.focus = false;
        }
        let touch = ui.input(|input| input.multi_touch());
        if response.dragged() && touch.is_none() {
            self.pan += response.drag_delta();
        }
        if response.contains_pointer() && !egui::Popup::is_any_open(ui.ctx()) {
            let (scroll, zoom, pointer) = ui.input(|input| {
                (
                    input.smooth_scroll_delta()
                        + touch.map_or(Vec2::ZERO, |touch| touch.translation_delta),
                    input.zoom_delta(),
                    touch
                        .map(|touch| touch.center_pos - touch.translation_delta)
                        .or_else(|| input.pointer.hover_pos()),
                )
            });
            if let Some(pointer) = pointer {
                zoom_at(&mut self.zoom, &mut self.pan, pointer - canvas.min, zoom);
                if scroll.is_finite() {
                    self.pan += scroll;
                }
            }
        }
        if response.double_clicked() {
            self.fit = true;
            self.cancel_wheel = true;
            ui.ctx().request_repaint();
        }
        let origin = canvas.min + self.pan;
        for edge in &self.content.connections {
            let Some(route) = self.placement.edges.get(&edge.key) else {
                continue;
            };
            paint_path(
                &painter,
                route,
                origin,
                self.zoom,
                Stroke::new(1.2, edge.color),
            );
            if self.zoom >= 0.6 {
                let label = screen_rect(canvas, route.label, self.pan, self.zoom);
                if label.intersects(canvas) {
                    painter.rect_filled(label, 2.0, palette::BACKGROUND);
                    painter.with_clip_rect(label.intersect(canvas)).text(
                        label.center(),
                        Align2::CENTER_CENTER,
                        &edge.label,
                        FontId::monospace(10.0 * self.zoom),
                        edge.color,
                    );
                }
            }
        }
        let mut picked = None;
        for card in &self.content.cards {
            let Some(world) = self.placement.nodes.get(&card.key) else {
                continue;
            };
            let rect = screen_rect(canvas, *world, self.pan, self.zoom);
            if !rect.intersects(canvas) {
                continue;
            }
            let hit = ui.interact(
                rect.intersect(canvas),
                identity.with(card.key),
                Sense::click(),
            );
            let active = selected == Some(card.key);
            hit.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    active,
                    &card.title,
                )
            });
            controls::observe(ui, &hit, &card.title, "decompile-block", Some(active));
            painter.rect(
                rect,
                3.0,
                if active {
                    palette::SELECTED
                } else {
                    palette::RAISED
                },
                Stroke::new(
                    if active { 2.0 } else { 1.0 },
                    if active {
                        palette::ACCENT
                    } else {
                        palette::BORDER
                    },
                ),
                StrokeKind::Inside,
            );
            if self.zoom >= 0.4 {
                let clipped =
                    painter.with_clip_rect(rect.shrink(3.0 * self.zoom).intersect(canvas));
                let mut at = rect.min + Vec2::splat(8.0 * self.zoom);
                clipped.text(
                    at,
                    Align2::LEFT_TOP,
                    &card.title,
                    FontId::monospace(12.0 * self.zoom),
                    palette::TEXT,
                );
                at.y += 20.0 * self.zoom;
                for (marker, color) in &card.markers {
                    clipped.text(
                        at,
                        Align2::LEFT_TOP,
                        marker,
                        FontId::monospace(11.0 * self.zoom),
                        *color,
                    );
                    at.y += 16.0 * self.zoom;
                }
                if self.zoom >= 0.65 {
                    for (line_index, line) in card.lines.iter().enumerate() {
                        let text_rect = clipped.text(
                            at,
                            Align2::LEFT_TOP,
                            line,
                            FontId::monospace(11.0 * self.zoom),
                            palette::MUTED,
                        );
                        let shown = text_rect.intersect(clipped.clip_rect());
                        if shown.is_positive() {
                            let preview = ui.interact(
                                shown,
                                identity.with((card.key, "preview", line_index)),
                                Sense::hover(),
                            );
                            controls::observe(ui, &preview, line, "graph-preview", None);
                        }
                        at.y += 16.0 * self.zoom;
                    }
                    if card.total_lines > card.lines.len() {
                        clipped.text(
                            at,
                            Align2::LEFT_TOP,
                            format!(
                                "… {} more lines; open IR for the full body",
                                card.total_lines - card.lines.len()
                            ),
                            FontId::monospace(10.0 * self.zoom),
                            palette::MUTED,
                        );
                    }
                }
            }
            if hit.clicked() {
                picked = Some(card.key);
            }
            hit.on_hover_ui(|ui| {
                ui.strong(&card.title);
                for (label, color) in &card.markers {
                    ui.colored_label(*color, label);
                }
                for line in &card.lines {
                    ui.monospace(line);
                }
            });
        }
        picked
    }
}

fn measure(ui: &Ui, content: &Scene) -> geometry::Input {
    let nodes = content
        .cards
        .iter()
        .map(|card| {
            let width = std::iter::once(card.title.as_str())
                .chain(card.lines.iter().map(String::as_str))
                .chain(card.markers.iter().map(|(label, _)| label.as_str()))
                .map(|line| {
                    ui.painter()
                        .layout_no_wrap(line.to_owned(), FontId::monospace(11.0), palette::TEXT)
                        .size()
                        .x
                })
                .fold(180.0_f32, f32::max)
                .min(520.0)
                + 16.0;
            geometry::Node {
                id: card.key,
                size: geometry::Size::new(
                    f64::from(width),
                    (42 + (card.lines.len()
                        + card.markers.len()
                        + usize::from(card.total_lines > card.lines.len()))
                        * 16) as f64,
                ),
                program: None,
                block: card.key,
                frame_depth: None,
                boundary: card.boundary,
            }
        })
        .collect();
    let edges = content
        .connections
        .iter()
        .map(|edge| {
            let measured = ui
                .painter()
                .layout_no_wrap(edge.label.clone(), FontId::monospace(10.0), palette::MUTED)
                .size();
            geometry::Edge {
                id: edge.key,
                from: edge.from,
                to: edge.to,
                kind: edge.kind,
                label: geometry::Label {
                    text: edge.label.clone(),
                    size: geometry::Size::new(
                        f64::from(measured.x + 6.0),
                        f64::from(measured.y + 4.0),
                    ),
                },
            }
        })
        .collect();
    geometry::Input { nodes, edges }
}

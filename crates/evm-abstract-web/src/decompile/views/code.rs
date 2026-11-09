//! Structured rows keep status, depth and selectable text in independent
//! columns. Metrics depend only on the active font and bounded status alphabet.

use super::super::{diff::Marker, model::ViewState, rows, structured::Line};
use crate::{controls, palette};
use egui::{
    Align, Galley, Label, Rect, Response, RichText, TextStyle, TextWrapMode, Ui, WidgetText,
};
use evm_abstract_protocol::decompile_model::{Artifact, EntityRef};
use std::{collections::BTreeSet, sync::Arc};

const STATUSES: [(Marker, &str); 5] = [
    (Marker::New, "N"),
    (Marker::Copied, "C"),
    (Marker::Removed, "−"),
    (Marker::Redirected, "→"),
    (Marker::Changed, "Δ"),
];

pub(super) fn legend(ui: &mut Ui) {
    ui.label(
        RichText::new("Status: N New · C Copied · − Removed · → Redirected · Δ Changed")
            .small()
            .color(palette::MUTED),
    );
}

pub(super) struct Layout {
    pub row_height: f32,
    indent: f32,
    slot: f32,
    gap: f32,
}

impl Layout {
    pub fn new(ui: &Ui) -> Self {
        let spaces = galley(ui, RichText::new("    ").monospace(), Align::LEFT);
        let status_size = STATUSES
            .iter()
            .map(|(_, symbol)| galley(ui, RichText::new(*symbol).monospace(), Align::LEFT).size())
            .fold(egui::Vec2::ZERO, egui::Vec2::max);
        Self {
            row_height: rows::HEIGHT.max(spaces.size().y).max(status_size.y),
            indent: spaces.size().x,
            slot: status_size.x.ceil() + 4.0,
            gap: ui.spacing().item_spacing.x.max(4.0),
        }
    }

    pub fn line(
        &self,
        ui: &mut Ui,
        artifact: &Artifact,
        state: &mut ViewState,
        at: EntityRef,
        line: &Line,
        markers: Option<&BTreeSet<Marker>>,
    ) -> bool {
        let row = ui.max_rect();
        let gutter = Rect::from_min_size(
            row.min,
            egui::vec2(self.slot * STATUSES.len() as f32, self.row_height),
        );
        let selected = state
            .selected_in(artifact, at.stage)
            .is_some_and(|selected| {
                state
                    .index
                    .stage(at.stage)
                    .shared
                    .row_matches(at.entity, selected)
            });
        let text = format!("{}{}", "    ".repeat(line.indent), line.text);
        for (position, (marker, symbol)) in STATUSES.iter().enumerate() {
            if markers.is_some_and(|markers| markers.contains(marker)) {
                let status = galley(
                    ui,
                    RichText::new(*symbol).monospace().color(marker.color()),
                    Align::LEFT,
                );
                let rect = Rect::from_center_size(
                    egui::pos2(
                        gutter.left() + (position as f32 + 0.5) * self.slot,
                        gutter.center().y,
                    ),
                    status.size(),
                );
                let response = ui.place(rect, Label::new(status).selectable(false));
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Label,
                        ui.is_enabled(),
                        marker.label(),
                    )
                });
                controls::observe(
                    ui,
                    &response,
                    &format!("{} · {text}", marker.label()),
                    "decompile-code-marker",
                    Some(selected),
                );
                response.on_hover_text(marker.label());
            }
        }

        // Centered text layout excludes leading whitespace from body geometry,
        // retaining it in Galley::text for native drag-copy. The explicit depth
        // offset below positions the body; copy indentation adds no display width.
        let code = galley(
            ui,
            RichText::new(&text).monospace().color(if selected {
                palette::ACCENT
            } else {
                palette::TEXT
            }),
            Align::Center,
        );
        let label = if let Some(markers) = markers.filter(|markers| !markers.is_empty()) {
            format!(
                "{} · {text}",
                markers
                    .iter()
                    .map(|marker| marker.label())
                    .collect::<Vec<_>>()
                    .join(" · ")
            )
        } else {
            text
        };
        let rect = Rect::from_min_size(
            egui::pos2(
                gutter.right() + self.gap + line.indent as f32 * self.indent,
                gutter.center().y - code.size().y * 0.5,
            ),
            code.size(),
        );
        let glyphs = code.mesh_bounds;
        // Keep native Label selection and a stable identity independent of how
        // many status widgets were added before it.
        let response = ui
            .new_child(
                egui::UiBuilder::new()
                    .id(ui.id().with("code"))
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::LEFT)),
            )
            .add(
                Label::new(code)
                    .wrap_mode(TextWrapMode::Extend)
                    .sense(egui::Sense::click()),
            );
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, &label)
        });
        controls::observe(ui, &response, &label, "decompile-code", Some(selected));
        observe_geometry(
            ui,
            &response,
            glyphs.translate(response.rect.center_top().to_vec2()),
            &label,
            "decompile-code-glyphs",
            selected,
        );
        observe_geometry(
            ui,
            &response,
            gutter,
            &label,
            "decompile-code-gutter",
            selected,
        );
        // Manual placement must report the full code extent to ScrollArea.
        // Reserve empty status slots too, without adding another widget/target.
        ui.advance_cursor_after_rect(gutter.union(response.rect));
        if response.clicked() {
            state.select(artifact, at);
        }
        rows::visible(ui, &response)
    }
}

fn galley(ui: &Ui, text: RichText, align: Align) -> Arc<Galley> {
    let mut job = Arc::unwrap_or_clone(WidgetText::from(text).into_layout_job(
        ui.style(),
        TextStyle::Monospace.into(),
        ui.text_valign(),
    ));
    job.halign = align;
    job.wrap.max_width = f32::INFINITY;
    ui.fonts_mut(|fonts| fonts.layout_job(job))
}

fn observe_geometry(
    ui: &Ui,
    response: &Response,
    rect: Rect,
    label: &str,
    role: &'static str,
    selected: bool,
) {
    // Reuse the real text response only as an observation carrier. This adds
    // neither an accessibility node nor a competing interaction region.
    let mut geometry = response.clone();
    geometry.rect = rect;
    controls::observe(ui, &geometry, label, role, Some(selected));
}

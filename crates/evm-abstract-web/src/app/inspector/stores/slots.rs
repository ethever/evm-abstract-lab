//! Slot presentation reserves a complete U256 by default and preserves its low
//! bits when resized. Only viewport rows need text layout; no entry scan is used.

use std::sync::Arc;

use egui::{
    Color32, FontId, Id, Sense, Stroke, TextStyle, Ui, epaint::Galley,
    text_selection::LabelSelectionState,
};

// Leave room for glyph ink outside its logical advance, within the cell clip.
const TEXT_MARGIN: f32 = 2.0;

#[derive(Clone, Copy)]
struct WidthPreference {
    default: f32,
    explicit: bool,
}

pub(in crate::app::inspector) fn default_width(ui: &Ui) -> f32 {
    let font = TextStyle::Monospace.resolve(ui.style());
    ui.fonts_mut(|fonts| {
        // StorageEntry keys are compact hex U256s. Reserve all 64 digits even
        // when the longest key is offscreen or arrives in a subsequent report.
        let widest = "0123456789abcdef"
            .chars()
            .max_by(|a, b| {
                fonts
                    .glyph_width(&font, *a)
                    .total_cmp(&fonts.glyph_width(&font, *b))
            })
            .expect("hex digits");
        let sample = format!("0x{}", widest.to_string().repeat(64));
        fonts
            .layout_no_wrap(sample, font, Color32::WHITE)
            .size()
            .x
            .ceil()
            + 2.0 * TEXT_MARGIN
    })
}

/// Only font sizing changes can invalidate an untouched default. Native table
/// state remains the authority for any explicitly dragged width.
pub(in crate::app::inspector) fn refresh_default(ui: &mut Ui, table: Id, default: f32) -> bool {
    ui.data_mut(|data| {
        let previous = data.get_temp::<WidthPreference>(table.with("slot_width_preference"));
        let explicit = previous.is_some_and(|previous| previous.explicit);
        data.insert_temp(
            table.with("slot_width_preference"),
            WidthPreference { default, explicit },
        );
        previous.is_some_and(|previous| !explicit && previous.default != default)
    })
}

pub(in crate::app::inspector) fn remember_resize(ui: &mut Ui, table: Id) {
    remember_resize_at(ui, table, 0);
}

pub(in crate::app::inspector) fn remember_resize_at(ui: &mut Ui, table: Id, column: usize) {
    if ui
        .ctx()
        .read_response(table.with("resize_column").with(column))
        .is_some_and(|response| response.dragged())
    {
        ui.data_mut(|data| {
            if let Some(mut preference) =
                data.get_temp::<WidthPreference>(table.with("slot_width_preference"))
            {
                preference.explicit = true;
                data.insert_temp(table.with("slot_width_preference"), preference);
            }
        });
    }
}

pub(in crate::app::inspector) fn cell(ui: &mut Ui, slot: &str) {
    let cell = ui.max_rect();
    // Table clipping includes half the inter-column spacing. Keep even a
    // sub-ellipsis viewport strictly within the slot cell instead.
    ui.set_clip_rect(ui.clip_rect().intersect(cell));
    let content = cell.shrink2(egui::vec2(TEXT_MARGIN.min(cell.width() * 0.5), 0.0));
    let font = TextStyle::Monospace.resolve(ui.style());
    let color = ui.visuals().text_color();
    let galley = suffix(ui, slot, content.width().max(0.0), font, color);
    let selectable = ui.style().interaction.selectable_labels;
    let mut sense = if ui.memory(|memory| memory.options.screen_reader) {
        Sense::focusable_noninteractive()
    } else {
        Sense::hover()
    };
    // Copying the complete key remains available even when text selection is
    // disabled or the user narrows the column below a single glyph.
    sense |= Sense::click() - Sense::FOCUSABLE;
    if selectable {
        // Match native Label: touch drags belong to the enclosing ScrollArea,
        // and selecting text must not add labels to keyboard tab navigation.
        let mut selection_sense = if ui.input(|input| input.has_touch_screen()) {
            Sense::click()
        } else {
            Sense::click_and_drag()
        };
        selection_sense -= Sense::FOCUSABLE;
        sense |= selection_sense;
    }
    let (_, response) = ui.allocate_exact_size(cell.size(), sense);
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), slot));
    if ui.is_rect_visible(cell) {
        let pos = egui::pos2(
            content.right() - galley.size().x,
            cell.center().y - galley.size().y * 0.5,
        );
        let underline = if response.has_focus() || response.highlighted() {
            Stroke::new(1.0, color)
        } else {
            Stroke::NONE
        };
        if selectable {
            // The native API paints and selects this fitted galley without an
            // extra widget/container per row. Copy returns the displayed text.
            LabelSelectionState::label_text_selection(ui, &response, pos, galley, color, underline);
        } else {
            ui.painter()
                .add(egui::epaint::TextShape::new(pos, galley, color).with_underline(underline));
        }
    }
    // Hover the full cell, including when no glyph fits. The original key is
    // independent of the fitted display string and remains available in full.
    response.context_menu(|ui| {
        let copy = ui.button("Copy full slot key");
        crate::controls::observe(ui, &copy, "Copy full slot key", "action", None);
        if copy.clicked() {
            ui.ctx().copy_text(slot.to_owned());
            ui.close();
        }
        ui.label(slot);
    });
    response.on_hover_ui(|ui| {
        ui.set_max_width(default_width(ui));
        ui.monospace(slot);
    });
}

fn suffix(ui: &Ui, slot: &str, width: f32, font: FontId, color: Color32) -> Arc<Galley> {
    ui.fonts_mut(|fonts| {
        let full = fonts.layout_no_wrap(slot.to_owned(), font.clone(), color);
        if full.size().x <= width {
            return full;
        }
        let mut fitted = fonts.layout_no_wrap("…".into(), font.clone(), color);
        if fitted.size().x > width {
            return fonts.layout_no_wrap(String::new(), font, color);
        }
        let boundaries: Vec<_> = slot.char_indices().map(|(index, _)| index).collect();
        let mut low = 0;
        let mut high = boundaries.len();
        // Measure actual candidates, including ellipsis and font rounding.
        // Binary search is bounded by the 66-character protocol key length.
        while low < high {
            let count = low + (high - low).div_ceil(2);
            let start = boundaries[boundaries.len() - count];
            let candidate =
                fonts.layout_no_wrap(format!("…{}", &slot[start..]), font.clone(), color);
            if candidate.size().x <= width {
                fitted = candidate;
                low = count;
            } else {
                high = count - 1;
            }
        }
        fitted
    })
}

#[cfg(test)]
mod tests;

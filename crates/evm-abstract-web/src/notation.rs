//! Native typesetting of canonical notation. Unicode stays in jobs, widget
//! metadata and copies; small font subsets map indices to ordinary glyphs.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, Color32, FontDefinitions, FontFamily, FontId, FontSelection, Painter, Pos2,
    Rect, Ui, WidgetText,
};
use evm_abstract_notation::subscript_digit;

#[cfg(test)]
mod tests;

const INDEX_SCALE: f32 = 0.75;
const MATH_MONO: &str = "MathIndicesMono";
const MATH_SANS: &str = "MathIndicesSans";

/// The subsets preserve their source typeface's metrics. They are selected only
/// for indexed runs; the existing body/code/emoji fallback chains are untouched.
pub(crate) fn install_fonts(fonts: &mut FontDefinitions) {
    for (name, bytes) in [
        (
            MATH_MONO,
            include_bytes!("../assets/fonts/math-indices-hack.ttf").as_slice(),
        ),
        (
            MATH_SANS,
            include_bytes!("../assets/fonts/math-indices-sans.ttf").as_slice(),
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
        fonts
            .families
            .insert(FontFamily::Name(name.into()), vec![name.into()]);
    }
}

#[cfg(test)]
pub(crate) fn initialize_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    install_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

/// Preserve styles, interaction colors and the canonical text read by egui.
pub(crate) fn widget(ui: &Ui, text: impl Into<WidgetText>) -> WidgetText {
    let text = text.into();
    if !text.text().chars().any(indexed) {
        return text;
    }
    let original = text.into_layout_job(ui.style(), FontSelection::Default, Align::Center);
    typeset((*original).clone()).into()
}

pub(crate) fn job(text: &str, font: FontId, color: Color32) -> LayoutJob {
    typeset(LayoutJob::single_section(
        text.into(),
        TextFormat {
            font_id: font,
            color,
            valign: Align::Center,
            ..TextFormat::default()
        },
    ))
}

pub(crate) fn paint(
    painter: &Painter,
    at: Pos2,
    anchor: Align2,
    text: &str,
    font: FontId,
    color: Color32,
) -> Rect {
    let galley = painter.layout_job(job(text, font, color));
    let rect = anchor.anchor_size(at, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}

fn indexed(character: char) -> bool {
    subscript_digit(character).is_some() || character == 'ᵖ'
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Body,
    Subscript,
    Superscript,
}

fn typeset(original: LayoutJob) -> LayoutJob {
    let mut result = LayoutJob {
        text: String::new(),
        sections: Vec::new(),
        ..original.clone()
    };
    for section in original.sections {
        let text = &original.text[section.byte_range.start.0..section.byte_range.end.0];
        let mut run = String::new();
        let mut position = Position::Body;
        let mut first = true;
        for character in text.chars() {
            let next = if subscript_digit(character).is_some() {
                Position::Subscript
            } else if character == 'ᵖ' {
                Position::Superscript
            } else {
                Position::Body
            };
            if !run.is_empty() && next != position {
                append(
                    &mut result,
                    &run,
                    &section.format,
                    position,
                    if first { section.leading_space } else { 0.0 },
                );
                run.clear();
                first = false;
            }
            position = next;
            run.push(character);
        }
        if !run.is_empty() {
            append(
                &mut result,
                &run,
                &section.format,
                position,
                if first { section.leading_space } else { 0.0 },
            );
        }
    }
    result
}

fn append(
    job: &mut LayoutJob,
    text: &str,
    original: &TextFormat,
    position: Position,
    leading: f32,
) {
    let mut format = original.clone();
    if position != Position::Body {
        format.font_id.family = FontFamily::Name(
            if original.font_id.family == FontFamily::Monospace {
                MATH_MONO
            } else {
                MATH_SANS
            }
            .into(),
        );
        format.font_id.size *= INDEX_SCALE;
        format.valign = if position == Position::Subscript {
            Align::BOTTOM
        } else {
            Align::TOP
        };
    }
    job.append(text, leading, format);
}

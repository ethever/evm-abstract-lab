//! Exercise the real slot widget even at widths below the native drag minimum.

use egui::{Context, Event, FontId, Pos2, Rect, Vec2, epaint::TextShape};

fn draw(
    context: &Context,
    time: &mut f64,
    slot: &str,
    width: f32,
    events: Vec<Event>,
) -> (Rect, Vec<(Rect, TextShape)>) {
    *time += 0.1;
    let mut cell = Rect::NOTHING;
    let mut output = context.run_ui(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 240.0))),
            time: Some(*time),
            events,
            ..Default::default()
        },
        |ui| {
            ui.allocate_ui_with_layout(
                Vec2::new(width, 21.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    cell = ui.max_rect();
                    super::cell(ui, slot);
                },
            );
        },
    );
    output.textures_delta.clear();
    let painted = output
        .shapes
        .into_iter()
        .filter_map(|shape| match shape.shape {
            egui::Shape::Text(text) => Some((shape.clip_rect, text)),
            _ => None,
        })
        .collect();
    (cell, painted)
}

fn glyphs(text: &TextShape) -> String {
    text.galley
        .rows
        .iter()
        .flat_map(|row| &row.glyphs)
        .map(|glyph| glyph.chr)
        .collect()
}

#[test]
fn fractional_slot_widths_paint_the_longest_fitting_suffix_at_the_right_edge() {
    let slot = format!("0x{}90abcdef", "ab".repeat(28));
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut time = 0.0;
    for width in [17.3, 23.75, 56.2, 93.25, 128.4, 255.5, 700.0] {
        let (cell, paint) = draw(&context, &mut time, &slot, width, vec![]);
        let (clip, text) = paint.first().expect("slot must paint visible glyphs");
        let displayed = glyphs(text);
        assert!(!displayed.is_empty() && !text.galley.elided);
        assert_eq!(paint.len(), 1, "no hidden full-key text shape");
        assert!(clip.contains_rect(Rect::from_min_size(text.pos, text.galley.size())));
        assert!(
            (text.pos.x + text.galley.size().x - (cell.right() - super::TEXT_MARGIN)).abs() < 0.01
        );
        for row in &text.galley.rows {
            assert!(!row.visuals.mesh.vertices.is_empty());
            let ink = row
                .visuals
                .mesh_bounds
                .translate(text.pos.to_vec2() + row.pos.to_vec2());
            assert!(clip.contains_rect(ink), "{width}: {ink:?} escaped {clip:?}");
        }
        if let Some(tail) = displayed.strip_prefix('…') {
            assert!(slot.ends_with(tail), "{width}: {displayed}");
            let next = &slot[slot.len() - tail.len() - 1..];
            let wider = context.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(
                        format!("…{next}"),
                        FontId::monospace(13.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            });
            assert!(
                wider > width - 2.0 * super::TEXT_MARGIN,
                "another low-order digit would fit at {width}"
            );
        } else {
            assert_eq!(displayed, slot);
        }
    }
}

#[test]
fn sub_ellipsis_widths_keep_all_ink_inside_the_cell_and_original_hover_available() {
    let slot = "0x1234567890abcdef";
    for width in [0.0, 1.0, 4.0, 5.0, 10.0] {
        let context = Context::default();
        crate::notation::initialize_fonts(&context);
        context.global_style_mut(|style| style.animation_time = 0.0);
        let mut time = 0.0;
        let (cell, paint) = draw(&context, &mut time, slot, width, vec![]);
        assert!(
            paint.iter().all(|(clip, text)| {
                clip.left() >= cell.left()
                    && clip.right() <= cell.right()
                    && glyphs(text).is_empty()
            }),
            "no part of an oversized ellipsis may spill from a {width}-point cell"
        );
        if width > 0.0 {
            draw(
                &context,
                &mut time,
                slot,
                width,
                vec![Event::PointerMoved(cell.center())],
            );
            let mut hovered = Vec::new();
            for _ in 0..15 {
                hovered = draw(&context, &mut time, slot, width, vec![]).1;
            }
            assert!(
                hovered
                    .iter()
                    .any(|(_, text)| glyphs(text) == slot && !text.galley.elided),
                "full original hover at width {width}"
            );
        }
    }
}

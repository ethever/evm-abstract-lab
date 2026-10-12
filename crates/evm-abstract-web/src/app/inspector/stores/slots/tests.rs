//! Exercise the real slot widget even at widths below the native drag minimum.

use egui::{Context, Event, FontId, Pos2, Rect, Vec2, epaint::TextShape};

fn draw(
    context: &Context,
    time: &mut f64,
    slot: &str,
    width: f32,
    events: Vec<Event>,
) -> (Rect, Vec<(Rect, TextShape)>) {
    let (cell, paint, _) = draw_with_output(context, time, slot, width, events);
    (cell, paint)
}

fn draw_with_output(
    context: &Context,
    time: &mut f64,
    slot: &str,
    width: f32,
    events: Vec<Event>,
) -> (Rect, Vec<(Rect, TextShape)>, egui::PlatformOutput) {
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
    (cell, painted, output.platform_output)
}

fn glyphs(text: &TextShape) -> String {
    text.galley
        .rows
        .iter()
        .flat_map(|row| &row.glyphs)
        .map(|glyph| glyph.chr)
        .collect()
}

fn pointer_copy(slot: &str, width: Option<f32>, selectable: bool) -> (String, Vec<String>) {
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    context.global_style_mut(|style| style.interaction.selectable_labels = selectable);
    let mut cell_width = width.unwrap_or_default();
    let mut sizing = context.run_ui(egui::RawInput::default(), |ui| {
        if width.is_none() {
            cell_width = super::default_width(ui);
        }
    });
    sizing.textures_delta.clear();
    let mut time = 0.0;
    let (cell, paint) = draw(&context, &mut time, slot, cell_width, vec![]);
    let text = &paint.first().expect("visible slot text").1;
    let displayed = glyphs(text);
    let start = Pos2::new(text.pos.x - 0.5, cell.center().y);
    let end = Pos2::new(text.pos.x + text.galley.size().x + 0.5, cell.center().y);
    // Use the actual widget's hit testing and drag state, then the native copy
    // event. Inspecting only a selection range would miss platform integration.
    for events in [
        vec![Event::PointerMoved(start)],
        vec![Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
        vec![Event::PointerMoved(end)],
        vec![],
        vec![Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        draw(&context, &mut time, slot, cell_width, events);
    }
    let (_, _, output) = draw_with_output(&context, &mut time, slot, cell_width, vec![Event::Copy]);
    let copied = output
        .commands
        .into_iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text),
            _ => None,
        })
        .collect();
    (displayed, copied)
}

#[test]
fn native_pointer_drag_copies_full_default_and_short_slot_text() {
    for slot in [format!("0x{}90abcdef", "ab".repeat(28)), "0x1".into()] {
        let (displayed, copied) = pointer_copy(&slot, None, true);
        assert_eq!(displayed, slot);
        assert_eq!(copied, vec![slot], "native selection must emit CopyText");
    }
}

#[test]
fn native_pointer_drag_copies_the_displayed_suffix() {
    let slot = format!("0x{}90abcdef", "ab".repeat(28));
    let (displayed, copied) = pointer_copy(&slot, Some(93.25), true);
    let tail = displayed.strip_prefix('…').expect("fitted suffix");
    assert!(!tail.is_empty() && slot.ends_with(tail));
    assert_ne!(displayed, slot);
    assert_eq!(copied, vec![displayed]);
}

#[test]
fn disabling_native_label_selection_prevents_pointer_copy() {
    let (displayed, copied) = pointer_copy("0x123456", None, false);
    assert_eq!(displayed, "0x123456");
    assert!(copied.is_empty(), "respect selectable_labels = false");
}

#[test]
fn touch_drags_over_slot_text_scroll_the_native_table() {
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut time = 0.0;
    let mut frame = |events| {
        time += 0.1;
        let mut cell = Rect::NOTHING;
        let mut offset = Vec2::ZERO;
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 240.0))),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ui| {
                let scrolled = egui::ScrollArea::vertical()
                    .id_salt("touch_slot_selection")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        egui_extras::TableBuilder::new(ui)
                            .vscroll(false)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(egui_extras::Column::exact(160.0).clip(true))
                            .body(|body| {
                                body.rows(21.0, 80, |mut row| {
                                    let index = row.index();
                                    row.col(|ui| {
                                        if index == 4 {
                                            cell = ui.max_rect();
                                        }
                                        super::cell(ui, "0x123456");
                                    });
                                });
                            });
                    });
                offset = scrolled.state.offset;
            },
        );
        output.textures_delta.clear();
        (cell, offset, output.platform_output)
    };
    let touch = |phase, pos| Event::Touch {
        device_id: egui::TouchDeviceId(0),
        id: egui::TouchId(1),
        phase,
        pos,
        force: None,
    };
    // Establish a known touch device before starting the gesture, as on a
    // native touch screen. Touch input accompanies its pointer events.
    let (cell, initial, _) = frame(vec![touch(egui::TouchPhase::End, Pos2::ZERO)]);
    let start = Pos2::new(cell.right() - 20.0, cell.center().y);
    let end = start - Vec2::new(0.0, 48.0);
    frame(vec![Event::PointerMoved(start)]);
    frame(vec![
        touch(egui::TouchPhase::Start, start),
        Event::PointerButton {
            pos: start,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    frame(vec![
        touch(egui::TouchPhase::Move, end),
        Event::PointerMoved(end),
    ]);
    let (_, dragged, _) = frame(vec![]);
    assert!(
        dragged.y >= initial.y + 40.0,
        "touch drag over slot text must scroll: {initial:?} -> {dragged:?}"
    );
    frame(vec![
        touch(egui::TouchPhase::End, end),
        Event::PointerButton {
            pos: end,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    let (_, _, output) = frame(vec![Event::Copy]);
    assert!(
        output
            .commands
            .iter()
            .all(|command| !matches!(command, egui::OutputCommand::CopyText(_))),
        "touch scrolling must not create a copied text selection"
    );
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

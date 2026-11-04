//! Regressions inspect production Label galleys and mesh bounds, as well as
//! input-driven selection/copy/scrolling. Text equality alone misses this bug.

use super::super::Views;
use crate::{
    controls,
    decompile::{
        diff::{Differences, Marker},
        model::{Publication, ViewState},
        rows, structured,
        tests::fixture,
    },
};
use egui::{
    Context, Event, FontId, Modifiers, PointerButton, Pos2, RawInput, Rect, RichText, TextStyle,
    Vec2, WidgetText,
};
use evm_abstract_protocol::{
    JobId,
    decompile_model::{
        Artifact, EntityId, EntityRef, StageId, StageKind, StructuredKind, StructuredNode,
        StructuredProgram,
    },
};
use std::collections::BTreeMap;

struct Frame {
    output: egui::FullOutput,
    controls: Vec<serde_json::Value>,
}

fn paint(
    ctx: &Context,
    views: &mut Views,
    artifact: &Artifact,
    state: &mut ViewState,
    size: Vec2,
    events: Vec<Event>,
) -> Frame {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..RawInput::default()
        },
        |ui| {
            controls::begin_observation(ui.ctx(), true);
            rows::begin_frame(ui.ctx());
            views.code(ui, artifact, state);
        },
    );
    output.textures_delta.clear();
    Frame {
        output,
        controls: serde_json::from_str(&controls::observed(ctx).unwrap()).unwrap(),
    }
}

fn nested() -> Artifact {
    let mut artifact = fixture::large(1);
    let leaf = |id, kind| StructuredNode {
        id: EntityId(id),
        kind,
    };
    let program = StructuredProgram {
        root: leaf(
            30,
            StructuredKind::Sequence(vec![
                leaf(
                    40,
                    StructuredKind::While {
                        header: Box::new(leaf(41, StructuredKind::Block { block: EntityId(1) })),
                        condition: None,
                        condition_nonzero: true,
                        body: Box::new(leaf(
                            42,
                            StructuredKind::If {
                                condition: EntityId(100_000),
                                then_branch: Box::new(leaf(43, StructuredKind::Continue)),
                                else_branch: Box::new(leaf(44, StructuredKind::Break)),
                            },
                        )),
                        exit_edge: None,
                    },
                ),
                leaf(
                    50,
                    StructuredKind::If {
                        condition: EntityId(100_000),
                        then_branch: Box::new(leaf(51, StructuredKind::Continue)),
                        else_branch: Box::new(leaf(52, StructuredKind::Break)),
                    },
                ),
            ]),
        ),
    };
    for stage in &mut artifact.stages[1..] {
        stage.kind = StageKind::Structured;
        stage.ir.structured = Some(program.clone());
    }
    artifact
}

fn state(artifact: &Artifact) -> ViewState {
    let mut state = ViewState::default();
    state.install(
        artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    state
}

fn texts(output: &egui::FullOutput) -> Vec<(&egui::epaint::TextShape, Rect)> {
    fn collect<'a>(
        shape: &'a egui::Shape,
        clip: Rect,
        result: &mut Vec<(&'a egui::epaint::TextShape, Rect)>,
    ) {
        match shape {
            egui::Shape::Text(text) => result.push((text, clip)),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, result);
                }
            }
            _ => {}
        }
    }
    let mut result = Vec::new();
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &mut result);
    }
    result
}

fn bounds(control: &serde_json::Value) -> Rect {
    let [left, top, right, bottom] = serde_json::from_value(control["rect"].clone()).unwrap();
    Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, bottom))
}

fn observed<'a>(frame: &'a Frame, role: &str) -> Vec<&'a serde_json::Value> {
    frame
        .controls
        .iter()
        .filter(|control| control["role"] == role)
        .collect()
}

fn copied(frame: &Frame) -> Vec<&str> {
    frame
        .output
        .platform_output
        .commands
        .iter()
        .filter_map(|command| match command {
            egui::OutputCommand::CopyText(text) => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn glyph_x(text: &egui::epaint::TextShape) -> f32 {
    let row = &text.galley.rows[0];
    let glyph = row
        .glyphs
        .iter()
        .find(|glyph| !glyph.chr.is_whitespace())
        .unwrap();
    text.pos.x + row.pos.x + glyph.pos.x
}

fn close(left: f32, right: f32) {
    assert!((left - right).abs() <= 1.0, "{left} != {right}");
}

fn verify_columns(frame: &Frame, lines: &[structured::Line]) -> Vec<f32> {
    let painted: Vec<_> = texts(&frame.output)
        .into_iter()
        .filter(|(text, _)| {
            lines
                .iter()
                .any(|line| line.text == text.galley.text().trim_start())
        })
        .collect();
    let code = observed(frame, "decompile-code");
    let glyphs = observed(frame, "decompile-code-glyphs");
    let gutters = observed(frame, "decompile-code-gutter");
    assert_eq!(
        painted.len(),
        lines.len(),
        "all fixture rows must really paint"
    );
    assert_eq!(code.len(), lines.len());
    assert_eq!(glyphs.len(), lines.len());
    assert_eq!(gutters.len(), lines.len());
    let mut origins = Vec::new();
    let mut previous_bottom = None;
    for (number, ((text, clip), line)) in painted.iter().zip(lines).enumerate() {
        assert_eq!(
            text.galley.text(),
            format!("{}{}", "    ".repeat(line.indent), line.text)
        );
        assert_eq!(text.galley.rows.len(), 1, "code must not wrap");
        assert!(!text.galley.elided);
        assert!(
            text.galley.num_vertices > 0,
            "code must have real glyph meshes"
        );
        let expected = text
            .galley
            .mesh_bounds
            .translate(text.pos.to_vec2())
            .intersect(*clip);
        let actual = bounds(glyphs[number]);
        close(actual.left(), expected.left());
        close(actual.right(), expected.right());
        close(actual.top(), expected.top());
        close(actual.bottom(), expected.bottom());
        assert_eq!(code[number]["id"], glyphs[number]["id"]);
        assert_eq!(code[number]["label"], glyphs[number]["label"]);
        assert_eq!(code[number]["selected"], glyphs[number]["selected"]);
        let gutter = bounds(gutters[number]);
        close(gutter.left(), bounds(gutters[0]).left());
        close(gutter.right(), bounds(gutters[0]).right());
        assert!(gutter.width() < 150.0, "five compact slots: {gutter:?}");
        assert!(gutter.right() < glyph_x(text));
        if let Some(bottom) = previous_bottom {
            assert!(
                bottom <= text.pos.y,
                "font changes must not overlap adjacent rows"
            );
        }
        previous_bottom = Some(text.pos.y + text.galley.size().y);
        origins.push(glyph_x(text));
    }
    let root = origins[0];
    let depth_one = lines.iter().position(|line| line.indent == 1).unwrap();
    let step = origins[depth_one] - root;
    assert!(
        step >= 12.0,
        "visible indentation must survive Label alignment"
    );
    for (line, x) in lines.iter().zip(&origins) {
        close(*x, root + line.indent as f32 * step);
    }
    // The fixture includes while, if/else and nested braces. Their first glyph
    // origins must agree with each owning construct's explicitly stored depth.
    for entity in [40, 42, 50] {
        let owned: Vec<_> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.entity == EntityId(entity))
            .collect();
        let (_, opening) = owned.first().unwrap();
        let closing = owned.iter().find(|(_, line)| line.text == "}").unwrap();
        assert_eq!(opening.indent, closing.1.indent);
        close(origins[owned[0].0], origins[closing.0]);
    }
    origins
}

#[test]
fn structured_status_slots_and_depth_use_real_glyph_geometry_at_each_font_and_density() {
    let artifact = nested();
    let lines = structured::lines(&artifact.stages[2]);
    for (font_size, spacing, override_font) in [
        (12.0, egui::vec2(3.0, 2.0), None),
        (18.0, egui::vec2(9.0, 6.0), None),
        (26.0, egui::vec2(5.0, 3.0), None),
        (13.0, egui::vec2(5.0, 3.0), Some(FontId::monospace(21.0))),
    ] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        ctx.global_style_mut(|style| {
            style
                .text_styles
                .insert(TextStyle::Monospace, FontId::monospace(font_size));
            style.spacing.item_spacing = spacing;
            style.override_font_id = override_font;
        });
        let mut state = state(&artifact);
        let mut views = Views::default();
        let size = Vec2::new(1200.0, 1100.0);
        let baseline = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
        let baseline_x = verify_columns(&baseline, &lines);
        let baseline_ids: Vec<_> = observed(&baseline, "decompile-code")
            .iter()
            .map(|row| row["id"].clone())
            .collect();
        let mut space_width = 0.0;
        let mut measured = ctx.run_ui(RawInput::default(), |ui| {
            space_width = WidgetText::from(RichText::new("    ").monospace())
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    TextStyle::Monospace,
                )
                .size()
                .x;
        });
        measured.textures_delta.clear();
        let nested_continue = lines
            .iter()
            .position(|line| line.entity == EntityId(43))
            .unwrap();
        let outer_continue = lines
            .iter()
            .position(|line| line.entity == EntityId(51))
            .unwrap();
        close(
            baseline_x[nested_continue] - baseline_x[outer_continue],
            space_width,
        );
        for markers in [
            vec![Marker::New],
            vec![Marker::Copied],
            vec![Marker::Redirected, Marker::Changed],
            vec![
                Marker::New,
                Marker::Copied,
                Marker::Removed,
                Marker::Redirected,
                Marker::Changed,
            ],
            vec![],
        ] {
            views
                .differences
                .after
                .insert(EntityId(43), markers.iter().copied().collect());
            views
                .differences
                .after
                .insert(EntityId(50), markers.iter().copied().collect());
            assert!(state.select(
                &artifact,
                EntityRef {
                    stage: StageId(2),
                    entity: EntityId(43)
                }
            ));
            let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
            let x = verify_columns(&frame, &lines);
            for (actual, expected) in x.iter().zip(&baseline_x) {
                close(*actual, *expected);
            }
            let code = observed(&frame, "decompile-code");
            let ids: Vec<_> = code.iter().map(|row| row["id"].clone()).collect();
            assert_eq!(
                ids, baseline_ids,
                "marker count must not change code widget identity"
            );
            assert_eq!(code[nested_continue]["selected"], true);
            let label = code[nested_continue]["label"].as_str().unwrap();
            assert!(label.ends_with("        continue;"));
            for marker in &markers {
                assert!(label.contains(marker.label()));
            }
            let status = observed(&frame, "decompile-code-marker");
            assert_eq!(status.len(), markers.len() * 4);
            let painted = texts(&frame.output);
            let selected = painted
                .iter()
                .find(|(text, _)| text.galley.text().trim_start() == "continue;")
                .unwrap()
                .0;
            assert_eq!(
                selected.galley.job.sections[0].format.color,
                crate::palette::ACCENT
            );
        }
        state.set_stage(&artifact, StageId(1));
        views.differences = Differences::default();
        let changed_stage = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
        let x = verify_columns(&changed_stage, &lines);
        for (actual, expected) in x.iter().zip(&baseline_x) {
            close(*actual, *expected);
        }
    }
}

fn click(
    ctx: &Context,
    views: &mut Views,
    artifact: &Artifact,
    state: &mut ViewState,
    size: Vec2,
    pos: Pos2,
) -> Frame {
    paint(
        ctx,
        views,
        artifact,
        state,
        size,
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    paint(
        ctx,
        views,
        artifact,
        state,
        size,
        vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    )
}

#[test]
fn structured_real_clicks_select_entities_and_copy_exact_indentation_without_statuses() {
    let artifact = nested();
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut state = state(&artifact);
    let mut views = Views::default();
    views
        .differences
        .after
        .insert(EntityId(43), [Marker::Copied, Marker::Changed].into());
    let size = Vec2::new(1200.0, 1100.0);
    paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let row = observed(&frame, "decompile-code")
        .into_iter()
        .find(|row| {
            row["label"]
                .as_str()
                .unwrap()
                .ends_with("        continue;")
        })
        .unwrap();
    let clicked = click(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        size,
        bounds(row).center(),
    );
    assert!(
        clicked.output.platform_output.events.iter().any(|event| {
            let info = event.widget_info();
            matches!(event, egui::output::OutputEvent::Clicked(_))
                && info.typ == egui::WidgetType::Button
                && info.label.as_deref() == row["label"].as_str()
                && info.selected.is_some()
        }),
        "native accessibility keeps the full marked code label and selected Button role"
    );
    assert_eq!(state.selected_in(&artifact, StageId(2)), Some(EntityId(43)));
    let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let copy = frame
        .controls
        .iter()
        .find(|control| control["role"] == "action" && control["label"] == "Copy structured text")
        .unwrap();
    let frame = click(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        size,
        bounds(copy).center(),
    );
    let expected = structured::lines(&artifact.stages[2])
        .iter()
        .map(|line| format!("{}{}", "    ".repeat(line.indent), line.text))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(copied(&frame), vec![expected.as_str()]);
    assert!(expected.contains("\n        continue;\n"));
}

#[test]
fn structured_native_drag_copy_retains_full_indentation_without_status_symbols() {
    let artifact = nested();
    let expected = "        continue;";
    for (font_size, interact_radius) in [(13.0, 5.0), (18.0, 12.0), (26.0, 0.0)] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        ctx.global_style_mut(|style| {
            style
                .text_styles
                .insert(TextStyle::Monospace, FontId::monospace(font_size));
            style.interaction.interact_radius = interact_radius;
        });
        let mut state = state(&artifact);
        let mut views = Views::default();
        views
            .differences
            .after
            .insert(EntityId(43), [Marker::Copied, Marker::Changed].into());
        let size = Vec2::new(1200.0, 1100.0);
        paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
        let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
        let (text, _) = texts(&frame.output)
            .into_iter()
            .find(|(text, _)| text.galley.text() == expected)
            .unwrap();
        let body = text.galley.rect.translate(text.pos.to_vec2());
        let control = observed(&frame, "decompile-code")
            .into_iter()
            .find(|control| control["label"].as_str().unwrap().ends_with(expected))
            .unwrap();
        let hit = bounds(control);
        // Centered Galley bounds include pixel rounding and can extend beyond
        // Label's actual response. Press inside the real hit rectangle even
        // with zero interact_radius, while staying in the final caret cell.
        let last_advance = text.galley.rows[0].glyphs.last().unwrap().advance_width;
        let from = hit.right_center() - egui::vec2(last_advance * 0.25, 0.0);
        assert!(hit.contains(from));
        assert_eq!(
            text.galley.cursor_from_pos(from - text.pos),
            text.galley.end(),
            "press at the actual last character boundary"
        );
        // A one-point upward exit at body_left+1 can still hit the Label via
        // interact_radius and select character 8, not the whitespace at index 0.
        // Exit horizontally beyond every code hit region so native upward drag
        // selection must reach Galley::begin, independent of font and hit slop.
        let right = observed(&frame, "decompile-code")
            .into_iter()
            .map(|row| bounds(row).right())
            .fold(body.right(), f32::max);
        let to = egui::pos2(right + interact_radius + 1.0, body.top() - 1.0);
        for events in [
            vec![Event::PointerMoved(from)],
            vec![Event::PointerButton {
                pos: from,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            }],
            // Establish a nonempty in-label selection before crossing out of
            // the hit region. Native upward extension uses that painted range.
            vec![Event::PointerMoved(
                body.left_center() + egui::vec2(1.0, 0.0),
            )],
            vec![Event::PointerMoved(to)],
            vec![Event::PointerButton {
                pos: to,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        ] {
            paint(&ctx, &mut views, &artifact, &mut state, size, events);
        }
        let frame = paint(
            &ctx,
            &mut views,
            &artifact,
            &mut state,
            size,
            vec![Event::Copy],
        );
        assert_native_whole_line_selection(&frame, expected, &ctx);
        assert_eq!(copied(&frame), vec!["        continue;"]);

        // Changing markers must preserve the actual native selection as well
        // as its response identity and the independent code-column geometry.
        views.differences.after.remove(&EntityId(43));
        let frame = paint(
            &ctx,
            &mut views,
            &artifact,
            &mut state,
            size,
            vec![Event::Copy],
        );
        assert_native_whole_line_selection(&frame, expected, &ctx);
        assert_eq!(copied(&frame), vec!["        continue;"]);
    }
}

fn assert_native_whole_line_selection(frame: &Frame, expected: &str, ctx: &Context) {
    let (text, _) = texts(&frame.output)
        .into_iter()
        .find(|(text, _)| text.galley.text() == expected)
        .unwrap();
    let row = &text.galley.rows[0];
    let selection_color = ctx.global_style().visuals.selection.bg_fill;
    let selected: Vec<_> = row
        .visuals
        .mesh
        .vertices
        .iter()
        .filter(|vertex| vertex.color == selection_color)
        .collect();
    // Native Label copy also copies an entire Galley for an EMPTY range. The
    // actual painted selection must therefore span the whitespace too; clipboard
    // equality by itself would let a broken drag gesture pass accidentally.
    assert_eq!(selected.len(), 4, "one real selection rectangle must paint");
    let left = selected
        .iter()
        .map(|vertex| vertex.pos.x)
        .fold(f32::INFINITY, f32::min);
    let right = selected
        .iter()
        .map(|vertex| vertex.pos.x)
        .fold(f32::NEG_INFINITY, f32::max);
    close(left, row.glyphs.first().unwrap().pos.x);
    close(right, row.glyphs.last().unwrap().max_x());
    assert!(
        left < row
            .glyphs
            .iter()
            .find(|glyph| !glyph.chr.is_whitespace())
            .unwrap()
            .pos
            .x
    );
}

#[test]
fn structured_virtual_rows_and_large_font_navigation_keep_late_glyphs_reachable() {
    let artifact = fixture::large(1500);
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    ctx.global_style_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::monospace(26.0));
    });
    let mut state = state(&artifact);
    let mut views = Views::default();
    let size = Vec2::new(480.0, 330.0);
    paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let cached = views.rendered.as_ref().unwrap().2.as_ptr();
    assert!(state.navigate(
        &artifact,
        EntityRef {
            stage: StageId(2),
            entity: EntityId(3994)
        }
    ));
    let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    assert_eq!(views.rendered.as_ref().unwrap().2.as_ptr(), cached);
    assert!(rows::prepared(&ctx) < 20, "prepare only a visible window");
    assert!(!state.entity_navigation);
    let code = observed(&frame, "decompile-code");
    let selected = code
        .iter()
        .find(|row| row["selected"] == true && row["label"].as_str().unwrap().contains("v101497 ="))
        .unwrap();
    assert!(bounds(selected).top() >= 0.0 && bounds(selected).bottom() <= size.y);
    assert!(
        observed(&frame, "decompile-code-glyphs")
            .iter()
            .any(|row| row["id"] == selected["id"])
    );
}

#[test]
fn structured_narrow_horizontal_scroll_preserves_full_long_line_glyphs_and_depth() {
    let mut artifact = nested();
    artifact.stages[2].ir.blocks[0].operations[0].replacement =
        Some(evm_abstract_protocol::decompile_model::U256::MAX);
    let root = &mut artifact.stages[2].ir.structured.as_mut().unwrap().root;
    let StructuredKind::Sequence(children) = &mut root.kind else {
        unreachable!()
    };
    let StructuredKind::While { body, .. } = &mut children[0].kind else {
        unreachable!()
    };
    let StructuredKind::If { then_branch, .. } = &mut body.kind else {
        unreachable!()
    };
    then_branch.kind = StructuredKind::Block { block: EntityId(1) };
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut state = state(&artifact);
    let mut views = Views::default();
    let size = Vec2::new(320.0, 1000.0);
    paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let before = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let code = views
        .rendered
        .as_ref()
        .unwrap()
        .2
        .iter()
        .find(|line| line.text.contains(&"f".repeat(64)))
        .expect("fixture has an unelided full U256")
        .text
        .clone();
    let locate = |frame: &Frame| {
        let (text, _) = texts(&frame.output)
            .into_iter()
            .find(|(text, _)| text.galley.text().trim_start() == code)
            .expect("long production Label exists");
        assert!(!text.galley.elided && text.galley.rows.len() == 1);
        (glyph_x(text), text.pos.x + text.galley.rect.right())
    };
    let (old_x, old_right) = locate(&before);
    assert!(old_right > size.x);
    paint(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        size,
        vec![
            Event::PointerMoved(egui::pos2(150.0, 250.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-4096.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    }
    let after = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let (new_x, new_right) = locate(&after);
    assert!(
        new_x < old_x - 10.0,
        "horizontal input must move actual glyphs"
    );
    assert!(
        new_right > 0.0 && new_right <= size.x,
        "line end must be reachable: {new_right}"
    );
    let origins = |frame: &Frame| -> BTreeMap<String, f32> {
        texts(&frame.output)
            .into_iter()
            .filter(|(text, _)| text.galley.text().trim_start() == code)
            .map(|(text, _)| {
                (
                    format!("{}:{}", text.galley.text(), text.pos.y),
                    glyph_x(text),
                )
            })
            .collect()
    };
    let before_origins = origins(&before);
    let after_origins = origins(&after);
    assert_eq!(before_origins.len(), 2, "long code exists at two depths");
    assert_eq!(
        after_origins.len(),
        2,
        "both depths stay paintable after scrolling"
    );
    for (row, x) in before_origins {
        close(after_origins[&row] - x, new_x - old_x);
    }
    let before_marker_change = new_x;
    views
        .differences
        .after
        .insert(EntityId(1000), [Marker::New, Marker::Changed].into());
    let marked = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    close(locate(&marked).0, before_marker_change);
}

#[test]
fn structured_keyboard_focus_survives_marker_count_changes_in_earlier_and_focused_rows() {
    let artifact = nested();
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut state = state(&artifact);
    let mut views = Views::default();
    let size = Vec2::new(1200.0, 1100.0);
    let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
    let target = observed(&frame, "decompile-code")
        .into_iter()
        .find(|row| row["label"] == "    continue;")
        .unwrap();
    let target_id = target["id"].as_str().unwrap().parse::<u64>().unwrap();
    let mut focused = None;
    for _ in 0..30 {
        paint(
            &ctx,
            &mut views,
            &artifact,
            &mut state,
            size,
            vec![Event::Key {
                key: egui::Key::Tab,
                physical_key: Some(egui::Key::Tab),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        if let Some(id) = ctx.memory(|memory| memory.focused())
            && target_id == id.value()
        {
            focused = Some(id);
            break;
        }
    }
    let focused = focused.expect("native keyboard traversal must reach the code Label");
    for markers in [
        vec![Marker::New],
        vec![Marker::Copied, Marker::Redirected, Marker::Changed],
        vec![],
    ] {
        for entity in [40, 42, 51] {
            views
                .differences
                .after
                .insert(EntityId(entity), markers.iter().copied().collect());
        }
        let frame = paint(&ctx, &mut views, &artifact, &mut state, size, vec![]);
        assert_eq!(ctx.memory(|memory| memory.focused()), Some(focused));
        assert!(
            observed(&frame, "decompile-code")
                .iter()
                .any(|row| row["id"].as_str().unwrap().parse::<u64>().unwrap() == focused.value())
        );
        let text = texts(&frame.output)
            .into_iter()
            .filter(|(text, _)| text.galley.text().trim_start() == "continue;")
            .nth(1)
            .unwrap()
            .0;
        assert!(
            text.underline.width > 0.0,
            "the same real Label retains its focus underline"
        );
    }
}

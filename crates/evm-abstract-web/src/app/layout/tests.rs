use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use egui_tiles::{Tile, Tree};
use evm_abstract_protocol::{Diagnostic, DiagnosticKind};

use super::{Pane, WidthClass};
use crate::{Workspace, tests::ready};

pub(super) fn frame(
    ctx: &Context,
    workspace: &mut Workspace,
    size: Vec2,
    events: Vec<Event>,
) -> FullOutput {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
        events,
        ..RawInput::default()
    };
    workspace.prepare_input(&mut input);
    let mut output = ctx.run_ui(input, |ui| {
        workspace.show(ui);
    });
    output.textures_delta.clear();
    output
}

pub(super) fn active_tree(workspace: &Workspace, width: f32) -> &Tree<Pane> {
    match WidthClass::for_width(width - 8.0) {
        WidthClass::Wide => &workspace.layout.wide,
        WidthClass::Medium => &workspace.layout.medium,
        WidthClass::Narrow => &workspace.layout.narrow,
    }
}

fn pane_rects(tree: &Tree<Pane>) -> Vec<(Pane, Rect)> {
    tree.tiles
        .iter()
        .filter_map(|(id, tile)| match tile {
            Tile::Pane(pane) if tree.is_visible_in_layout(*id) => {
                tree.tiles.rect(*id).map(|rect| (*pane, rect))
            }
            _ => None,
        })
        .collect()
}

pub(super) fn pane_rect(tree: &Tree<Pane>, target: Pane) -> Rect {
    pane_rects(tree)
        .into_iter()
        .find(|(pane, _)| *pane == target)
        .unwrap()
        .1
}

fn divider_stroke(output: &FullOutput, x: f32, pane: Rect) -> egui::Stroke {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::LineSegment { points, stroke }
                if (points[0].x - x).abs() < 1.0
                    && (points[1].x - x).abs() < 1.0
                    && points[0].y <= pane.top() + 1.0
                    && points[1].y >= pane.bottom() - 1.0
                    && shape.clip_rect.contains(points[0])
                    && shape.clip_rect.contains(points[1]) =>
            {
                Some(*stroke)
            }
            _ => None,
        })
        .expect("the divider must be painted across the visible pane")
}

pub(super) fn settled(ctx: &Context, workspace: &mut Workspace, size: Vec2) -> FullOutput {
    frame(ctx, workspace, size, vec![]);
    frame(ctx, workspace, size, vec![]);
    frame(ctx, workspace, size, vec![])
}

pub(super) fn click(ctx: &Context, workspace: &mut Workspace, size: Vec2, pos: Pos2) {
    for pressed in [true, false] {
        frame(
            ctx,
            workspace,
            size,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
    }
}

pub(super) fn texts(output: &FullOutput) -> Vec<(String, Rect)> {
    fn collect(shape: &egui::Shape, result: &mut Vec<(String, Rect)>) {
        match shape {
            egui::Shape::Text(text) => result.push((
                text.galley.text().into(),
                Rect::from_min_size(text.pos, text.galley.size()),
            )),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, result);
                }
            }
            _ => {}
        }
    }
    let mut result = vec![];
    for shape in &output.shapes {
        collect(&shape.shape, &mut result);
    }
    result
}

#[test]
fn resizing_reflows_all_panes_without_resetting_workspace_mode() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    for (width, height, pane_count) in [
        (1920.0, 1080.0, 3),
        (1440.0, 900.0, 3),
        (1024.0, 768.0, 2),
        (768.0, 600.0, 2),
        (390.0, 844.0, 1),
        (844.0, 390.0, 2),
        (320.0, 240.0, 1),
        (1440.0, 900.0, 3),
    ] {
        let size = Vec2::new(width, height);
        settled(&ctx, &mut workspace, size);
        let rects = pane_rects(active_tree(&workspace, width));
        assert_eq!(rects.len(), pane_count, "{size:?}: {rects:?}");
        for (_, rect) in rects {
            assert!(
                Rect::from_min_size(Pos2::ZERO, size).contains_rect(rect),
                "pane outside viewport: {size:?}: {rect:?}"
            );
            assert!(
                rect.height() >= if height >= 600.0 { height * 0.45 } else { 44.0 },
                "chrome consumed analysis viewport: {size:?}: {rect:?}"
            );
        }
        assert_eq!(workspace.view, crate::app::View::Split);
    }
}

#[test]
fn compact_tabs_keep_every_analysis_view_accessible() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(390.0, 844.0);
    for (label, pane) in [
        ("Control flow", Pane::Graph),
        ("SSA", Pane::Ssa),
        ("Disassembly", Pane::Disassembly),
    ] {
        let output = settled(&ctx, &mut workspace, size);
        let target = texts(&output)
            .into_iter()
            .find(|(text, rect)| text == label && rect.top() > 35.0)
            .unwrap()
            .1;
        click(&ctx, &mut workspace, size, target.center());
        settled(&ctx, &mut workspace, size);
        assert_eq!(pane_rects(&workspace.layout.narrow)[0].0, pane);
    }
    let medium = Vec2::new(844.0, 390.0);
    let output = settled(&ctx, &mut workspace, medium);
    let target = texts(&output)
        .into_iter()
        .find(|(text, _)| text == "SSA")
        .unwrap()
        .1;
    // The header also offers SSA; the pane tab is below the input editor.
    let target = texts(&output)
        .into_iter()
        .find(|(text, rect)| text == "SSA" && rect.top() > target.top())
        .unwrap()
        .1;
    click(&ctx, &mut workspace, medium, target.center());
    settled(&ctx, &mut workspace, medium);
    let panes = pane_rects(&workspace.layout.medium);
    assert!(panes.iter().any(|(pane, _)| *pane == Pane::Graph));
    assert!(panes.iter().any(|(pane, _)| *pane == Pane::Ssa));
}

#[test]
fn dragging_a_divider_survives_repeated_resizes_and_width_classes() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1440.0, 900.0);
    for viewport in [size, Vec2::new(1024.0, 768.0), size] {
        let output = settled(&ctx, &mut workspace, viewport);
        let mut panes = pane_rects(active_tree(&workspace, viewport.x));
        panes.sort_by(|left, right| left.1.left().total_cmp(&right.1.left()));
        for adjacent in panes.windows(2) {
            let x = (adjacent[0].1.right() + adjacent[1].1.left()) * 0.5;
            let stroke = divider_stroke(&output, x, adjacent[0].1);
            assert!(stroke.width > 0.0 && stroke.color.a() > 0);
            assert_ne!(
                stroke.color,
                crate::palette::BACKGROUND,
                "idle pane divider disappears into the background"
            );
        }
    }
    let original = pane_rect(&workspace.layout.wide, Pane::Disassembly);
    let divider = Pos2::new(original.right() + 2.5, original.center().y);
    let idle = divider_stroke(&settled(&ctx, &mut workspace, size), divider.x, original);
    let hovered = frame(
        &ctx,
        &mut workspace,
        size,
        vec![Event::PointerMoved(divider)],
    );
    let hovered = divider_stroke(&hovered, divider.x, original);
    assert!(hovered.width > idle.width);
    assert_ne!(hovered.color, idle.color);
    frame(
        &ctx,
        &mut workspace,
        size,
        vec![Event::PointerButton {
            pos: divider,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    let moved = divider + Vec2::new(90.0, 0.0);
    let dragging = frame(&ctx, &mut workspace, size, vec![Event::PointerMoved(moved)]);
    let dragging = divider_stroke(&dragging, divider.x, original);
    assert_ne!(dragging.color, hovered.color);
    frame(
        &ctx,
        &mut workspace,
        size,
        vec![Event::PointerButton {
            pos: moved,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    settled(&ctx, &mut workspace, size);
    let adjusted = pane_rect(&workspace.layout.wide, Pane::Disassembly);
    assert!(
        adjusted.width() > original.width() + 50.0,
        "divider did not resize: {original:?} -> {adjusted:?}"
    );
    for resized in [
        Vec2::new(1920.0, 1080.0),
        Vec2::new(1024.0, 768.0),
        Vec2::new(390.0, 844.0),
        size,
    ] {
        settled(&ctx, &mut workspace, resized);
    }
    let restored = pane_rect(&workspace.layout.wide, Pane::Disassembly);
    assert!(
        (restored.width() - adjusted.width()).abs() <= 1.0,
        "divider preference lost: {adjusted:?} -> {restored:?}"
    );
}

#[test]
fn long_input_and_diagnostics_stay_bounded_in_a_short_viewport() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    workspace.form.bytecode.bytecode = "60ff\n".repeat(500);
    workspace.inspector.expanded = true;
    workspace.inspector.tab = crate::app::inspector::Tab::Diagnostics;
    workspace.report.as_mut().unwrap().diagnostics = (0..100)
        .map(|index| Diagnostic {
            state: 0,
            pc: index,
            kind: DiagnosticKind::OpaqueResult,
            reduction: None,
            detail: "A detailed diagnostic must remain independently scrollable. ".repeat(20),
        })
        .collect();
    for size in [Vec2::new(844.0, 390.0), Vec2::new(390.0, 300.0)] {
        let output = settled(&ctx, &mut workspace, size);
        assert!(
            output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text().contains("OpaqueResult") => {
                    shape
                        .clip_rect
                        .intersect(Rect::from_min_size(text.pos, text.galley.size()))
                        .height()
                        >= 12.0
                }
                _ => false,
            }),
            "expanded diagnostics are allocated but not visible at {size:?}"
        );
        for (_, rect) in pane_rects(active_tree(&workspace, size.x)) {
            assert!(
                rect.height() > size.y * 0.28,
                "input/details overflowed: {size:?}: {rect:?}"
            );
            assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(rect));
        }
    }
}

#[test]
fn modal_editor_is_visible_and_bounded_without_global_font_scaling() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    workspace.form.open = true;
    for size in [
        Vec2::new(1440.0, 900.0),
        Vec2::new(844.0, 390.0),
        Vec2::new(390.0, 844.0),
        Vec2::new(320.0, 240.0),
    ] {
        let output = settled(&ctx, &mut workspace, size);
        let response = ctx
            .read_response(egui::Id::new("runtime_bytecode"))
            .unwrap();
        let viewport = Rect::from_min_size(Pos2::ZERO, size);
        assert!(
            response.interact_rect.height() >= 16.0,
            "editor clipped at {size:?}: {:?}",
            response.interact_rect
        );
        assert!(viewport.contains_rect(response.interact_rect));
        for (text, rect) in texts(&output) {
            if text == "Analyze" || text == "Close" {
                assert!(
                    viewport.contains_rect(rect),
                    "modal action clipped at {size:?}: {text} {rect:?}"
                );
            }
        }
    }
    assert_eq!(ctx.zoom_factor(), 1.0);
    assert_eq!(
        ctx.global_style().text_styles,
        egui::Style::default().text_styles
    );
}

#[test]
fn visible_bytecode_editor_accepts_pointer_and_keyboard_input_after_resizes() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    workspace.form.open = true;
    for size in [
        Vec2::new(1440.0, 900.0),
        Vec2::new(390.0, 844.0),
        Vec2::new(844.0, 390.0),
    ] {
        settled(&ctx, &mut workspace, size);
        let editor = ctx
            .read_response(egui::Id::new("runtime_bytecode"))
            .unwrap();
        assert!(editor.interact_rect.height() >= 16.0);
        click(
            &ctx,
            &mut workspace,
            size,
            editor.interact_rect.left_center() + Vec2::new(12.0, 0.0),
        );
        let command = Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        };
        frame(
            &ctx,
            &mut workspace,
            size,
            vec![Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: command,
            }],
        );
        let input = format!("0x{:04x}", size.x as usize);
        frame(&ctx, &mut workspace, size, vec![Event::Text(input.clone())]);
        assert_eq!(
            workspace.form.bytecode.bytecode, input,
            "visible editor did not receive input at {size:?}"
        );
    }
}

fn press_f(
    ctx: &Context,
    workspace: &mut Workspace,
    size: Vec2,
    modifiers: Modifiers,
    text: Option<&str>,
) -> FullOutput {
    let mut events = vec![Event::Key {
        key: egui::Key::F,
        physical_key: Some(egui::Key::F),
        pressed: true,
        repeat: false,
        modifiers,
    }];
    if let Some(text) = text {
        events.push(Event::Text(text.into()));
    }
    frame(ctx, workspace, size, events);
    frame(
        ctx,
        workspace,
        size,
        vec![Event::Key {
            key: egui::Key::F,
            physical_key: Some(egui::Key::F),
            pressed: false,
            repeat: false,
            modifiers,
        }],
    );
    settled(ctx, workspace, size)
}

fn graph_titles(output: &FullOutput) -> Vec<(String, Rect)> {
    texts(output)
        .into_iter()
        .filter(|(label, _)| label.starts_with('S') && label.contains("  ·  B"))
        .collect()
}

fn has_camera_label(output: &FullOutput, zoom: f32, mode: &str) -> bool {
    let label = format!("{:.0}% · {mode}", zoom * 100.0);
    texts(output).iter().any(|(text, _)| *text == label)
}

#[test]
fn f_and_shift_f_restore_the_visible_graph_fit_and_node_positions() {
    let size = Vec2::new(1440.0, 900.0);
    for view in [crate::app::View::Split, crate::app::View::Graph] {
        for (modifiers, character) in [(Modifiers::NONE, "f"), (Modifiers::SHIFT, "F")] {
            let ctx = Context::default();
            crate::palette::configure(&ctx);
            let mut workspace = ready();
            workspace.view = view;
            let fitted = settled(&ctx, &mut workspace, size);
            let fitted_zoom = workspace.graph.zoom;
            assert!(has_camera_label(&fitted, fitted_zoom, "Fit"));
            let fitted_titles = graph_titles(&fitted);
            assert_eq!(fitted_titles.len(), 3);
            workspace.graph.zoom_at(Vec2::new(37.0, 89.0), 0.5);
            let manual = settled(&ctx, &mut workspace, size);
            let manual_zoom = workspace.graph.zoom;
            assert!(manual_zoom < fitted_zoom);
            assert!(has_camera_label(&manual, manual_zoom, "Manual"));
            assert_ne!(graph_titles(&manual), fitted_titles);

            let restored = press_f(&ctx, &mut workspace, size, modifiers, Some(character));
            assert_eq!(workspace.graph.zoom, fitted_zoom, "{view:?}, {character}");
            assert!(has_camera_label(&restored, fitted_zoom, "Fit"));
            assert_eq!(
                graph_titles(&restored),
                fitted_titles,
                "shortcut must restore the rendered camera, not only its mode label"
            );
        }
    }
}

#[test]
fn typing_f_and_shift_f_in_the_focused_editor_keeps_the_manual_camera() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    workspace.form.bytecode.bytecode.clear();
    let size = Vec2::new(1440.0, 900.0);
    settled(&ctx, &mut workspace, size);
    workspace.graph.zoom_at(Vec2::new(37.0, 89.0), 0.5);
    settled(&ctx, &mut workspace, size);
    let manual_zoom = workspace.graph.zoom;
    workspace.form.open = true;
    settled(&ctx, &mut workspace, size);
    let editor = ctx
        .read_response(egui::Id::new("runtime_bytecode"))
        .unwrap();
    click(
        &ctx,
        &mut workspace,
        size,
        editor.interact_rect.left_center() + Vec2::new(10.0, 0.0),
    );
    assert!(
        ctx.text_edit_focused(),
        "exercise the actual TextEdit focus path"
    );
    let original_camera = graph_titles(&settled(&ctx, &mut workspace, size));
    for (modifiers, character, expected) in
        [(Modifiers::NONE, "f", "f"), (Modifiers::SHIFT, "F", "fF")]
    {
        let output = press_f(&ctx, &mut workspace, size, modifiers, Some(character));
        assert_eq!(
            workspace.form.bytecode.bytecode, expected,
            "text input must retain the typed letter"
        );
        assert!(ctx.text_edit_focused());
        assert_eq!(workspace.graph.zoom, manual_zoom);
        assert!(has_camera_label(&output, manual_zoom, "Manual"));
        assert_eq!(graph_titles(&output), original_camera);
    }
}

#[test]
fn modified_find_shortcuts_do_not_reset_the_graph_camera() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1440.0, 900.0);
    settled(&ctx, &mut workspace, size);
    workspace.graph.zoom_at(Vec2::new(37.0, 89.0), 0.5);
    let original_camera = graph_titles(&settled(&ctx, &mut workspace, size));
    let manual_zoom = workspace.graph.zoom;
    for modifiers in [
        Modifiers::CTRL,
        Modifiers::ALT,
        Modifiers {
            command: true,
            ..Modifiers::NONE
        },
        Modifiers {
            mac_cmd: true,
            ..Modifiers::NONE
        },
        Modifiers {
            shift: true,
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        },
    ] {
        let output = press_f(&ctx, &mut workspace, size, modifiers, None);
        assert_eq!(workspace.graph.zoom, manual_zoom, "{modifiers:?}");
        assert!(has_camera_label(&output, manual_zoom, "Manual"));
        assert_eq!(graph_titles(&output), original_camera);
    }
}

#[test]
fn fit_shortcut_does_not_reach_a_hidden_graph_pane() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let wide = Vec2::new(1440.0, 900.0);
    settled(&ctx, &mut workspace, wide);
    workspace.graph.zoom_at(Vec2::new(37.0, 89.0), 0.5);
    let manual_zoom = workspace.graph.zoom;
    for (view, size) in [
        (crate::app::View::Disassembly, wide),
        (crate::app::View::Ssa, wide),
        (crate::app::View::Split, Vec2::new(390.0, 844.0)),
    ] {
        workspace.view = view;
        let before = settled(&ctx, &mut workspace, size);
        assert!(graph_titles(&before).is_empty());
        press_f(&ctx, &mut workspace, size, Modifiers::NONE, Some("f"));
        assert_eq!(
            workspace.graph.zoom, manual_zoom,
            "hidden graph must ignore F: {view:?}, {size:?}"
        );
    }
}

#[test]
fn scrolling_the_modal_editor_does_not_navigate_the_background_graph() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1440.0, 900.0);
    settled(&ctx, &mut workspace, size);
    workspace.graph.zoom_at(Vec2::new(80.0, 90.0), 0.75);
    workspace.form.bytecode.bytecode = "6001\n".repeat(300);
    workspace.form.open = true;
    let before = settled(&ctx, &mut workspace, size);
    let titles = graph_titles(&before);
    let zoom = workspace.graph.zoom;
    let editor = ctx
        .read_response(egui::Id::new("runtime_bytecode"))
        .unwrap();
    let top = editor.rect.top();
    frame(
        &ctx,
        &mut workspace,
        size,
        vec![
            Event::PointerMoved(editor.interact_rect.center()),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -180.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        frame(&ctx, &mut workspace, size, vec![]);
    }
    let after = settled(&ctx, &mut workspace, size);
    assert!(
        ctx.read_response(egui::Id::new("runtime_bytecode"))
            .unwrap()
            .rect
            .top()
            < top - 50.0,
        "wheel must reach the modal's scrollable editor"
    );
    assert_eq!(graph_titles(&after), titles);
    assert_eq!(workspace.graph.zoom, zoom);
}

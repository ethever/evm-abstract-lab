use egui::{Context, Event, FullOutput, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use egui_tiles::{Tile, Tree};
use evm_abstract_protocol::{Diagnostic, DiagnosticKind};

use super::{Pane, WidthClass};
use crate::{Workspace, tests::ready};

fn frame(ctx: &Context, workspace: &mut Workspace, size: Vec2, events: Vec<Event>) -> FullOutput {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            events,
            ..RawInput::default()
        },
        |ui| {
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    output
}

fn active_tree(workspace: &Workspace, width: f32) -> &Tree<Pane> {
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

fn pane_rect(tree: &Tree<Pane>, target: Pane) -> Rect {
    pane_rects(tree)
        .into_iter()
        .find(|(pane, _)| *pane == target)
        .unwrap()
        .1
}

fn settled(ctx: &Context, workspace: &mut Workspace, size: Vec2) -> FullOutput {
    frame(ctx, workspace, size, vec![]);
    frame(ctx, workspace, size, vec![]);
    frame(ctx, workspace, size, vec![])
}

fn click(ctx: &Context, workspace: &mut Workspace, size: Vec2, pos: Pos2) {
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

fn texts(output: &FullOutput) -> Vec<(String, Rect)> {
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
                rect.height() >= height * 0.45,
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
            .find(|(text, rect)| text == label && rect.top() > 60.0)
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
    settled(&ctx, &mut workspace, size);
    let original = pane_rect(&workspace.layout.wide, Pane::Disassembly);
    let divider = Pos2::new(original.right() + 2.5, original.center().y);
    frame(
        &ctx,
        &mut workspace,
        size,
        vec![Event::PointerMoved(divider)],
    );
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
    frame(&ctx, &mut workspace, size, vec![Event::PointerMoved(moved)]);
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
    workspace.request.bytecode = "60ff\n".repeat(500);
    workspace.show_details = true;
    workspace.report.as_mut().unwrap().diagnostics = (0..100)
        .map(|index| Diagnostic {
            state: 0,
            pc: index,
            kind: DiagnosticKind::OpaqueResult,
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
                rect.height() > size.y * 0.33,
                "input/details overflowed: {size:?}: {rect:?}"
            );
            assert!(Rect::from_min_size(Pos2::ZERO, size).contains_rect(rect));
        }
    }
}

#[test]
fn navigation_and_editor_fit_without_global_font_scaling() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
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
        let editor = response.rect;
        assert!(
            response.interact_rect.height() >= 16.0,
            "editor is allocated but not visibly interactive: {:?}",
            response.interact_rect,
        );
        assert!(
            editor.top() < 65.0,
            "editor has excess chrome above it: {editor:?}"
        );
        assert!(
            editor.bottom() < 90.0,
            "editor exceeds compact input budget: {editor:?}"
        );
        let viewport = Rect::from_min_size(Pos2::ZERO, size);
        for (text, rect) in texts(&output) {
            if ["Bytecode", "Workspace", "Examples", "Limits", "▶ Analyze"].contains(&text.as_str())
            {
                assert!(
                    viewport.contains_rect(rect),
                    "navigation clipped: {size:?}, {text}: {rect:?}"
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
            workspace.request.bytecode, input,
            "visible editor did not receive input at {size:?}"
        );
    }
}

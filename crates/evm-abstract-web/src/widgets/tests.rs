//! Source widgets move between fullscreen, responsive panes and tabs while
//! retaining one logical selection and the user's independent scroll position.

use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::AnalyzeReply;

use crate::{
    Workspace,
    app::{Selection, View},
};

fn workspace() -> Workspace {
    let mut report = crate::tests::report();
    report.cfg.truncate(1);
    report.disassembly.truncate(1);
    report.ssa.blocks.truncate(1);
    report.edges.clear();
    let immediate = format!("0x{}", "fe".repeat(256));
    let source = report.disassembly[0].instructions[0].clone();
    report.disassembly[0].instructions = (0..1000)
        .map(|pc| {
            let mut instruction = source.clone();
            instruction.pc = pc;
            instruction.immediate = Some(immediate.clone());
            instruction
        })
        .collect();
    let instruction = report.ssa.blocks[0].instructions[0].clone();
    report.ssa.blocks[0].instructions = (0..1000)
        .map(|pc| {
            let mut instruction = instruction.clone();
            instruction.pc = pc;
            instruction.immediate = Some(immediate.clone());
            instruction
        })
        .collect();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    workspace
}

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

fn settle(ctx: &Context, workspace: &mut Workspace, size: Vec2) -> FullOutput {
    for _ in 0..40 {
        frame(ctx, workspace, size, Vec::new());
    }
    frame(ctx, workspace, size, Vec::new())
}

fn text_position(output: &FullOutput, expected: &str) -> Option<Pos2> {
    output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == expected => Some(text.pos),
            _ => None,
        })
}

fn first_pc(output: &FullOutput, view: View) -> (usize, Pos2) {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => {
                let label = text.galley.text();
                let pc = match view {
                    View::Ssa if label.len() == 6 && label.ends_with("  ") => &label[..4],
                    View::Disassembly if label.len() == 4 => label,
                    _ => return None,
                };
                usize::from_str_radix(pc, 16).ok().map(|pc| (pc, text.pos))
            }
            _ => None,
        })
        .min_by_key(|(pc, _)| *pc)
        .expect("a visible source row")
}

#[test]
fn manual_source_scroll_survives_view_parent_changes_and_resizing() {
    for view in [View::Disassembly, View::Ssa] {
        let ctx = Context::default();
        let mut workspace = workspace();
        workspace.view = view;
        let wide = Vec2::new(1440.0, 900.0);
        settle(&ctx, &mut workspace, wide);
        frame(
            &ctx,
            &mut workspace,
            wide,
            vec![
                Event::PointerMoved(Pos2::new(600.0, 400.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(-180.0, -3000.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let before = settle(&ctx, &mut workspace, wide);
        let (pc, pos) = first_pc(&before, view);
        assert!(pc > 100, "fixture must scroll away from the first row");
        let title = if view == View::Ssa {
            "SSA"
        } else {
            "DISASSEMBLY"
        };
        let origin = text_position(&before, title).unwrap().x;
        let offset = pos.x - origin;
        assert!(offset < -100.0, "fixture must also scroll horizontally");
        workspace.view = View::Split;
        let split = settle(&ctx, &mut workspace, wide);
        let (split_pc, split_pos) = first_pc(&split, view);
        let split_origin = text_position(&split, title).unwrap().x;
        assert_eq!(
            split_pc, pc,
            "{view:?} lost its manually scrolled row on reparenting"
        );
        assert!(
            (split_pos.x - split_origin - offset).abs() < 1.0,
            "{view:?} lost horizontal scroll on reparenting"
        );
        for width in [844.0, 390.0, 1440.0] {
            let size = Vec2::new(width, 900.0);
            let mut output = settle(&ctx, &mut workspace, size);
            if view == View::Ssa && width < 1120.0 {
                // Medium/narrow arrangements initially display Disassembly.
                // Activate the real SSA tab, with an actual pointer click.
                let pos = text_position(&output, "SSA").unwrap() + Vec2::splat(4.0);
                for pressed in [true, false] {
                    frame(
                        &ctx,
                        &mut workspace,
                        size,
                        vec![
                            Event::PointerMoved(pos),
                            Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: egui::Modifiers::NONE,
                            },
                        ],
                    );
                }
                output = settle(&ctx, &mut workspace, size);
            }
            let (next_pc, next_pos) = first_pc(&output, view);
            let next_origin = text_position(&output, title).unwrap().x;
            assert_eq!(next_pc, pc, "{view:?} lost its row at width {width}");
            assert!(
                (next_pos.x - next_origin - offset).abs() < 1.0,
                "{view:?} lost horizontal scroll at width {width}"
            );
        }
        workspace.view = view;
        let resized = settle(&ctx, &mut workspace, Vec2::new(720.0, 900.0));
        let (resized_pc, resized_pos) = first_pc(&resized, view);
        let resized_origin = text_position(&resized, title).unwrap().x;
        assert_eq!(resized_pc, pc);
        assert!((resized_pos.x - resized_origin - offset).abs() < 1.0);
    }
}

#[test]
fn selected_offscreen_source_row_remains_visible_when_moved_to_workspace() {
    for view in [View::Disassembly, View::Ssa] {
        let ctx = Context::default();
        let mut workspace = workspace();
        workspace.view = view;
        workspace.selection = Selection {
            state: Some(0),
            pc: Some(999),
        };
        let size = Vec2::new(1440.0, 900.0);
        let label = if view == View::Ssa { "03e7  " } else { "03e7" };
        let before = settle(&ctx, &mut workspace, size);
        assert!(text_position(&before, label).is_some());
        workspace.view = View::Split;
        let after = settle(&ctx, &mut workspace, size);
        assert!(
            text_position(&after, label).is_some(),
            "{view:?} lost the selected row when moved to a tile"
        );
    }
}

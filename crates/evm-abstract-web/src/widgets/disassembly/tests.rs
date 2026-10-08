use egui::{Context, Event, FullOutput, Pos2, RawInput, Rect, Vec2};

use super::{content_columns, disassembly, rows};
use crate::app::Selection;

#[test]
fn source_columns_size_to_current_data_including_full_push_values() {
    let mut report = crate::tests::report();
    let selection = Selection {
        state: Some(0),
        pc: None,
    };
    let ordinary = content_columns(&rows(&report, selection));
    assert_eq!(ordinary, [4, 15, 6]);
    report.disassembly[0].instructions[0].name = "PUSH32".into();
    report.disassembly[0].instructions[0].immediate = Some(format!("0x{}", "fe".repeat(32)));
    let full_push = content_columns(&rows(&report, selection));
    assert_eq!(full_push[1], 6 + 1 + 66);
    // The target is the native state, even when its basic-block ID is different.
    report.cfg[0].id = 91;
    let rows = rows(
        &report,
        Selection {
            state: Some(91),
            pc: None,
        },
    );
    assert_eq!(
        rows[1].target(),
        Selection {
            state: Some(91),
            pc: Some(0)
        }
    );
}

fn push_bounds(output: &FullOutput, immediate: &str) -> Rect {
    output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == format!(" {immediate}") => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("the complete PUSH operand must be painted without truncation")
}

#[test]
fn long_push_is_complete_and_horizontally_reachable_in_a_narrow_pane() {
    let mut report = crate::tests::report();
    let immediate = format!("0x{}", "fe".repeat(32));
    report.disassembly[0].instructions[0].name = "PUSH32".into();
    report.disassembly[0].instructions[0].immediate = Some(immediate.clone());
    let ctx = Context::default();
    let mut selection = Selection {
        state: Some(0),
        pc: None,
    };
    let mut previous = selection;
    let mut frame = |events| {
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 350.0))),
                events,
                ..RawInput::default()
            },
            |ui| disassembly(ui, &report, &mut selection, &mut previous),
        );
        output.textures_delta.clear();
        output
    };
    frame(Vec::new());
    let output = frame(Vec::new());
    assert!(push_bounds(&output, &immediate).right() > 300.0);
    frame(vec![
        Event::PointerMoved(Pos2::new(150.0, 150.0)),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(-1000.0, 0.0),
            phase: egui::TouchPhase::Move,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    for _ in 0..30 {
        frame(Vec::new());
    }
    let output = frame(Vec::new());
    assert!((0.0..300.0).contains(&push_bounds(&output, &immediate).right()));
}

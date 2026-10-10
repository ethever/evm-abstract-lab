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
    let ordinary = content_columns(&rows(&report, Some(0), selection));
    assert_eq!(ordinary, [4, 15]);
    report.programs[0].blocks[0].instructions[0].name = "PUSH32".into();
    report.programs[0].blocks[0].instructions[0].immediate = Some(format!("0x{}", "fe".repeat(32)));
    let full_push = content_columns(&rows(&report, Some(0), selection));
    assert_eq!(full_push[1], 6 + 1 + 66);
    // The target is the native state, even when its basic-block ID is different.
    report.cfg[0].id = 91;
    let rows = rows(
        &report,
        Some(0),
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
    report.programs[0].blocks[0].instructions[0].name = "PUSH32".into();
    report.programs[0].blocks[0].instructions[0].immediate = Some(immediate.clone());
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
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
            |ui| disassembly(ui, &report, Some(0), &mut selection, &mut previous),
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

#[test]
fn stack_signatures_and_explanations_appear_only_on_instruction_hover() {
    for (opcode, name, inputs, outputs, explanation) in [
        (0x60, "PUSH1", 0, 1, "Push the immediate value"),
        (0x01, "ADD", 2, 1, "Pop two values and push their sum"),
        (0x50, "POP", 1, 0, "Pop and discard the top value"),
        (0x80, "DUP1", 1, 2, "Copy stack position 1 (1 = top)"),
        (
            0x90,
            "SWAP1",
            2,
            2,
            "Exchange the top with stack position 2",
        ),
        (0x8f, "DUP16", 16, 17, "Copy stack position 16 (1 = top)"),
        (
            0x9f,
            "SWAP16",
            17,
            17,
            "Exchange the top with stack position 17",
        ),
    ] {
        let mut report = crate::tests::report();
        let observed = opcode == 0x01;
        if !observed {
            report.cfg[0].executed_pcs.clear();
        }
        let instruction = &mut report.programs[0].blocks[0].instructions[0];
        instruction.opcode = opcode;
        instruction.name = name.into();
        instruction.stack_inputs = inputs;
        instruction.stack_outputs = outputs;
        instruction.valid = true;
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        ctx.style_mut_of(egui::Theme::Dark, |style| {
            style.interaction.tooltip_delay = 0.0
        });
        let mut selection = Selection {
            state: None,
            pc: None,
        };
        let mut previous = selection;
        let mut frame = |time, events| {
            let mut output = ctx.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 500.0))),
                    time: Some(time),
                    events,
                    ..RawInput::default()
                },
                |ui| disassembly(ui, &report, Some(0), &mut selection, &mut previous),
            );
            output.textures_delta.clear();
            output
        };
        frame(0.0, Vec::new());
        let idle = frame(0.1, Vec::new());
        let signature = format!("{inputs} → {outputs}");
        let labels = painted_text(&idle);
        assert!(!labels.contains("IN→OUT") && !labels.contains(&signature));
        let position = idle
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == name => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("visible instruction row");
        frame(0.2, vec![Event::PointerMoved(position)]);
        frame(1.0, Vec::new());
        let hover = painted_text(&frame(1.1, Vec::new()));
        assert!(hover.contains(&signature), "{name}: {hover}");
        assert!(hover.contains(explanation), "{name}: {hover}");
        assert!(hover.contains("not the total stack heights"));
        if observed {
            // Root source rows can use a current receipt from an unselected
            // state. The hover must not attribute it to the selected state.
            assert!(hover.contains("Observed in current execution evidence"));
            assert!(!hover.contains("selected state"));
        } else {
            assert!(hover.contains("Decoded source; no current execution receipt"));
        }
    }
}

fn painted_text(output: &FullOutput) -> String {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn invalid_and_deferred_instructions_do_not_claim_observed_results() {
    let mut instruction = crate::tests::report().disassembly[0].instructions[0].clone();
    instruction.opcode = 0xfe;
    instruction.valid = true;
    let invalid = super::tooltip::instruction(&instruction, false);
    assert!(invalid.contains("exceptional halt"));
    assert!(!invalid.contains("Stack signature"));
    instruction.opcode = 0x5f;
    instruction.valid = false;
    assert!(super::tooltip::instruction(&instruction, false).contains("exceptional halt"));
    instruction.valid = true;
    for opcode in [0xf0, 0xf1, 0xf2, 0xf4, 0xf5, 0xfa] {
        instruction.opcode = opcode;
        let hover = super::tooltip::instruction(&instruction, false);
        assert!(hover.contains("if the caller resumes"));
        assert!(hover.contains("incomplete analysis may not have this result yet"));
    }
}

#[test]
fn selected_child_program_keeps_its_complete_source_and_native_state_identity() {
    let mut report = crate::tests::report();
    let mut child = report.cfg[0].clone();
    child.id = 83;
    child.program = Some(1);
    child.frame_depth = 2;
    child.code_address = "0x2222222222222222222222222222222222222222".into();
    // DELEGATECALL-style storage identity is independent of executable source.
    child.instructions[0].name = "CALLDATASIZE".into();
    child.instructions[0].opcode = 0x36;
    let first = evm_abstract_protocol::DisasmBlock {
        id: 0,
        start_pc: 0,
        instructions: child.instructions.clone(),
    };
    let mut unexecuted = first.clone();
    unexecuted.id = 9;
    unexecuted.start_pc = 88;
    unexecuted.instructions[0].pc = 88;
    unexecuted.instructions[0].name = "RETURN".into();
    unexecuted.instructions[0].opcode = 0xf3;
    report.programs.push(crate::tests::source_program(
        1,
        child.code_address.clone(),
        vec![first, unexecuted],
    ));
    report.cfg.push(child);
    let mut receipt = report.ssa.blocks[0].clone();
    receipt.state = 83;
    report.ssa.blocks.push(receipt);
    let rows = rows(
        &report,
        Some(1),
        Selection {
            state: Some(83),
            pc: Some(0),
        },
    );
    assert_eq!(
        rows.len(),
        4,
        "the full child source includes an unexecuted block"
    );
    assert_eq!(
        rows[1].target(),
        Selection {
            state: Some(83),
            pc: Some(0)
        }
    );
    assert!(
        matches!(&rows[1], super::Row::Instruction { instruction, executed: true, .. } if instruction.name == "CALLDATASIZE")
    );
    assert!(
        matches!(&rows[3], super::Row::Instruction { instruction, state: None, executed: false, .. } if instruction.name == "RETURN" && instruction.pc == 88)
    );
    assert!(rows.iter().all(|row| !matches!(row, super::Row::Instruction { instruction, .. } if instruction.name == "CALLDATALOAD")), "identical PCs in the root must never leak into child source");
}

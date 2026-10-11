use egui::{Context, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, BlockCoverage, CfgEdge, EdgeKind, EffectInput,
    Fork, InstructionProgress, Phi, PhiInput, SCHEMA_VERSION, SsaBlock, SsaInstruction, SsaReport,
    SsaTransition,
};

use super::{Row, rows, scoped_rows, ssa};
use crate::{app::Selection, ssa_display::SsaDisplay};

fn report() -> AnalysisReport {
    let mut report = AnalysisReport {
        schema_version: SCHEMA_VERSION,
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 1,
        status: AnalysisStatus::Converged,
        transfers: 1,
        disassembly: Vec::new(),
        cfg: vec![{
            let mut state = crate::tests::report().cfg[0].clone();
            state.id = 27;
            state.program = Some(0);
            state
        }],
        edges: vec![CfgEdge {
            id: 88,
            from: 27,
            to: 91,
            kind: EdgeKind::Jump,
        }],
        ssa: SsaReport {
            complete: true,
            value_count: 5,
            values: vec![],
            effect_count: 3,
            blocks: vec![SsaBlock {
                state: 27,
                coverage: BlockCoverage::Current,
                incoming_complete: true,
                phis: vec![Phi {
                    result: 11,
                    frame: 0,
                    slot: 0,
                    inputs: vec![PhiInput {
                        edge: 77,
                        predecessor: 8,
                        value: 10,
                    }],
                }],
                instructions: vec![SsaInstruction {
                    operation: crate::tests::opcode_operation(1, "ADD", &[11, 12], None),
                    pc: 400,
                    opcode: 1,
                    name: "ADD".into(),
                    immediate: None,
                    operands: vec![11, 12],
                    results: vec![13],
                    fault: false,
                    progress: InstructionProgress::Completed,
                    effect_input: 2,
                    effect_result: Some(3),
                }],
                exit_frames: vec![vec![13]],
                effect: 2,
                effect_inputs: vec![EffectInput {
                    edge: 77,
                    effect: 1,
                }],
                exit_effect: 3,
                open_incoming: Vec::new(),
            }],
            transitions: vec![SsaTransition {
                edge: 88,
                kind: EdgeKind::Jump,
                operands: vec![13],
                stacks: vec![vec![13, 14]],
                effect_input: 3,
                effect_result: 4,
                result: Some(14),
            }],
            deferred_edges: Vec::new(),
        },
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        ..crate::tests::empty_report()
    };
    report.cfg[0].start_pc = Some(400);
    report.cfg[0].instructions[0].pc = 400;
    report.cfg[0].instructions[0].name = "ADD".into();
    report.cfg[0].instructions[0].opcode = 1;
    report.cfg[0].instructions[0].stack_inputs = 2;
    report.cfg[0].instructions[0].stack_outputs = 1;
    report.disassembly = vec![evm_abstract_protocol::DisasmBlock {
        id: 0,
        start_pc: 400,
        instructions: report.cfg[0].instructions.clone(),
    }];
    report.programs.push(crate::tests::source_program(
        0,
        report.cfg[0].code_address.clone(),
        report.disassembly.clone(),
    ));
    report.metadata.root_program = Some(0);
    report
}

#[test]
fn percent_names_cover_definitions_phi_exits_and_transition_tooltips() {
    let report = report();
    let rows = rows(&report);
    let visible = rows
        .iter()
        .map(Row::plain_text)
        .collect::<Vec<_>>()
        .join("\n");
    let details = rows
        .iter()
        .map(|row| row.tooltip.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for expected in [
        "%11 = φ(σ₈:%10)",
        "%13 = ADD %11 %12 · μ₂→μ₃",
        "exit μ₃ · f₀ [%13]",
        "μ₃→μ₄ · %14",
        "μ₂ = effect φ(e₇₇:μ₁)",
    ] {
        assert!(visible.contains(expected), "missing {expected}: {visible}");
    }
    for expected in [
        "edge e₇₇ from σ₈: %10",
        "Arguments: [%13]",
        "Destination frame stacks: f₀ [%13, %14]",
        "Completed",
    ] {
        assert!(details.contains(expected), "missing {expected}: {details}");
    }
    assert_eq!(
        rows[3].target,
        Some(Selection {
            value: None,
            state: Some(27),
            pc: Some(400)
        })
    );
    assert_eq!(
        rows[5].target,
        Some(Selection {
            value: None,
            state: Some(91),
            pc: None
        })
    );
}

#[test]
fn incomplete_attempt_keeps_effect_and_phase_on_the_instruction_row() {
    let mut report = report();
    let instruction = &mut report.ssa.blocks[0].instructions[0];
    instruction.progress = InstructionProgress::Started;
    instruction.effect_result = None;
    instruction.results.clear();
    let rendered = rows(&report);
    let instruction = &rendered[3];
    assert!(instruction.plain_text().ends_with("μ₂→pending · Started"));
    assert!(!instruction.plain_text().contains("%13 ="));
}

fn frame(
    ctx: &Context,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) -> FullOutput {
    frame_with_events(ctx, report, selection, previous, vec![])
}

fn frame_with_events(
    ctx: &Context,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
    events: Vec<egui::Event>,
) -> FullOutput {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 550.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            ssa(
                ui,
                report,
                Some(0),
                SsaDisplay::Compact,
                selection,
                previous,
            )
        },
    );
    output.textures_delta.clear();
    output
}

#[test]
fn normalized_operations_keep_native_opcode_and_operand_provenance_in_tooltips() {
    use evm_abstract_protocol::{SsaOpcodeOperation, SsaOperand, SsaOperation};

    for (operation, expected) in [
        (SsaOperation::Constant("0x3".into()), "%13 = CONST 0x3"),
        (SsaOperation::Copy(SsaOperand::Value(11)), "%13 = COPY %11"),
        (
            SsaOperation::Opcode(SsaOpcodeOperation {
                opcode: 1,
                name: "ADD".into(),
                operands: vec![SsaOperand::Constant("0x1".into()), SsaOperand::Value(12)],
            }),
            "%13 = ADD 0x1 %12",
        ),
    ] {
        let mut report = report();
        report.ssa.blocks[0].instructions[0].operation = operation;
        let rendered = rows(&report);
        let row = &rendered[3];
        assert!(row.plain_text().contains(expected), "{}", row.plain_text());
        assert!(row.tooltip.contains("Original opcode: 0x01 ADD %11 %12"));
        assert!(row.tooltip.contains("μ₂→μ₃"));
        assert!(row.tooltip.contains("Completed"));
        assert_eq!(
            row.target,
            Some(Selection {
                state: Some(27),
                pc: Some(400),
                value: None,
            })
        );
    }
}

#[test]
fn clicking_a_value_token_selects_its_id_and_retains_the_native_reading_row() {
    let report = report();
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    let mut selection = Selection {
        state: Some(27),
        ..Selection::default()
    };
    let mut previous = selection;
    frame(&context, &report, &mut selection, &mut previous);
    let output = frame(&context, &report, &mut selection, &mut previous);
    let value = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "%13" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()).center())
            }
            _ => None,
        })
        .expect("instruction result token is visible");
    for pressed in [true, false] {
        frame_with_events(
            &context,
            &report,
            &mut selection,
            &mut previous,
            vec![
                egui::Event::PointerMoved(value),
                egui::Event::PointerButton {
                    pos: value,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
    assert_eq!(
        selection,
        Selection {
            state: Some(27),
            pc: Some(400),
            value: Some(13)
        }
    );
}

fn positions(output: &FullOutput) -> Vec<(String, Pos2)> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            egui::Shape::Text(text) => Some((text.galley.text().to_owned(), text.pos)),
            _ => None,
        })
        .collect()
}

#[test]
fn table_virtualizes_dense_rows_and_focuses_an_offscreen_native_instruction() {
    let mut report = report();
    let template = report.ssa.blocks[0].instructions[0].clone();
    report.ssa.blocks[0].instructions = (0..1000)
        .map(|pc| {
            let mut instruction = template.clone();
            instruction.pc = pc;
            instruction
        })
        .collect();
    let hidden = &mut report.ssa.blocks[0].instructions[999];
    hidden.opcode = 0x50;
    hidden.name = "POP".into();
    crate::tests::refresh_operation(hidden);
    hidden.results.clear();
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut selection = Selection {
        value: None,
        state: Some(27),
        pc: None,
    };
    let mut previous = selection;
    frame(&ctx, &report, &mut selection, &mut previous);
    let output = frame(&ctx, &report, &mut selection, &mut previous);
    let painted = positions(&output);
    let instruction_positions = painted
        .iter()
        .filter(|(text, _)| text.len() == 6 && text.ends_with("  "))
        .collect::<Vec<_>>();
    assert!(
        instruction_positions.len() >= 18,
        "compact table should show at least 18 instructions"
    );
    assert!(
        instruction_positions.len() < 30,
        "offscreen instructions must not be painted"
    );
    for adjacent in instruction_positions.windows(2) {
        assert!((adjacent[1].1.y - adjacent[0].1.y - 22.0).abs() < 0.1);
    }
    selection.pc = Some(999);
    for _ in 0..40 {
        frame(&ctx, &report, &mut selection, &mut previous);
    }
    let output = frame(&ctx, &report, &mut selection, &mut previous);
    assert!(
        positions(&output)
            .iter()
            .any(|(text, pos)| text == "03e7  " && (0.0..550.0).contains(&pos.y))
    );
    assert!(
        positions(&output)
            .iter()
            .any(|(text, pos)| text == "POP" && (0.0..550.0).contains(&pos.y))
    );
}

fn stack_moves_and_important_operations() -> AnalysisReport {
    let mut report = report();
    let template = report.ssa.blocks[0].instructions[0].clone();
    let operations = std::iter::once((0x50, "POP".to_owned()))
        .chain((0x80..=0x8f).map(|opcode| (opcode, format!("DUP{}", opcode - 0x7f))))
        .chain((0x90..=0x9f).map(|opcode| (opcode, format!("SWAP{}", opcode - 0x8f))))
        .chain(
            [
                (0x5b, "JUMPDEST"),
                (0x52, "MSTORE"),
                (0x55, "SSTORE"),
                (0xa0, "LOG0"),
                (0xf1, "CALL"),
                (0xf0, "CREATE"),
                (0x56, "JUMP"),
                (0xfd, "REVERT"),
            ]
            .into_iter()
            .map(|(opcode, name)| (opcode, name.to_owned())),
        );
    report.ssa.blocks[0].instructions = operations
        .enumerate()
        .map(|(pc, (opcode, name))| {
            let mut instruction = template.clone();
            instruction.pc = pc;
            instruction.opcode = opcode;
            instruction.name = name;
            crate::tests::refresh_operation(&mut instruction);
            instruction.results.clear();
            instruction
        })
        .collect();
    report
}

#[test]
fn compact_rows_hide_completed_stack_moves_and_full_restores_all_instruction_details() {
    let report = stack_moves_and_important_operations();
    let compact = scoped_rows(&report, Some(0), SsaDisplay::Compact, Selection::default());
    let full = scoped_rows(&report, Some(0), SsaDisplay::Full, Selection::default());
    let instruction_rows = |rows: &[Row]| {
        rows.iter()
            .filter(|row| row.target.is_some_and(|target| target.pc.is_some()))
            .count()
    };
    assert_eq!(instruction_rows(&compact), 8);
    assert_eq!(instruction_rows(&full), 41);
    for instruction in &report.ssa.blocks[0].instructions {
        let target = Some(Selection {
            value: None,
            state: Some(27),
            pc: Some(instruction.pc),
        });
        let full_row = full.iter().find(|row| row.target == target).unwrap();
        assert!(full_row.plain_text().contains(" · μ₂→μ₃ · Completed"));
        assert!(
            full_row
                .plain_text()
                .starts_with(&format!("{:04x}  ", instruction.pc))
        );
        let compact_row = compact.iter().find(|row| row.target == target);
        if instruction.pc < 33 {
            assert!(compact_row.is_none(), "{} must be hidden", instruction.name);
        } else {
            let row = compact_row.unwrap();
            assert!(row.plain_text().contains(&instruction.name));
            assert!(!row.plain_text().contains('μ'));
            assert!(!row.plain_text().contains("Completed"));
            assert!(row.tooltip.contains(" · μ₂→μ₃"));
            assert!(row.tooltip.contains("Completed"));
            assert!(
                row.tooltip
                    .contains(&format!("State σ₂₇ · pc 0x{:04x}", instruction.pc))
            );
        }
    }
}

#[test]
fn compact_rows_keep_pending_faulted_and_noncurrent_stack_moves() {
    let mut report = report();
    let template = report.ssa.blocks[0].instructions[0].clone();
    report.ssa.blocks[0].instructions = [
        InstructionProgress::Started,
        InstructionProgress::OperandsConsumed,
        InstructionProgress::Dispatched,
        InstructionProgress::Faulted,
        InstructionProgress::Completed,
    ]
    .into_iter()
    .enumerate()
    .map(|(pc, progress)| {
        let mut instruction = template.clone();
        instruction.pc = pc;
        instruction.opcode = 0x50;
        instruction.name = "POP".into();
        crate::tests::refresh_operation(&mut instruction);
        instruction.results.clear();
        instruction.progress = progress;
        instruction.fault = progress == InstructionProgress::Completed;
        instruction
    })
    .collect();
    let compact = scoped_rows(&report, Some(0), SsaDisplay::Compact, Selection::default());
    for instruction in &report.ssa.blocks[0].instructions {
        let row = compact
            .iter()
            .find(|row| {
                row.target
                    .is_some_and(|target| target.pc == Some(instruction.pc))
            })
            .unwrap();
        assert!(row.plain_text().contains(" · μ₂→μ₃"));
        assert!(
            row.plain_text()
                .ends_with(&format!(" · {:?}", instruction.progress))
        );
    }
    for coverage in [BlockCoverage::Unexecuted, BlockCoverage::Stale] {
        report.ssa.blocks[0].coverage = coverage;
        let instruction = &mut report.ssa.blocks[0].instructions[0];
        instruction.progress = InstructionProgress::Completed;
        instruction.fault = false;
        let compact = scoped_rows(&report, Some(0), SsaDisplay::Compact, Selection::default());
        assert!(compact.iter().any(|row| {
            row.target.is_some_and(|target| target.pc == Some(0))
                && row.plain_text().contains("POP")
        }));
    }
}

#[test]
fn compact_rows_reveal_only_the_selected_native_state_and_pc() {
    let mut report = stack_moves_and_important_operations();
    let mut cfg = report.cfg[0].clone();
    cfg.id = 91;
    report.cfg.push(cfg);
    let mut block = report.ssa.blocks[0].clone();
    block.state = 91;
    report.ssa.blocks.push(block);
    let selection = Selection {
        value: None,
        state: Some(91),
        pc: Some(5),
    };
    let compact = scoped_rows(&report, Some(0), SsaDisplay::Compact, selection);
    let revealed: Vec<_> = compact
        .iter()
        .filter(|row| {
            row.target
                .is_some_and(|target| target.pc.is_some_and(|pc| pc < 33))
        })
        .collect();
    assert_eq!(revealed.len(), 1);
    let row = revealed[0];
    assert_eq!(row.target, Some(selection));
    assert!(row.plain_text().starts_with("0005  DUP5"));
    assert!(!row.plain_text().contains('μ'));
    assert!(row.tooltip.contains(" · μ₂→μ₃"));
}

#[test]
fn program_scope_preserves_every_native_context_and_transition_to_other_programs() {
    let mut report = report();
    let cfg = report.cfg[0].clone();
    let ssa = report.ssa.blocks[0].clone();
    report.cfg = (0..131)
        .map(|position| {
            let mut state = cfg.clone();
            state.id = position * 17 + 27;
            state.context = vec![position];
            state.program = if position < 129 { Some(0) } else { Some(9) };
            state
        })
        .collect();
    report.ssa.blocks = report
        .cfg
        .iter()
        .map(|state| {
            let mut block = ssa.clone();
            block.state = state.id;
            block
        })
        .collect();
    report.edges[0].to = report.cfg[129].id;
    let scoped = scoped_rows(
        &report,
        Some(0),
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(27),
            pc: None,
        },
    );
    let headers: Vec<_> = scoped
        .iter()
        .filter(|row| row.header)
        .map(|row| row.target.unwrap().state.unwrap())
        .collect();
    assert_eq!(
        headers,
        report.cfg[..129]
            .iter()
            .map(|state| state.id)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        scoped
            .iter()
            .filter(|row| row.plain_text().contains("%13 = ADD"))
            .count(),
        129
    );
    let transition = scoped
        .iter()
        .find(|row| row.plain_text().starts_with("e₈₈ "))
        .unwrap();
    assert_eq!(transition.target.unwrap().state, Some(report.cfg[129].id));
    assert!(transition.tooltip.contains("Arguments: [%13]"));
    assert!(
        transition
            .tooltip
            .contains("Destination frame stacks: f₀ [%13, %14]")
    );
    let other = scoped_rows(
        &report,
        Some(9),
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(27),
            pc: None,
        },
    );
    assert_eq!(other.iter().filter(|row| row.header).count(), 2);
    assert!(!other.iter().any(|row| row.plain_text().starts_with("e₈₈ ")));
}

#[test]
fn no_program_scope_shows_only_the_selected_no_code_state_and_keeps_deferred_reason() {
    let mut report = report();
    report.cfg[0].program = None;
    let mut other_cfg = report.cfg[0].clone();
    other_cfg.id = 91;
    report.cfg.push(other_cfg);
    let mut other_ssa = report.ssa.blocks[0].clone();
    other_ssa.state = 91;
    report.ssa.blocks.push(other_ssa);
    report.ssa.transitions.clear();
    report
        .ssa
        .deferred_edges
        .push(evm_abstract_protocol::DeferredEdge {
            edge: 88,
            reason: evm_abstract_protocol::DeferredReason::SourceStale,
        });
    let scoped = scoped_rows(
        &report,
        None,
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(27),
            pc: None,
        },
    );
    assert_eq!(scoped.iter().filter(|row| row.header).count(), 1);
    assert_eq!(scoped[0].target.unwrap().state, Some(27));
    assert!(
        scoped
            .iter()
            .any(|row| row.plain_text() == "e₈₈ → σ₉₁ · deferred Some(SourceStale)")
    );
    let other = scoped_rows(
        &report,
        None,
        SsaDisplay::Compact,
        Selection {
            value: None,
            state: Some(91),
            pc: None,
        },
    );
    assert_eq!(other.iter().filter(|row| row.header).count(), 1);
    assert_eq!(other[0].target.unwrap().state, Some(91));
    assert!(scoped_rows(&report, None, SsaDisplay::Compact, Selection::default()).is_empty());
    assert!(
        scoped_rows(
            &report,
            Some(0),
            SsaDisplay::Compact,
            Selection {
                value: None,
                state: Some(27),
                pc: None
            }
        )
        .is_empty()
    );
}

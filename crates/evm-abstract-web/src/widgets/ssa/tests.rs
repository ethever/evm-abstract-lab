use egui::{Context, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, BlockCoverage, CfgEdge, EdgeKind, EffectInput,
    Fork, InstructionProgress, Phi, PhiInput, SCHEMA_VERSION, SsaBlock, SsaInstruction, SsaReport,
    SsaTransition,
};

use super::{Row, rows, scoped_rows, ssa};
use crate::app::Selection;

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
        "%11 = φ(S8:%10)",
        "%13 = ADD %11 %12 · μ2→μ3",
        "exit μ3 · f0 [%13]",
        "μ3→μ4 · %14",
        "μ2 = effect φ(e77:μ1)",
    ] {
        assert!(visible.contains(expected), "missing {expected}: {visible}");
    }
    for expected in [
        "edge e77 from S8: %10",
        "Arguments: [%13]",
        "Destination frame stacks: f0 [%13, %14]",
        "Completed",
    ] {
        assert!(details.contains(expected), "missing {expected}: {details}");
    }
    assert_eq!(
        rows[3].target,
        Some(Selection {
            state: Some(27),
            pc: Some(400)
        })
    );
    assert_eq!(
        rows[5].target,
        Some(Selection {
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
    assert!(instruction.plain_text().ends_with("μ2→pending · Started"));
    assert!(!instruction.plain_text().contains("%13 ="));
}

fn frame(
    ctx: &Context,
    report: &AnalysisReport,
    selection: &mut Selection,
    previous: &mut Selection,
) -> FullOutput {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 550.0))),
            ..RawInput::default()
        },
        |ui| ssa(ui, report, Some(0), selection, previous),
    );
    output.textures_delta.clear();
    output
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
    let ctx = Context::default();
    let mut selection = Selection {
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
        Selection {
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
        .find(|row| row.plain_text().starts_with("e88 "))
        .unwrap();
    assert_eq!(transition.target.unwrap().state, Some(report.cfg[129].id));
    assert!(transition.tooltip.contains("Arguments: [%13]"));
    assert!(
        transition
            .tooltip
            .contains("Destination frame stacks: f0 [%13, %14]")
    );
    let other = scoped_rows(
        &report,
        Some(9),
        Selection {
            state: Some(27),
            pc: None,
        },
    );
    assert_eq!(other.iter().filter(|row| row.header).count(), 2);
    assert!(!other.iter().any(|row| row.plain_text().starts_with("e88 ")));
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
        Selection {
            state: Some(27),
            pc: None,
        },
    );
    assert_eq!(scoped.iter().filter(|row| row.header).count(), 1);
    assert_eq!(scoped[0].target.unwrap().state, Some(27));
    assert!(
        scoped
            .iter()
            .any(|row| row.plain_text() == "e88 → S91 · deferred Some(SourceStale)")
    );
    let other = scoped_rows(
        &report,
        None,
        Selection {
            state: Some(91),
            pc: None,
        },
    );
    assert_eq!(other.iter().filter(|row| row.header).count(), 1);
    assert_eq!(other[0].target.unwrap().state, Some(91));
    assert!(scoped_rows(&report, None, Selection::default()).is_empty());
    assert!(
        scoped_rows(
            &report,
            Some(0),
            Selection {
                state: Some(27),
                pc: None
            }
        )
        .is_empty()
    );
}

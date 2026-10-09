use egui::{Context, FullOutput, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, BlockCoverage, CfgEdge, EdgeKind, EffectInput,
    Fork, InstructionProgress, Phi, PhiInput, SCHEMA_VERSION, SsaBlock, SsaInstruction, SsaReport,
    SsaTransition,
};

use super::{Row, rows, ssa};
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

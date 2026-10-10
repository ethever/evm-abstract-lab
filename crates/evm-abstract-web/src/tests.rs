use egui::{Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol as api;
use evm_abstract_protocol::{
    AnalysisReport, AnalysisScope, AnalysisStatus, AnalyzeReply, ApiError, ApiErrorCode,
    BlockCoverage, CfgBlock, CfgEdge, DisasmBlock, DisasmInstruction, EdgeKind, Fork,
    InstructionProgress, Phi, PhiInput, SCHEMA_VERSION, SsaBlock, SsaInstruction, SsaReport,
};

use crate::{
    Workspace,
    app::{Selection, View},
    widgets,
};

fn instruction(pc: usize, name: &str) -> DisasmInstruction {
    DisasmInstruction {
        pc,
        opcode: 0x5b,
        name: name.into(),
        immediate: None,
        size: 1,
        valid: true,
        stack_inputs: 0,
        stack_outputs: 0,
    }
}

// A structurally typed, unanchored fixture. Individual tests provide the
// graph/source facts they exercise; no RPC identity or transaction result is
// invented for those rendering-only fixtures.
pub(crate) fn empty_report() -> AnalysisReport {
    let top = api::ValueInfo {
        summary: "Top".into(),
        constants: None,
        known_zero: "0x0".into(),
        known_one: "0x0".into(),
        unsigned: api::WordBounds {
            lower: "0x0".into(),
            upper: format!("0x{}", "ff".repeat(32)),
        },
        signed: api::WordBounds {
            lower: format!("0x80{}", "00".repeat(31)),
            upper: format!("0x7f{}", "ff".repeat(31)),
        },
        congruence: api::CongruenceValue::Any,
        origins: None,
        code_address_role: false,
        identity: None,
        expression: None,
        symbolic_limit: false,
    };
    let address = |kind| api::AddressValue::Symbolic(api::InputSymbol { kind, index: None });
    AnalysisReport {
        schema_version: SCHEMA_VERSION,
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 0,
        status: AnalysisStatus::Converged,
        transfers: 0,
        disassembly: vec![],
        cfg: vec![],
        edges: vec![],
        diagnostics: vec![],
        frontiers: vec![],
        ssa: SsaReport {
            complete: true,
            value_count: 0,
            effect_count: 0,
            blocks: vec![],
            transitions: vec![],
            deferred_edges: vec![],
        },
        metadata: api::ReportMetadata {
            entry_address: "0x0000000000000000000000000000000000000000".into(),
            root_program: None,
            snapshot: None,
            fingerprint: format!("0x{}", "00".repeat(32)),
            work: 0,
            limits: api::AnalysisLimits::default(),
            environment: api::EnvironmentSnapshot {
                to: address(api::InputSymbolKind::To),
                caller: address(api::InputSymbolKind::Caller),
                origin: address(api::InputSymbolKind::Caller),
                call_value: top.clone(),
                calldata: 0,
                is_static: false,
                gas_upper_bound: None,
                gas_price: top.clone(),
                coinbase: address(api::InputSymbolKind::Coinbase),
                timestamp: top.clone(),
                number: top.clone(),
                prevrandao: top.clone(),
                gas_limit: top.clone(),
                chain_id: top.clone(),
                base_fee: top.clone(),
                blob_base_fee: top.clone(),
                block_hashes: vec![],
                blob_hashes: vec![],
                blob_count: top.clone(),
            },
        },
        programs: vec![],
        accounts: vec![],
        states: vec![],
        stores: vec![],
        outcomes: vec![],
        acquisition: None,
        byte_arrays: vec![api::ByteArraySnapshot {
            length: top.clone(),
            default: top,
            values: vec![],
            cells: vec![],
            memory: false,
        }],
    }
}

pub(crate) fn source_program(
    id: usize,
    code_address: String,
    blocks: Vec<DisasmBlock>,
) -> api::ProgramInfo {
    let byte_len = blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .map(|instruction| instruction.pc + instruction.size)
        .max()
        .unwrap_or(0);
    api::ProgramInfo {
        id,
        code_address,
        code_hash: format!("0x{}", "00".repeat(32)),
        kind: api::CodeKind::Runtime,
        bytecode: format!("0x{}", "00".repeat(byte_len)),
        blocks,
    }
}

pub(crate) fn report() -> AnalysisReport {
    let cfg: Vec<_> = [(0, 0, "CALLDATALOAD"), (1, 5, "PUSH1"), (2, 10, "ADD")]
        .into_iter()
        .map(|(id, pc, name)| CfgBlock {
            id,
            basic_block: id,
            start_pc: Some(pc),
            context: vec![],
            frame_depth: 1,
            code_address: "0x0000000000000000000000000000000000000000".into(),
            storage_address: "0x0000000000000000000000000000000000000000".into(),
            program: Some(0),
            instructions: vec![instruction(pc, name)],
            entry_stack: vec![],
            exit_stack: vec!["Top".into()],
            executed_pcs: vec![pc],
        })
        .collect();
    let blocks = cfg
        .iter()
        .map(|block| SsaBlock {
            state: block.id,
            coverage: BlockCoverage::Current,
            incoming_complete: true,
            phis: if block.id == 2 {
                vec![Phi {
                    result: 5,
                    frame: 0,
                    slot: 0,
                    inputs: vec![
                        PhiInput {
                            edge: 1,
                            predecessor: 0,
                            value: 1,
                        },
                        PhiInput {
                            edge: 2,
                            predecessor: 1,
                            value: 2,
                        },
                    ],
                }]
            } else {
                vec![]
            },
            instructions: vec![SsaInstruction {
                pc: block.start_pc.unwrap(),
                opcode: 0x01,
                name: block.instructions[0].name.clone(),
                immediate: None,
                operands: vec![1, 2],
                results: vec![block.id + 3],
                fault: false,
                progress: InstructionProgress::Completed,
                effect_input: block.id,
                effect_result: Some(block.id + 3),
            }],
            exit_frames: vec![vec![block.id + 3]],
            effect: block.id,
            effect_inputs: vec![],
            exit_effect: block.id + 3,
            open_incoming: vec![],
        })
        .collect();
    let mut report = AnalysisReport {
        schema_version: SCHEMA_VERSION,
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 11,
        status: AnalysisStatus::Converged,
        transfers: 3,
        disassembly: cfg
            .iter()
            .map(|block| DisasmBlock {
                id: block.basic_block,
                start_pc: block.start_pc.unwrap(),
                instructions: block.instructions.clone(),
            })
            .collect(),
        cfg,
        edges: vec![
            CfgEdge {
                id: 0,
                from: 0,
                to: 1,
                kind: EdgeKind::BranchFalse,
            },
            CfgEdge {
                id: 1,
                from: 0,
                to: 2,
                kind: EdgeKind::BranchTrue,
            },
            CfgEdge {
                id: 2,
                from: 1,
                to: 2,
                kind: EdgeKind::Jump,
            },
        ],
        ssa: SsaReport {
            complete: true,
            value_count: 6,
            effect_count: 6,
            blocks,
            transitions: vec![],
            deferred_edges: vec![],
        },
        diagnostics: vec![],
        frontiers: vec![],
        ..empty_report()
    };
    report.programs.push(source_program(
        0,
        report.cfg[0].code_address.clone(),
        report.disassembly.clone(),
    ));
    report.metadata.root_program = Some(0);
    report
}

pub(crate) fn ready() -> Workspace {
    let mut workspace = Workspace::default();
    // Existing source/SSA identity and camera regressions exercise native
    // states. The overview integration tests construct the product default.
    workspace.graph.set_mode(widgets::GraphMode::States);
    workspace.initial_command();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(report()),
    }));
    workspace
}

fn frame(ctx: &Context, workspace: &mut Workspace, events: Vec<Event>) -> FullOutput {
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
        events,
        ..RawInput::default()
    };
    workspace.prepare_input(&mut input);
    let mut output = ctx.run_ui(input, |ui| {
        workspace.show(ui);
    });
    // Host tests inspect shapes without a renderer; acknowledge texture deltas.
    output.textures_delta.clear();
    output
}

fn painted_text(output: &FullOutput) -> Vec<(String, Pos2)> {
    let mut result = vec![];
    for shape in &output.shapes {
        collect_text(&shape.shape, &mut result);
    }
    result
}

fn collect_text(shape: &egui::Shape, result: &mut Vec<(String, Pos2)>) {
    match shape {
        egui::Shape::Text(text) => result.push((
            evm_abstract_notation::normalize_subscripts(text.galley.text()),
            text.pos,
        )),
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, result);
            }
        }
        _ => {}
    }
}

fn click(ctx: &Context, workspace: &mut Workspace, pos: Pos2) {
    for pressed in [true, false] {
        frame(
            ctx,
            workspace,
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

#[test]
fn report_counters_above_wasm_word_size_decode_and_render_exactly() {
    let count = 9_007_199_254_740_993_u64;
    let mut report = empty_report();
    report.transfers = count;
    report.metadata.work = count;
    let reply = AnalyzeReply { result: Ok(report) };
    let decoded: AnalyzeReply =
        serde_json::from_slice(&serde_json::to_vec(&reply).unwrap()).unwrap();
    assert_eq!(decoded, reply);
    let mut workspace = Workspace::default();
    workspace.receive(Ok(decoded));
    assert!(workspace.accessible_status().starts_with("Ready:"));
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    assert!(
        painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains(&format!("{count} work")))
    );
}

#[test]
fn all_three_custom_views_paint_structural_content() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    // Panels stabilize their sizes during the first pass.
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let text = painted_text(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "DISASSEMBLY",
        "CONTROL FLOW",
        "SSA",
        "CALLDATALOAD",
        "σ2",
        "%5",
        "σ0:%1",
        "σ1:%2",
    ] {
        assert!(
            text.contains(required),
            "missing painted structural content {required}: {text}"
        );
    }
    assert!(
        output.shapes.len() > 80,
        "widgets must paint graph and structured rows"
    );
}

#[test]
fn clicking_source_row_links_native_state_and_ssa_instruction() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut workspace = ready();
    workspace.view = View::Disassembly;
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let pos = painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == "000a")
        .unwrap()
        .1
        + Vec2::new(8.0, 6.0);
    click(&ctx, &mut workspace, pos);
    assert_eq!(
        workspace.selection,
        Selection {
            state: Some(2),
            pc: Some(10)
        }
    );
    workspace.view = View::Ssa;
    let output = frame(&ctx, &mut workspace, vec![]);
    assert!(painted_text(&output).iter().any(|(text, _)| text == "%5"));
}

#[test]
fn clicking_cfg_node_selects_same_native_state() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut workspace = ready();
    workspace.view = View::Graph;
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let pos = painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == "σ2  ·  B2")
        .unwrap()
        .1
        + Vec2::splat(4.0);
    click(&ctx, &mut workspace, pos);
    assert_eq!(
        workspace.selection,
        Selection {
            state: Some(2),
            pc: None
        }
    );
}

#[test]
fn keyboard_view_switching_uses_real_egui_input() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut workspace = ready();
    for (key, view) in [
        (Key::Num1, View::Disassembly),
        (Key::Num2, View::Graph),
        (Key::Num3, View::Ssa),
        (Key::Num0, View::Split),
    ] {
        frame(
            &ctx,
            &mut workspace,
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(workspace.view, view);
        frame(
            &ctx,
            &mut workspace,
            vec![Event::Key {
                key,
                physical_key: None,
                pressed: false,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
    }
}

#[test]
fn reanalysis_retains_previous_snapshot_and_backend_errors_remain_visible() {
    let mut workspace = ready();
    assert!(workspace.accessible_status().starts_with("Ready:"));
    let crate::Command::Submit { request, .. } = workspace.initial_command() else {
        panic!("expected submit")
    };
    assert_eq!(request.limits.context_depth, 128);
    assert!(
        workspace
            .accessible_status()
            .contains("showing previous result")
    );
    workspace.receive(Ok(AnalyzeReply {
        result: Err(ApiError {
            code: ApiErrorCode::InvalidBytecode,
            message: "invalid hex at byte 1".into(),
            details: api::ErrorDetails::Bytecode(api::BytecodeFailure::InvalidHex(
                api::HexDigitFailure {
                    index: 1,
                    character: 'z',
                },
            )),
        }),
    }));
    assert!(
        workspace
            .accessible_status()
            .contains("InvalidBytecode: invalid hex at byte 1")
    );
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let output = frame(&ctx, &mut workspace, vec![]);
    assert!(
        painted_text(&output)
            .iter()
            .any(|(text, _)| text == "DISASSEMBLY"),
        "a rejected rerun must not discard the prior immutable report"
    );
}

#[test]
fn unsupported_schema_is_rejected_and_partial_coverage_is_explicit() {
    let mut workspace = Workspace::default();
    let mut unsupported = report();
    unsupported.schema_version += 1;
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(unsupported),
    }));
    assert!(
        workspace
            .accessible_status()
            .contains("Unsupported response schema")
    );
    let mut partial = report();
    partial.status = AnalysisStatus::Incomplete;
    partial.ssa.complete = false;
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(partial),
    }));
    assert!(
        workspace
            .accessible_status()
            .contains("Incomplete; SSA partial")
    );
}

#[test]
fn graph_layout_handles_cycles_disconnected_nodes_and_sparse_native_ids() {
    let mut report = report();
    report.cfg[2].id = 100;
    report.edges = vec![
        CfgEdge {
            id: 45,
            from: 0,
            to: 1,
            kind: EdgeKind::Jump,
        },
        CfgEdge {
            id: 81,
            from: 1,
            to: 0,
            kind: EdgeKind::Jump,
        },
    ];
    let positions = widgets::test_layout(&report);
    assert_eq!(positions.len(), 3);
    assert!(positions.contains_key(&100));
    assert_ne!(positions[&0], positions[&1]);
    assert_ne!(positions[&1], positions[&100]);
}

#[test]
fn zoom_clamps_extremes() {
    let mut graph = widgets::Graph::default();
    graph.zoom_at(Vec2::new(50.0, 30.0), 2.0);
    assert_eq!(graph.zoom, 2.0);
    graph.zoom_at(Vec2::new(50.0, 30.0), 100.0);
    assert_eq!(graph.zoom, 2.5);
    graph.zoom_at(Vec2::new(50.0, 30.0), 0.0001);
    assert_eq!(graph.zoom, 0.08);
}

#[test]
fn long_push_and_phi_rows_reserve_scrollable_painter_extents() {
    let mut report = report();
    let ordinary_width = widgets::test_content_columns(&report);
    report.ssa.blocks[0].instructions[0].immediate = Some(format!("0x{}", "ff".repeat(32)));
    let push_width = widgets::test_content_columns(&report);
    assert!(push_width > ordinary_width);
    report.ssa.blocks[2].phis[0].inputs = (0..40)
        .map(|index| PhiInput {
            edge: index,
            predecessor: index,
            value: index + 100,
        })
        .collect();
    assert!(widgets::test_content_columns(&report) > push_width * 3);
}

#[test]
fn horizontal_scroll_reaches_the_last_phi_argument() {
    let mut report = report();
    report.ssa.blocks[2].phis[0].inputs = (0..40)
        .map(|index| PhiInput {
            edge: index,
            predecessor: index,
            value: index + 100,
        })
        .collect();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    workspace.view = View::Ssa;
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let before = painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == "σ39:%139")
        .unwrap()
        .1;
    assert!(before.x > 1440.0, "fixture must extend beyond the viewport");
    frame(
        &ctx,
        &mut workspace,
        vec![
            Event::PointerMoved(Pos2::new(600.0, 400.0)),
            Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(-6000.0, 0.0),
                phase: egui::TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..30 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = frame(&ctx, &mut workspace, vec![]);
    let after = painted_text(&output)
        .into_iter()
        .find(|(text, _)| text == "σ39:%139")
        .unwrap()
        .1;
    assert!(
        (0.0..1400.0).contains(&after.x),
        "last phi argument must be visible after scrolling: {after:?}"
    );
}

#[test]
fn workspace_keeps_dark_palette_when_browser_prefers_light() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
            system_theme: Some(egui::Theme::Light),
            ..RawInput::default()
        },
        |ui| {
            workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    assert_eq!(ctx.theme(), egui::Theme::Dark);
    assert_eq!(
        ctx.global_style().visuals.panel_fill,
        crate::palette::BACKGROUND
    );
}

#[test]
fn automatic_row_focus_preserves_leading_columns_in_split_view() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut workspace = ready();
    for _ in 0..30 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = frame(&ctx, &mut workspace, vec![]);
    let labels = painted_text(&output);
    let disasm_left = labels
        .iter()
        .find(|(text, _)| text == "DISASSEMBLY")
        .unwrap()
        .1
        .x;
    let ssa_heading = labels
        .windows(2)
        .find(|pair| pair[0].0 == "SSA" && pair[1].0.starts_with("verified complete"))
        .unwrap()[0]
        .1;
    let ssa_left = ssa_heading.x;
    let pc_left = labels.iter().find(|(text, _)| text == "0000").unwrap().1.x;
    // The inspector also labels σ0, before the source panes are painted.
    // The final standalone σ0 is the actual SSA table definition.
    let definition_left = labels.iter().rfind(|(text, _)| text == "σ0").unwrap().1.x;
    assert!(
        pc_left >= disasm_left,
        "PC column is clipped by focus: {pc_left} < {disasm_left}"
    );
    assert!(
        definition_left >= ssa_left,
        "SSA definition is clipped by focus: {definition_left} < {ssa_left}"
    );
    workspace.selection = Selection {
        state: Some(2),
        pc: Some(10),
    };
    for _ in 0..30 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = frame(&ctx, &mut workspace, vec![]);
    let labels = painted_text(&output);
    let selected_pc_left = labels.iter().find(|(text, _)| text == "000a").unwrap().1.x;
    assert!((selected_pc_left - pc_left).abs() < 0.1);
}

#[test]
fn stale_and_unexecuted_blocks_never_claim_current_exit_evidence() {
    for coverage in [BlockCoverage::Stale, BlockCoverage::Unexecuted] {
        let mut report = report();
        report.cfg.retain(|block| block.id == 2);
        report.ssa.blocks.retain(|block| block.state == 2);
        report.ssa.blocks[0].coverage = coverage;
        report.ssa.blocks[0].instructions.clear();
        report.edges.clear();
        let mut workspace = Workspace::default();
        workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
        workspace.view = View::Ssa;
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        frame(&ctx, &mut workspace, vec![]);
        let output = frame(&ctx, &mut workspace, vec![]);
        let labels = painted_text(&output);
        assert!(
            labels
                .iter()
                .any(|(text, _)| text.starts_with("entry only μ"))
        );
        assert!(!labels.iter().any(|(text, _)| text.starts_with("exit μ")));
        workspace.view = View::Disassembly;
        let output = frame(&ctx, &mut workspace, vec![]);
        let add_is_muted = output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == "ADD" => {
                text.galley.job.sections[0].format.color == crate::palette::MUTED
            }
            _ => false,
        });
        assert!(
            add_is_muted,
            "historical executed_pcs must not highlight a current instruction"
        );
        workspace.view = View::Graph;
        let output = frame(&ctx, &mut workspace, vec![]);
        assert!(
            painted_text(&output)
                .iter()
                .any(|(text, _)| text.contains(&format!("{coverage:?}")))
        );
    }
}

#[test]
fn child_without_bytecode_has_no_invented_program_counter() {
    let mut report = report();
    report.cfg[0].frame_depth = 2;
    report.cfg[0].start_pc = None;
    report.cfg[0].program = None;
    report.cfg[0].instructions.clear();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    workspace.view = View::Disassembly;
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let output = frame(&ctx, &mut workspace, vec![]);
    let labels = painted_text(&output);
    assert!(
        labels
            .iter()
            .any(|(text, _)| text == "No captured bytecode for this frame or account")
    );
    assert!(!labels.iter().any(|(text, _)| text == "B0  ·  0x0000"));
}

#[path = "tests/overview.rs"]
mod overview;

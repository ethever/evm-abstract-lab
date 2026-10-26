use egui::{Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol as api;
use evm_abstract_protocol::{
    AnalysisGraph, AnalysisScope, AnalysisStatus, AnalyzeReply, ApiError, ApiErrorCode,
    BlockCoverage, CfgBlock, CfgEdge, DisasmBlock, DisasmInstruction, EdgeKind, Fork,
    InstructionProgress, Phi, PhiInput, SCHEMA_VERSION, SsaBlock, SsaInstruction, SsaReport,
};

mod fixture;
pub(crate) use fixture::Fixture as AnalysisReport;

use crate::{
    Workspace,
    app::{Selection, View},
    widgets,
};

pub(crate) fn native_publication() -> api::JobPublication {
    api::JobPublication {
        version: api::PublicationVersion(api::DecimalU64(1)),
        minimum_version: None,
        global_version: None,
        native_version: Some(api::PublicationVersion(api::DecimalU64(1))),
        model_closed: true,
    }
}

pub(crate) fn job_result(id: api::JobId, reply: api::AnalyzeReply) -> api::JobResultReply {
    api::JobResultReply {
        result: reply.result.map(|report| {
            let mut publication = native_publication();
            publication.global_version = report.global.as_ref().map(|_| publication.version);
            publication.native_version = report.native.as_ref().map(|_| publication.version);
            publication.model_closed = report.status == api::AnalysisStatus::Converged;
            api::JobResult {
                id,
                publication,
                report,
            }
        }),
    }
}

pub(crate) fn fixture_view(report: &api::AnalysisReport) -> crate::report::ReportView<'_> {
    crate::report::ReportView::new(
        report,
        crate::app::model::ReportViewId::fixture(1, report.selected_view),
        None,
    )
    .unwrap()
}

/// Native-component consumer fixtures from the real default producer. Symbolic
/// root detail is conditioned on the producer's retained owner namespace.
pub(crate) fn procedure_report(bytecode: &str) -> AnalysisReport {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: bytecode.into(),
            address: None,
        }),
        ..api::AnalyzeRequest::default()
    };
    request.limits.root_terminal_coverage = false;
    let report = native_component(
        evm_abstract_server::analyze::analyze(request)
            .unwrap()
            .into(),
    );
    report.validate().unwrap();
    assert!(!report.abstract_summaries.is_empty());
    report
}

/// Legacy native inspectors test this component independently of global coverage.
pub(crate) fn native_component(mut report: AnalysisReport) -> AnalysisReport {
    report.model.hybrid = None;
    report.model.minimum = None;
    report.model.global = None;
    report.model.selected_view = api::AnalysisNamespace::Native;
    report
}

fn instruction(pc: usize, name: &str) -> DisasmInstruction {
    DisasmInstruction {
        pc,
        opcode: 0x01,
        name: name.into(),
        immediate: None,
        size: 1,
        valid: true,
        stack_inputs: 0,
        stack_outputs: 0,
    }
}

pub(crate) fn opcode_operation(
    opcode: u8,
    name: &str,
    operands: &[usize],
    immediate: Option<&str>,
) -> api::SsaOperation {
    api::SsaOperation::Opcode(api::SsaOpcodeOperation {
        opcode,
        name: name.into(),
        operands: immediate
            .into_iter()
            .map(|word| api::SsaOperand::Constant(word.into()))
            .chain(operands.iter().map(|value| api::SsaOperand::Value(*value)))
            .collect(),
    })
}

pub(crate) fn refresh_operation(instruction: &mut SsaInstruction) {
    instruction.operation = opcode_operation(
        instruction.opcode,
        &instruction.name,
        &instruction.operands,
        instruction.immediate.as_deref(),
    );
}

// A structurally typed, unanchored fixture. Individual tests provide the
// graph/source facts they exercise; no RPC identity or transaction result is
// invented for those rendering-only fixtures.
pub(crate) fn empty_report() -> AnalysisReport {
    let top = api::ValueInfo {
        nonzero: false,
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
        model: api::AnalysisReport {
            process_controls: Vec::new(),
            schema_version: SCHEMA_VERSION,
            minimum: None,
            rich_export: api::RichExportOutcome::Complete,
            selected_view: api::AnalysisNamespace::Native,
            status: AnalysisStatus::Converged,
            work: 0,
            transfers: 0,
            postprocessing_work: 0,
            timings: api::AnalysisTimings::default(),
            acquisition: None,
            programs: vec![],
            global: None,
            hybrid: None,
            native: Some(std::sync::Arc::new(AnalysisGraph {
                symbolic: crate::tests::symbolic_graph(),
                pending_machines: vec![],
                scope: AnalysisScope::SingleProgram,
                fork: Fork::Osaka,
                byte_len: 0,
                status: AnalysisStatus::Converged,
                transfers: 0,
                cfg: vec![],
                edges: vec![],
                diagnostics: vec![],
                frontiers: vec![],
                ssa: SsaReport {
                    complete: true,
                    value_count: 0,
                    values: vec![],
                    effect_count: 0,
                    blocks: vec![],
                    transitions: vec![],
                    deferred_edges: vec![],
                },
                metadata: api::ReportMetadata {
                    source_snapshot: api::RootTerminalSnapshot::Offline(
                        api::RootTerminalOfflineSnapshot {
                            label: "consumer fixture".into(),
                            namespace: api::DecimalU64(0),
                        },
                    ),
                    revision: api::DecimalU64(0),
                    export_limits: api::ExportLimits {
                        max_nodes: 65536,
                        max_depth: 256,
                        max_exact_bytes: 16777216,
                    },
                    export_usage: api::ExportUsage {
                        nodes: 5,
                        exact_bytes: 0,
                        work: 5,
                    },
                    entry_address: "0x0000000000000000000000000000000000000000".into(),
                    root_binding: api::RootInputBinding::Physical(api::PhysicalRootInput {
                        address: "0x0000000000000000000000000000000000000000".into(),
                    }),
                    declared_root_source: api::ProcessSource::AnyRuntime(api::ProcessForkSource {
                        fork: Fork::Osaka,
                    }),
                    root_program: None,
                    snapshot: None,
                    fingerprint: format!("0x{}", "00".repeat(32)),
                    work: 0,
                    postprocessing_max_work: 2_000_000_000,
                    postprocessing_work: 0,
                    limits: api::AnalysisLimits::default(),
                    environment: api::EnvironmentSnapshot {
                        to: api::AddressValue::Concrete(
                            "0x0000000000000000000000000000000000000000".into(),
                        ),
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
                accounts: vec![],
                states: vec![],
                stores: vec![],
                outcomes: vec![],
                root_terminal_coverage: None,
                abstract_summaries: vec![],
                byte_arrays: vec![api::ByteArraySnapshot {
                    symbolic_facts: vec![],
                    symbolic: evm_abstract_protocol::ByteViewRef { node: 2 },
                    length: top.clone(),
                    default: top,
                    values: vec![],
                    cells: vec![],
                    memory: false,
                }],
            })),
        },
    }
}

pub(crate) fn source_program(
    id: usize,
    code_address: Option<String>,
    blocks: Vec<DisasmBlock>,
) -> std::sync::Arc<api::ProgramInfo> {
    let byte_len = blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .map(|instruction| instruction.pc + instruction.size)
        .max()
        .unwrap_or(0);
    std::sync::Arc::new(api::ProgramInfo {
        id,
        code_address,
        code_hash: format!("0x{}", "00".repeat(32)),
        kind: api::CodeKind::Runtime,
        fork: Fork::Osaka,
        bytecode: format!("0x{}", "00".repeat(byte_len)),
        blocks,
    })
}

pub(crate) fn report() -> AnalysisReport {
    let cfg: Vec<_> = [(0, 0, "CALLDATALOAD"), (1, 5, "PUSH1"), (2, 10, "ADD")]
        .into_iter()
        .map(|(id, pc, _)| CfgBlock {
            process: None,
            id,
            basic_block: Some(id),
            start_pc: Some(pc),
            context: vec![],
            frame_depth: 1,
            code_address: Some("0x0000000000000000000000000000000000000000".into()),
            storage_address: Some("0x0000000000000000000000000000000000000000".into()),
            program: Some(0),
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
                operation: opcode_operation(
                    0x01,
                    ["CALLDATALOAD", "PUSH1", "ADD"][block.id],
                    &[1, 2],
                    None,
                ),
                pc: block.start_pc.unwrap(),
                opcode: 0x01,
                name: ["CALLDATALOAD", "PUSH1", "ADD"][block.id].into(),
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
    let mut report = empty_report();
    let source = cfg
        .iter()
        .map(|block| DisasmBlock {
            id: block.basic_block.unwrap(),
            start_pc: block.start_pc.unwrap(),
            instructions: vec![instruction(
                block.start_pc.unwrap(),
                ["CALLDATALOAD", "PUSH1", "ADD"][block.id],
            )],
        })
        .collect();
    *report = AnalysisGraph {
        symbolic: crate::tests::symbolic_graph(),
        pending_machines: vec![],
        scope: AnalysisScope::SingleProgram,
        fork: Fork::Osaka,
        byte_len: 11,
        status: AnalysisStatus::Converged,
        transfers: 3,
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
            values: vec![],
            effect_count: 6,
            blocks,
            transitions: vec![],
            deferred_edges: vec![],
        },
        diagnostics: vec![],
        frontiers: vec![],
        ..std::sync::Arc::unwrap_or_clone(empty_report().model.native.unwrap())
    };
    report.model.programs.push(source_program(
        0,
        report.cfg[0].code_address.clone(),
        source,
    ));
    report.metadata.root_program = Some(0);
    fixture::attach_snapshots(&mut report);
    for state in &mut report.states {
        state.exit = None;
        state.entry.frames[0].stack.clear();
    }
    report
}

pub(crate) fn ready() -> Workspace {
    let mut workspace = Workspace::default();
    workspace.view = View::Split;
    // Existing source/SSA identity and camera regressions exercise native
    // states in Compare. Product-default tests construct Workspace directly.
    workspace.graph.set_mode(widgets::GraphMode::States);
    workspace.graph.show_ssa_content();
    report().validate().expect("consumer fixture must validate");
    workspace.initial_command();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((report()).into()),
    }));
    workspace
}

pub(crate) fn frame(ctx: &Context, workspace: &mut Workspace, events: Vec<Event>) -> FullOutput {
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

pub(crate) fn painted_text(output: &FullOutput) -> Vec<(String, Pos2)> {
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
    let reply = AnalyzeReply {
        result: Ok((report).into()),
    };
    let decoded: AnalyzeReply =
        serde_json::from_slice(&serde_json::to_vec(&reply).unwrap()).unwrap();
    assert_eq!(decoded, reply);
    let mut workspace = Workspace::default();
    workspace.receive(Ok(decoded));
    assert!(
        workspace
            .accessible_status()
            .starts_with("Preparing result view:")
    );
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
    workspace.select_source_ssa();
    // Panels stabilize their sizes during the first pass.
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let text = painted_text(&output)
        .into_iter()
        .map(|(text, _)| text)
        .collect::<Vec<_>>()
        .join("\n");
    for required in [
        "CODE",
        "CONTROL FLOW",
        "SSA",
        "CALLDATALOAD",
        "Block 0x000a",
        "%5",
        "σ0:%1",
        "σ1:%2",
    ] {
        assert!(
            text.contains(required),
            "missing painted structural content {required}: {text}"
        );
    }
}

#[test]
fn clicking_source_row_links_native_state_and_ssa_instruction() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut workspace = ready();
    workspace.view = View::Disassembly;
    workspace.selection = Selection {
        state: Some(2),
        pc: None,
        value: None,
    };
    frame(&ctx, &mut workspace, vec![]);
    let output = frame(&ctx, &mut workspace, vec![]);
    let pos = painted_text(&output)
        .into_iter()
        .find(|(text, _)| text.starts_with("0x000a  "))
        .unwrap()
        .1
        + Vec2::new(8.0, 6.0);
    click(&ctx, &mut workspace, pos);
    assert_eq!(
        workspace.selection,
        Selection {
            value: None,
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
        .find(|(text, _)| text == "Block 0x000a")
        .unwrap()
        .1
        + Vec2::splat(4.0);
    click(&ctx, &mut workspace, pos);
    assert_eq!(
        workspace.selection,
        Selection {
            value: None,
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
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    frame(&context, &mut workspace, vec![]);
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
        painted_text(&output).iter().any(|(text, _)| text == "CODE"),
        "a rejected rerun must not discard the prior immutable report"
    );
}

#[test]
fn unsupported_schema_is_rejected_and_partial_coverage_is_explicit() {
    let mut workspace = Workspace::default();
    for version in [SCHEMA_VERSION - 1, SCHEMA_VERSION + 1] {
        let mut unsupported = report();
        unsupported.model.schema_version = version;
        workspace.receive(Ok(AnalyzeReply {
            result: Ok((unsupported).into()),
        }));
        assert!(workspace.accessible_status().contains(&format!(
            "Unsupported response schema {version}; expected {SCHEMA_VERSION}"
        )));
    }
    let mut partial = report();
    partial.status = AnalysisStatus::Incomplete;
    partial.ssa.complete = false;
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((partial).into()),
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
    let positions = widgets::test_layout(&(report).view());
    assert_eq!(positions.len(), 3);
    assert!(positions.contains_key(&100));
    assert_ne!(positions[&0], positions[&1]);
    assert_ne!(positions[&1], positions[&100]);
}

#[test]
fn zoom_caps_enlargement_and_rejects_invalid_factors() {
    let mut graph = widgets::Graph::default();
    graph.zoom_at(Vec2::new(50.0, 30.0), 2.0);
    assert_eq!(graph.zoom, 2.0);
    graph.zoom_at(Vec2::new(50.0, 30.0), 100.0);
    assert_eq!(graph.zoom, 2.5);
    graph.zoom_at(Vec2::new(50.0, 30.0), 0.0001);
    assert!(graph.zoom > 0.0 && graph.zoom < 0.08);
    let small = graph.zoom;
    graph.zoom_at(Vec2::new(50.0, 30.0), 0.5);
    assert!(graph.zoom > 0.0 && graph.zoom < small);
    let unchanged = graph.zoom;
    for factor in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        graph.zoom_at(Vec2::new(50.0, 30.0), factor);
        assert_eq!(graph.zoom, unchanged);
    }
}

#[test]
fn long_push_and_phi_rows_reserve_scrollable_painter_extents() {
    let mut report = report();
    let ordinary_width = widgets::test_content_columns(&(report).view());
    report.ssa.blocks[0].instructions[0].immediate = Some(format!("0x{}", "ff".repeat(32)));
    refresh_operation(&mut report.ssa.blocks[0].instructions[0]);
    let push_width = widgets::test_content_columns(&(report).view());
    assert!(push_width > ordinary_width);
    report.ssa.blocks[2].phis[0].inputs = (0..40)
        .map(|index| PhiInput {
            edge: index,
            predecessor: index,
            value: index + 100,
        })
        .collect();
    assert!(widgets::test_content_columns(&(report).view()) > push_width * 3);
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
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((report).into()),
    }));
    workspace.view = View::Ssa;
    workspace.selection = Selection {
        state: Some(2),
        pc: None,
        value: None,
    };
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
    let disasm_left = labels.iter().find(|(text, _)| text == "CODE").unwrap().1.x;
    let ssa_heading = labels
        .windows(2)
        .find(|pair| pair[0].0 == "SSA" && pair[1].0.starts_with("verified complete"))
        .unwrap()[0]
        .1;
    let ssa_left = ssa_heading.x;
    let pc_left = labels
        .iter()
        .find(|(text, _)| text.starts_with("0x0000  "))
        .unwrap()
        .1
        .x;
    // The inspector also labels σ0, before the source panes are painted.
    // The final standalone σ0 is the actual SSA table definition.
    let definition_left = labels.iter().rfind(|(text, _)| text == "σ0").unwrap().1.x;
    let definition_clip_left = output
        .shapes
        .iter()
        .rev()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Text(text)
                if evm_abstract_notation::normalize_subscripts(text.galley.text()) == "σ0" =>
            {
                Some(shape.clip_rect.left())
            }
            _ => None,
        })
        .expect("the standalone definition must have a real painter clip");
    assert!(
        pc_left >= disasm_left,
        "PC column is clipped by focus: {pc_left} < {disasm_left}"
    );
    assert!(
        definition_left >= definition_clip_left,
        "SSA definition is clipped by focus: {definition_left} < {definition_clip_left}"
    );
    // Table text is snapped to physical pixels; the heading's widget position
    // can remain fractional after natural pane sizing. Check clipping against
    // the actual clip and alignment within that single-pixel rounding budget.
    assert!(
        (definition_left - ssa_left).abs() <= 1.0 / ctx.pixels_per_point(),
        "SSA leading column does not align with its heading"
    );
    workspace.selection = Selection {
        value: None,
        state: Some(2),
        pc: Some(10),
    };
    for _ in 0..30 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = frame(&ctx, &mut workspace, vec![]);
    let labels = painted_text(&output);
    let selected_pc_left = labels
        .iter()
        .find(|(text, _)| text.starts_with("0x000a  "))
        .unwrap()
        .1
        .x;
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
        workspace.receive(Ok(AnalyzeReply {
            result: Ok((report).into()),
        }));
        workspace.view = View::Ssa;
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        frame(&ctx, &mut workspace, vec![]);
        let output = frame(&ctx, &mut workspace, vec![]);
        let labels = painted_text(&output);
        match coverage {
            BlockCoverage::Stale => assert!(
                labels
                    .iter()
                    .any(|(text, _)| text.starts_with("entry only μ")),
                "a retained stale receipt must remain browseable"
            ),
            BlockCoverage::Unexecuted => assert!(labels.iter().any(|(text, _)| {
                text == crate::app::model::presentation::UnavailableReason::Unexecuted.label()
            })),
            BlockCoverage::Current => unreachable!(),
        }
        assert!(!labels.iter().any(|(text, _)| text.starts_with("exit μ")));
        workspace.view = View::Disassembly;
        let output = frame(&ctx, &mut workspace, vec![]);
        let add_is_muted = output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text().ends_with("  ADD") => {
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
    report.cfg[0].executed_pcs.clear();
    report.ssa.blocks[0].instructions.clear();
    report.states[0].entry.frames[0].program = None;
    report.states[0].entry.frames[0].kind = api::CodeKind::Empty;
    report.validate().unwrap();
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok((report).into()),
    }));
    workspace.view = View::Disassembly;
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let output = frame(&ctx, &mut workspace, vec![]);
    let labels = painted_text(&output);
    assert!(labels.iter().any(|(text, _)| text
        == crate::app::model::presentation::UnavailableReason::NoCapturedSource.label()));
    assert!(!labels.iter().any(|(text, _)| text == "B0  ·  0x0000"));
}

#[path = "tests/overview.rs"]
mod overview;

/// Minimal well-typed directory used by snapshots in UI fixtures.
pub(crate) fn symbolic_graph() -> api::SymbolicGraph {
    api::SymbolicGraph {
        nodes: vec![
            api::Expression::BytesZero,
            api::Expression::Constant("0x0".into()),
            api::Expression::View(api::ByteView {
                state: api::ByteStateRef { node: 0 },
                offset: api::ExpressionRef { node: 1 },
                length: api::ExpressionRef { node: 1 },
            }),
            api::Expression::SlotsZero,
            api::Expression::PathTrue,
        ],
    }
}

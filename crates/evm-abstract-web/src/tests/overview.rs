//! Real egui interactions retain native identity behind display-only groups.
use super::{click, frame, painted_text, report, source_program};
use crate::{
    Workspace,
    app::{Selection, View},
};
use egui::{Context, Event, FullOutput, Key, Modifiers, MouseWheelUnit, Rect, TouchPhase, Vec2};
use evm_abstract_protocol::{
    self as api, AnalysisReport, AnalyzeReply, CfgEdge, DisasmBlock, EdgeKind,
};

fn workspace(report: AnalysisReport) -> Workspace {
    let mut workspace = Workspace::default();
    workspace.receive(Ok(AnalyzeReply { result: Ok(report) }));
    workspace.view = View::Graph;
    workspace
}
fn settled(ctx: &Context, workspace: &mut Workspace) -> FullOutput {
    frame(ctx, workspace, vec![]);
    frame(ctx, workspace, vec![]);
    frame(ctx, workspace, vec![])
}
fn text_rect(output: &FullOutput, accept: impl Fn(&str) -> bool) -> Rect {
    fn collect(
        shape: &egui::Shape,
        clip: Rect,
        accept: &impl Fn(&str) -> bool,
        result: &mut Vec<Rect>,
    ) {
        match shape {
            egui::Shape::Text(text) if accept(text.galley.text()) => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                if rect.intersect(clip).height() >= rect.height() - 1.0 {
                    result.push(rect);
                }
            }
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    collect(shape, clip, accept, result)
                }
            }
            _ => {}
        }
    }
    let mut rects = vec![];
    for shape in &output.shapes {
        collect(&shape.shape, shape.clip_rect, &accept, &mut rects);
    }
    *rects.last().expect("expected visible interactive label")
}
fn choose(ctx: &Context, workspace: &mut Workspace, label: &str) {
    let output = settled(ctx, workspace);
    click(
        ctx,
        workspace,
        text_rect(&output, |text| text == label).center(),
    );
    settled(ctx, workspace);
}
fn grouped_report(count: usize) -> AnalysisReport {
    let mut result = report();
    let template = result.cfg[0].clone();
    let ssa = result.ssa.blocks[0].clone();
    result.cfg = (0..count)
        .map(|index| {
            let mut state = template.clone();
            state.id = 41 + index * 3;
            state.context = vec![0; index];
            state.entry_stack = vec![format!("entry-{}", state.id)];
            state
        })
        .collect();
    result.ssa.blocks = result
        .cfg
        .iter()
        .map(|state| {
            let mut block = ssa.clone();
            block.state = state.id;
            block.effect = state.id;
            block.instructions[0].results = vec![state.id + 1];
            block
        })
        .collect();
    result.ssa.value_count = count * 4 + 100;
    result.ssa.effect_count = count * 4 + 100;
    result.edges = (0..count)
        .map(|index| CfgEdge {
            id: 900 + index * 7,
            from: result.cfg[index].id,
            to: result.cfg[(index + 1).min(count - 1)].id,
            kind: EdgeKind::Jump,
        })
        .collect();
    result.disassembly.truncate(1);
    result.programs[0].blocks.truncate(1);
    result.transfers = count as u64;
    attach_snapshots(&mut result);
    result
}

fn scalar(value: usize) -> api::ValueInfo {
    let text = format!("0x{value:x}");
    api::ValueInfo {
        summary: format!("{{{text}}}"),
        constants: Some(vec![text.clone()]),
        known_zero: "0x0".into(),
        known_one: text.clone(),
        unsigned: api::WordBounds {
            lower: text.clone(),
            upper: text.clone(),
        },
        signed: api::WordBounds {
            lower: text.clone(),
            upper: text.clone(),
        },
        congruence: api::CongruenceValue::Exact(text),
        origins: Some(vec![api::ValueOrigin::Constant]),
        code_address_role: false,
        identity: None,
        expression: None,
        symbolic_limit: false,
    }
}

fn attach_snapshots(report: &mut AnalysisReport) {
    let top = report.metadata.environment.call_value.clone();
    report.stores = vec![api::StoreSnapshot {
        storage_default: top.clone(),
        transient_default: scalar(0),
        balance_default: top,
        accounts: vec![],
        logs: vec![],
        logs_unknown: false,
    }];
    report.byte_arrays.truncate(1);
    for memory in [true, false] {
        report.byte_arrays.push(api::ByteArraySnapshot {
            length: scalar(0),
            default: scalar(0),
            values: vec![],
            cells: vec![],
            memory,
        });
    }
    report.states = report
        .cfg
        .iter()
        .map(|block| {
            let program = report
                .programs
                .iter()
                .find(|program| Some(program.id) == block.program)
                .unwrap();
            let entry = api::MachineSnapshot {
                frames: vec![api::FrameSnapshot {
                    index: 0,
                    program: block.program,
                    code_address: block.code_address.clone(),
                    code_hash: program.code_hash.clone(),
                    storage_address: block.storage_address.clone(),
                    address_value: api::AddressValue::Concrete(block.storage_address.clone()),
                    caller: report.metadata.environment.caller.clone(),
                    call_value: report.metadata.environment.call_value.clone(),
                    is_static: false,
                    kind: program.kind,
                    basic_block: block.basic_block,
                    context: block.context.clone(),
                    stack: vec![scalar(block.id)],
                    memory: 1,
                    calldata: 0,
                    returndata: 2,
                    rollback_store: 0,
                    continuation: None,
                }],
                store: 0,
            };
            api::StateDetails {
                state: block.id,
                exit: Some(entry.clone()),
                entry,
            }
        })
        .collect();
}
fn assert_scope(workspace: &Workspace, mode: &str, nodes: usize, states: &str) {
    let status = workspace.accessible_status();
    for expected in [
        format!("CFG view: {mode}"),
        format!("{nodes} shown nodes"),
        format!("{states} states"),
    ] {
        assert!(status.contains(&expected), "missing {expected}: {status}");
    }
}

#[test]
fn default_overview_instance_picker_keeps_sparse_native_ids_and_linked_ssa() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let original = grouped_report(129);
    let last = original.cfg.last().unwrap().id;
    let mut workspace = workspace(original);
    let output = settled(&ctx, &mut workspace);
    assert_scope(&workspace, "Blocks", 1, "129/129");
    assert!(
        painted_text(&output)
            .iter()
            .any(|(text, _)| text.contains("129 states"))
    );
    assert!(workspace.accessible_status().contains("1 shown edges"));
    let instances = text_rect(&output, |text| text.starts_with("Instances"));
    click(&ctx, &mut workspace, instances.center());
    let output = settled(&ctx, &mut workspace);
    let first = text_rect(&output, |text| text == "S41" || text.starts_with("S41 "));
    frame(
        &ctx,
        &mut workspace,
        vec![
            Event::PointerMoved(first.center()),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -8000.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    for _ in 0..20 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = settled(&ctx, &mut workspace);
    let prefix = format!("S{last}");
    let target = text_rect(&output, |text| {
        text == prefix || text.starts_with(&format!("{prefix} "))
    });
    click(&ctx, &mut workspace, target.center());
    settled(&ctx, &mut workspace);
    assert_eq!(
        workspace.selection,
        Selection {
            state: Some(last),
            pc: None
        }
    );
    assert_scope(&workspace, "Blocks", 1, "129/129");
    assert!(
        workspace
            .accessible_status()
            .contains(&format!("Selected S{last}; source P0"))
    );
    choose(&ctx, &mut workspace, "Stack");
    let output = settled(&ctx, &mut workspace);
    assert!(workspace.accessible_status().contains("Frame 0"));
    assert!(
        painted_text(&output)
            .iter()
            .any(|(text, _)| text == &format!("{{0x{last:x}}}")),
        "inspector must resolve the chosen state's actual snapshot"
    );
    frame(
        &ctx,
        &mut workspace,
        vec![Event::Key {
            key: Key::Num3,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    // TableBuilder animates long-distance focus, just as the existing
    // offscreen SSA regression does; let its native frames finish scrolling.
    for _ in 0..40 {
        frame(&ctx, &mut workspace, vec![]);
    }
    let output = settled(&ctx, &mut workspace);
    assert_eq!(workspace.view, View::Ssa);
    assert!(
        painted_text(&output)
            .iter()
            .filter(|(text, _)| text == &prefix)
            .count()
            >= 2,
        "both SSA's selected native block and inspector must identify the chosen instance: {:?}",
        painted_text(&output)
    );
    let instruction = text_rect(&output, |text| text == format!("%{}", last + 1));
    click(&ctx, &mut workspace, instruction.center());
    assert_eq!(
        workspace.selection,
        Selection {
            state: Some(last),
            pc: Some(0)
        }
    );
    frame(
        &ctx,
        &mut workspace,
        vec![Event::Key {
            key: Key::Num2,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let output = settled(&ctx, &mut workspace);
    let card = text_rect(&output, |text| text == "P0:B0 · 129 states");
    click(&ctx, &mut workspace, card.center());
    settled(&ctx, &mut workspace);
    assert_eq!(
        workspace.selection,
        Selection {
            state: Some(last),
            pc: None
        },
        "a grouped card keeps the chosen native instance while clearing instruction focus"
    );
    // The same picker also reaches a sparse native ID through its real search
    // control, independently of the previously scrolled virtual list.
    let search = ctx
        .read_response(egui::Id::new("cfg_instance_search"))
        .unwrap();
    click(&ctx, &mut workspace, search.interact_rect.center());
    frame(
        &ctx,
        &mut workspace,
        vec![
            Event::Key {
                key: Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Modifiers::NONE
                },
            },
            Event::Text(prefix.clone()),
        ],
    );
    let output = settled(&ctx, &mut workspace);
    let target = text_rect(&output, |text| text.starts_with(&format!("{prefix} ·")));
    click(&ctx, &mut workspace, target.center());
    settled(&ctx, &mut workspace);
    choose(&ctx, &mut workspace, "Local");
    assert_scope(&workspace, "Local", 3, "3/129");
    let status = workspace.accessible_status();
    assert!(
        status.contains("126 hidden states") && status.contains("1 boundary edges"),
        "{status}"
    );
    assert!(
        status.contains("Converged"),
        "display truncation must not change analysis status"
    );
}

#[test]
fn states_and_program_filter_preserve_cross_program_local_neighbors() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut report = grouped_report(2);
    let mut child = report.cfg[0].clone();
    child.id = 777;
    child.program = Some(1);
    child.code_address = "0x2222222222222222222222222222222222222222".into();
    report.programs.push(source_program(
        1,
        child.code_address.clone(),
        vec![DisasmBlock {
            id: child.basic_block,
            start_pc: 0,
            instructions: child.instructions.clone(),
        }],
    ));
    report.cfg.push(child);
    report.edges = vec![
        CfgEdge {
            id: 501,
            from: 41,
            to: 44,
            kind: EdgeKind::Jump,
        },
        CfgEdge {
            id: 502,
            from: 44,
            to: 777,
            kind: EdgeKind::Call,
        },
        CfgEdge {
            id: 503,
            from: 777,
            to: 44,
            kind: EdgeKind::Return,
        },
    ];
    let mut child_ssa = report.ssa.blocks[0].clone();
    child_ssa.state = 777;
    report.ssa.blocks.push(child_ssa);
    attach_snapshots(&mut report);
    let mut workspace = workspace(report);
    settled(&ctx, &mut workspace);
    assert_scope(&workspace, "Blocks", 2, "3/3");
    choose(&ctx, &mut workspace, "States");
    assert_scope(&workspace, "States", 3, "3/3");
    choose(&ctx, &mut workspace, "All programs");
    let output = settled(&ctx, &mut workspace);
    let p1 = text_rect(&output, |text| text == "P1 · Runtime");
    click(&ctx, &mut workspace, p1.center());
    settled(&ctx, &mut workspace);
    assert_scope(&workspace, "States", 1, "1/3");
    let status = workspace.accessible_status();
    assert!(
        status.contains("2 hidden states") && status.contains("2 boundary edges"),
        "{status}"
    );
    // Program filtering changes visibility, never the underlying selected ID.
    assert_eq!(workspace.selection.state, Some(41));
    choose(&ctx, &mut workspace, "Local");
    assert_scope(&workspace, "Local", 3, "3/3");
    assert!(workspace.accessible_status().contains("0 hidden states"));
    assert_eq!(workspace.selection.state, Some(41));
}

#[test]
fn new_reports_in_code_or_ssa_views_announce_that_graph_display_is_pending() {
    for (key, view) in [(Key::Num1, View::Disassembly), (Key::Num3, View::Ssa)] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut workspace = workspace(report());
        settled(&ctx, &mut workspace);
        assert_scope(&workspace, "Blocks", 3, "3/3");
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
        workspace.receive(Ok(AnalyzeReply {
            result: Ok(grouped_report(2)),
        }));
        settled(&ctx, &mut workspace);
        let status = workspace.accessible_status();
        assert!(
            status.contains("CFG view: Blocks; awaiting display"),
            "{status}"
        );
        assert!(
            !status.contains("0/0 states"),
            "hidden display is not an empty analysis"
        );
        frame(
            &ctx,
            &mut workspace,
            vec![Event::Key {
                key: Key::Num2,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        settled(&ctx, &mut workspace);
        assert_scope(&workspace, "Blocks", 1, "2/2");
    }
}

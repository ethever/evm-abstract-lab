use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Vec2};
use evm_abstract_protocol::{AnalyzeReply, PhiInput};

use super::tests::{active_tree, click, frame, pane_rect, settled, texts};
use super::{GRAPH_AUTO_MIN, PANE_MIN, Pane, columns};
use crate::{Workspace, app::Selection, tests::ready, widgets};

fn widths(workspace: &Workspace, size: Vec2) -> [f32; 3] {
    let tree = active_tree(workspace, size.x);
    [Pane::Disassembly, Pane::Graph, Pane::Ssa].map(|pane| pane_rect(tree, pane).width())
}

fn natural(ctx: &Context, workspace: &Workspace) -> [f32; 2] {
    let mut widths = [0.0; 2];
    let mut output = ctx.run_ui(RawInput::default(), |ui| {
        let report = workspace.report.as_ref().unwrap();
        widths = [
            widgets::disassembly_width(ui, report, workspace.selection).max(PANE_MIN),
            widgets::ssa_width(ui, report).max(PANE_MIN),
        ];
    });
    output.textures_delta.clear();
    widths
}

#[test]
fn first_frame_sizes_short_code_from_its_tables_and_gives_growth_to_cfg() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1440.0, 900.0);
    frame(&ctx, &mut workspace, size, Vec::new());
    let first = widths(&workspace, size);
    let desired = natural(&ctx, &workspace);
    assert!((first[0] - desired[0]).abs() < 1.0);
    assert!((first[2] - desired[1]).abs() < 1.0);
    assert!(
        first[0] < 250.0,
        "short disassembly must not reserve 27% of a wide screen"
    );
    assert!(first[1] > GRAPH_AUTO_MIN);
    let larger = Vec2::new(1920.0, 900.0);
    settled(&ctx, &mut workspace, larger);
    let grown = widths(&workspace, larger);
    assert!((grown[0] - first[0]).abs() < 1.0);
    assert!((grown[2] - first[2]).abs() < 1.0);
    assert!((grown[1] - first[1] - (larger.x - size.x)).abs() < 1.0);
}

#[test]
fn long_push_and_phi_keep_a_graph_budget_without_hiding_source_data() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let report = workspace.report.as_mut().unwrap();
    let immediate = format!("0x{}", "fe".repeat(32));
    report.disassembly[0].instructions[0].name = "PUSH32".into();
    report.disassembly[0].instructions[0].immediate = Some(immediate.clone());
    report.ssa.blocks[2].phis[0].inputs = (0..80)
        .map(|id| PhiInput {
            edge: id,
            predecessor: id,
            value: id + 100,
        })
        .collect();
    let size = Vec2::new(1440.0, 900.0);
    let output = settled(&ctx, &mut workspace, size);
    let allocated = widths(&workspace, size);
    let desired = natural(&ctx, &workspace);
    assert!(allocated.iter().all(|width| *width >= PANE_MIN - 1.0));
    assert!(allocated[1] >= GRAPH_AUTO_MIN - 1.0);
    assert!(
        allocated[2] < desired[1] / 3.0,
        "a long phi must scroll within a bounded pane"
    );
    let painted = texts(&output);
    assert!(
        painted
            .iter()
            .any(|(text, _)| text == &format!(" {immediate}"))
    );
    assert!(painted.iter().any(|(text, _)| text == "S79:%179"));
}

#[test]
fn selected_child_source_and_reanalysis_remeasure_automatic_panes() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1920.0, 900.0);
    settled(&ctx, &mut workspace, size);
    let original = widths(&workspace, size);
    let report = workspace.report.as_mut().unwrap();
    let mut child = report.cfg[0].clone();
    child.id = 99;
    child.frame_depth = 2;
    child.start_pc = Some(1000);
    child.instructions[0].pc = 1000;
    child.instructions[0].name = "PUSH32".into();
    child.instructions[0].immediate = Some(format!("0x{}", "ff".repeat(32)));
    report.cfg.push(child);
    workspace.selection = Selection {
        state: Some(99),
        pc: None,
    };
    settled(&ctx, &mut workspace, size);
    let child_widths = widths(&workspace, size);
    assert!(child_widths[0] > original[0] + 200.0);
    assert!(
        (child_widths[2] - original[2]).abs() < 1.0,
        "SSA still displays every block"
    );
    workspace.selection.state = Some(0);
    settled(&ctx, &mut workspace, size);
    assert!((widths(&workspace, size)[0] - original[0]).abs() < 1.0);
    let mut replacement = crate::tests::report();
    replacement.disassembly[0].instructions[0].immediate = Some(format!("0x{}", "ab".repeat(32)));
    workspace.begin_analysis();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(replacement),
    }));
    settled(&ctx, &mut workspace, size);
    assert!(widths(&workspace, size)[0] > original[0] + 200.0);
}

fn drag_left_divider(ctx: &Context, workspace: &mut Workspace, size: Vec2) {
    let rect = pane_rect(&workspace.layout.wide, Pane::Disassembly);
    let start = Pos2::new(rect.right() + 2.5, rect.center().y);
    frame(ctx, workspace, size, vec![Event::PointerMoved(start)]);
    frame(
        ctx,
        workspace,
        size,
        vec![Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        }],
    );
    let end = start + Vec2::new(90.0, 0.0);
    frame(ctx, workspace, size, vec![Event::PointerMoved(end)]);
    frame(
        ctx,
        workspace,
        size,
        vec![Event::PointerButton {
            pos: end,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
}

#[test]
fn manual_divider_widths_survive_growth_content_changes_and_reanalysis() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(1440.0, 900.0);
    settled(&ctx, &mut workspace, size);
    let initial = widths(&workspace, size);
    drag_left_divider(&ctx, &mut workspace, size);
    settled(&ctx, &mut workspace, size);
    let manual = widths(&workspace, size);
    assert!(
        manual[0] > initial[0] + 50.0,
        "exercise a real divider drag"
    );
    for width in [1920.0, 1128.0, 1024.0, 390.0, 1920.0] {
        let resized = Vec2::new(width, 900.0);
        settled(&ctx, &mut workspace, resized);
        if width >= 1128.0 {
            let next = widths(&workspace, resized);
            assert!((next[0] - manual[0]).abs() < 1.0);
            assert!((next[2] - manual[2]).abs() < 1.0);
        }
    }
    let mut replacement = crate::tests::report();
    replacement.disassembly[0].instructions[0].immediate = Some(format!("0x{}", "ff".repeat(32)));
    workspace.begin_analysis();
    workspace.receive(Ok(AnalyzeReply {
        result: Ok(replacement),
    }));
    settled(&ctx, &mut workspace, size);
    let next = widths(&workspace, size);
    assert!((next[0] - manual[0]).abs() < 1.0);
    assert!((next[2] - manual[2]).abs() < 1.0);
}

#[test]
fn medium_code_tabs_use_active_content_and_keep_the_graph_visible() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = ready();
    let size = Vec2::new(900.0, 700.0);
    let output = settled(&ctx, &mut workspace, size);
    let disassembly = pane_rect(&workspace.layout.medium, Pane::Disassembly).width();
    let code_id = columns(&workspace.layout.medium).children[0];
    let tabs = workspace.layout.medium.tiles.rect(code_id).unwrap();
    let target = texts(&output)
        .into_iter()
        .find(|(text, rect)| {
            text == "SSA" && tabs.contains(rect.center()) && rect.center().y < tabs.top() + 24.0
        })
        .unwrap()
        .1
        .center();
    click(&ctx, &mut workspace, size, target);
    settled(&ctx, &mut workspace, size);
    let ssa = pane_rect(&workspace.layout.medium, Pane::Ssa).width();
    assert!(ssa > disassembly + 30.0);
    assert!(pane_rect(&workspace.layout.medium, Pane::Graph).width() >= GRAPH_AUTO_MIN - 1.0);
    let desired = natural(&ctx, &workspace)[1];
    assert!((ssa - desired).abs() < 1.0);
}

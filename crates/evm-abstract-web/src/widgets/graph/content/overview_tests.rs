//! Group cards are source summaries; native SSA and payload identities stay separate.
use super::{DisplayNode, NodeView, ReportIndex, node_preview, node_tooltip};
use egui::{Context, RawInput};
use evm_abstract_protocol as api;

fn grouped() -> (api::AnalysisReport, DisplayNode) {
    let mut report = crate::tests::report();
    let source = report.cfg[2].clone();
    let ssa = report.ssa.blocks[2].clone();
    report.cfg.clear();
    report.ssa.blocks.clear();
    report.edges.clear();
    for (offset, id) in [205, 999, 1001].into_iter().enumerate() {
        let mut block = source.clone();
        block.id = id;
        block.context = vec![offset];
        let mut names = ssa.clone();
        names.state = id;
        names.phis[0].result = 9000 + offset;
        names.instructions[0].results = vec![9100 + offset];
        report.cfg.push(block);
        report.ssa.blocks.push(names);
    }
    let node = DisplayNode {
        id: 205,
        members: vec![205, 999, 1001],
        program: source.program,
        basic_block: source.basic_block,
        start_pc: source.start_pc,
    };
    (report, node)
}
fn details(
    report: &api::AnalysisReport,
    state: usize,
    storage: &str,
    caller: &str,
    is_static: bool,
) -> api::StateDetails {
    let frame = api::FrameSnapshot {
        index: 0,
        program: Some(0),
        code_address: report.cfg[0].code_address.clone(),
        code_hash: "0x00".into(),
        storage_address: storage.into(),
        address_value: api::AddressValue::Concrete(storage.into()),
        caller: api::AddressValue::Concrete(caller.into()),
        call_value: report.metadata.environment.call_value.clone(),
        is_static,
        kind: api::CodeKind::Runtime,
        basic_block: 2,
        context: vec![],
        stack: vec![],
        memory: 0,
        calldata: 0,
        returndata: 0,
        rollback_store: 0,
        continuation: None,
    };
    api::StateDetails {
        state,
        entry: api::MachineSnapshot {
            frames: vec![frame],
            store: 0,
        },
        exit: None,
    }
}
#[test]
fn grouped_ssa_never_relabels_one_members_names_as_the_whole_block() {
    let (report, node) = grouped();
    let index = ReportIndex::new(&report);
    let context = Context::default();
    let mut output =
        context.run_ui(RawInput::default(), |ui| {
            let preview = node_preview(ui.painter(), &report, &index, &node, NodeView::Ssa);
            assert_eq!(preview.title, "P0:B2 · 3 states");
            assert!(
                preview
                    .lines
                    .iter()
                    .any(|(line, _)| line == "Select an instance for SSA")
            );
            assert!(
                preview
                    .lines
                    .iter()
                    .any(|(line, _)| line.contains("Per-state SSA: 1 phis"))
            );
            assert!(preview.lines.iter().any(|(line, _)| line.contains("ADD")));
            assert!(preview.lines.iter().all(|(line, _)| !line.contains('%')
                && !line.contains('μ')
                && !line.contains('φ')));
            assert!(preview.tooltip.is_empty());
            let singleton = DisplayNode {
                id: 999,
                members: vec![999],
                program: node.program,
                basic_block: node.basic_block,
                start_pc: node.start_pc,
            };
            let exact = node_tooltip(&report, &index, &singleton, NodeView::Ssa);
            assert!(exact.contains("%9001 = φ"));
            assert!(exact.contains("%9101 = ADD"));
            assert!(!exact.contains("%9000 = φ"));
        });
    output.textures_delta.clear();
}
#[test]
fn mixed_receipts_and_call_storage_roles_remain_visible_without_a_fake_current_group() {
    let (mut report, node) = grouped();
    report.ssa.blocks[1].coverage = api::BlockCoverage::Stale;
    report.ssa.blocks[2].coverage = api::BlockCoverage::Unexecuted;
    report.cfg[1].storage_address = "0x2222222222222222222222222222222222222222".into();
    report.cfg[1].frame_depth = 2;
    report.states = vec![
        details(&report, 205, &report.cfg[0].storage_address, "0x11", false),
        details(&report, 999, &report.cfg[1].storage_address, "0x22", true),
        details(&report, 1001, &report.cfg[2].storage_address, "0x11", false),
    ];
    report.frontiers = vec![
        api::Frontier {
            from: Some(999),
            pc: Some(10),
            kind: api::FrontierKind::Memory,
            reason: api::FrontierDetails::Memory,
            detail: "memory bound".into()
        };
        2
    ];
    let index = ReportIndex::new(&report);
    let context = Context::default();
    let mut output = context.run_ui(RawInput::default(), |ui| {
        let preview = node_preview(ui.painter(), &report, &index, &node, NodeView::Ssa);
        assert_eq!(preview.coverage, api::BlockCoverage::Stale);
        assert!(preview.frontier);
        assert!(preview.detail.contains("2 frontiers"));
        assert!(
            preview
                .lines
                .iter()
                .any(|(line, _)| line == "Current 1 · Stale 1 · Unexecuted 1")
        );
        let roles = preview
            .lines
            .iter()
            .find(|(line, _)| line.starts_with("Varies:"))
            .unwrap();
        for field in ["storage", "depth", "caller", "static"] {
            assert!(roles.0.contains(field), "missing role {field}: {}", roles.0);
        }
        let tooltip = node_tooltip(&report, &index, &node, NodeView::Ssa);
        for required in [
            "S205",
            "S999",
            "S1001",
            "not machine payloads or SSA definitions",
        ] {
            assert!(tooltip.contains(required));
        }
    });
    output.textures_delta.clear();
}
#[test]
fn large_receipts_keep_preview_bounded_and_generate_complete_details_only_on_demand() {
    let (mut report, group) = grouped();
    let block = &mut report.ssa.blocks[0];
    let input = block.phis[0].inputs[0].clone();
    block.phis[0].inputs = (0..2000)
        .map(|value| {
            let mut input = input.clone();
            input.value = value;
            input
        })
        .collect();
    let template = block.instructions[0].clone();
    block.instructions = (0..1000)
        .map(|pc| {
            let mut row = template.clone();
            row.pc = pc;
            row
        })
        .collect();
    block.exit_frames = vec![vec![123456; 1024]; 32];
    report.edges.push(api::CfgEdge {
        id: 900,
        from: 205,
        to: 999,
        kind: api::EdgeKind::Return,
    });
    report.ssa.transitions.push(api::SsaTransition {
        edge: 900,
        kind: api::EdgeKind::Return,
        operands: vec![1999],
        stacks: vec![vec![88888]],
        effect_input: 70,
        effect_result: 71,
        result: Some(88888),
    });
    let node = DisplayNode {
        id: 205,
        members: vec![205],
        program: group.program,
        basic_block: group.basic_block,
        start_pc: group.start_pc,
    };
    let index = ReportIndex::new(&report);
    let context = Context::default();
    let mut output = context.run_ui(RawInput::default(), |ui| {
        let preview = node_preview(ui.painter(), &report, &index, &node, NodeView::Ssa);
        assert!(preview.tooltip.is_empty());
        assert!(preview.lines.len() <= 8);
        assert!(
            preview
                .lines
                .iter()
                .all(|(line, _)| line.chars().count() <= super::SSA_COLUMNS)
        );
        assert!(
            !preview
                .lines
                .iter()
                .any(|(line, _)| line.contains("%1999") || line.contains("%88888"))
        );
        let full = node_tooltip(&report, &index, &node, NodeView::Ssa);
        for required in ["%1999", "03e7", "f31 [", "e900 → S999", "μ70→μ71 · %88888"] {
            assert!(full.contains(required), "full lazy tooltip lost {required}");
        }
    });
    output.textures_delta.clear();
}
#[test]
fn group_member_hover_is_bounded_and_keeps_original_sparse_ids() {
    let (mut report, mut node) = grouped();
    let block = report.cfg[0].clone();
    node.members.clear();
    report.cfg.clear();
    report.ssa.blocks.clear();
    for offset in 0..100 {
        let mut block = block.clone();
        block.id = 205 + offset * 17;
        node.members.push(block.id);
        report.cfg.push(block);
    }
    let index = ReportIndex::new(&report);
    let tooltip = node_tooltip(&report, &index, &node, NodeView::Ssa);
    assert!(tooltip.contains("S205 ·"));
    assert!(tooltip.contains("S392 ·"));
    assert!(!tooltip.contains("S409 ·"));
    assert!(tooltip.contains("88 more states. Open Instances"));
    assert!(tooltip.lines().count() < 24);
}

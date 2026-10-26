use super::{DisplayGraph, GraphMode, ReportIndex, SOURCE_CARD_LIMIT, same_context};
use evm_abstract_protocol::{CfgEdge, EdgeKind};

#[test]
fn qualified_scope_keeps_owner_caller_static_and_missing_evidence_separate() {
    let original = crate::tests::report();
    for difference in ["owner", "caller", "static", "source", "missing"] {
        let mut report = original.clone();
        match difference {
            "owner" => {
                report.states[1].entry.frames[0].storage_address = Some("other owner".into())
            }
            "caller" => {
                report.states[1].entry.frames[0].caller = Some(
                    evm_abstract_protocol::AddressValue::Concrete("other caller".into()),
                )
            }
            "static" => report.states[1].entry.frames[0].is_static = true,
            "source" => report.states[1].entry.frames[0].code_hash = Some("other code".into()),
            "missing" => {
                report.states.remove(1);
            }
            _ => unreachable!(),
        }
        let before = report.clone();
        let view = report.view();
        let index = ReportIndex::new(&view);
        assert!(!same_context(&view, &index, 0, 1), "{difference}");
        let display =
            DisplayGraph::scoped(&view, &index, GraphMode::Blocks, Some(0), Some(0), None, 2);
        assert!(!display.state_to_node.contains_key(&1), "{difference}");
        assert!(
            display
                .portals
                .values()
                .any(|portal| portal.targets.contains(&1)),
            "retained edge must expose the separate {difference} destination"
        );
        assert_eq!(report, before);
    }
}

#[test]
fn six_hundred_blocks_have_bounded_cards_and_reachable_unmodified_boundaries() {
    let mut report = crate::tests::report();
    let template = report.cfg[0].clone();
    let detail = report.states[0].clone();
    report.cfg = (0..600)
        .map(|id| {
            let mut state = template.clone();
            state.id = id * 7 + 10;
            state.basic_block = Some(id);
            state.start_pc = Some(id * 5);
            state
        })
        .collect();
    report.states = report
        .cfg
        .iter()
        .map(|state| {
            let mut detail = detail.clone();
            detail.state = state.id;
            detail.entry.frames[0].basic_block = state.basic_block;
            detail
        })
        .collect();
    report.edges = report
        .cfg
        .windows(2)
        .enumerate()
        .map(|(id, pair)| CfgEdge {
            id,
            from: pair[0].id,
            to: pair[1].id,
            kind: EdgeKind::Fallthrough,
        })
        .collect();
    let before = report.clone();
    let view = report.view();
    let index = ReportIndex::new(&view);
    let display = DisplayGraph::scoped(
        &view,
        &index,
        GraphMode::Blocks,
        Some(0),
        Some(10),
        Some(10),
        2,
    );
    assert_eq!(display.state_to_node.len(), SOURCE_CARD_LIMIT);
    assert_eq!(display.hidden_states, 600 - SOURCE_CARD_LIMIT);
    assert_eq!(display.portals.len(), 1);
    assert_eq!(display.boundary_edges, 1);
    let target = display.portals.values().next().unwrap().targets[0];
    assert_eq!(target, report.cfg[SOURCE_CARD_LIMIT].id);
    let continued = DisplayGraph::scoped(
        &view,
        &index,
        GraphMode::Blocks,
        Some(0),
        Some(10),
        Some(target),
        2,
    );
    assert!(continued.state_to_node.contains_key(&target));
    assert!(continued.state_to_node.len() <= SOURCE_CARD_LIMIT);
    assert_eq!(
        report, before,
        "windowing never mutates or joins analysis receipts"
    );
}

#[test]
fn native_portals_keep_distinct_original_edge_associations_and_dangling_ids_out() {
    let mut report = crate::tests::report();
    report.states[1].entry.frames[0].is_static = true;
    report.states[2].entry.frames[0].is_static = true;
    report.edges = vec![
        CfgEdge {
            id: 501,
            from: 0,
            to: 1,
            kind: EdgeKind::Call,
        },
        CfgEdge {
            id: 777,
            from: 0,
            to: 2,
            kind: EdgeKind::Call,
        },
        CfgEdge {
            id: 999,
            from: 0,
            to: 9999,
            kind: EdgeKind::Call,
        },
    ];
    let view = report.view();
    let index = ReportIndex::new(&view);
    let display = DisplayGraph::scoped(
        &view,
        &index,
        GraphMode::Blocks,
        Some(0),
        Some(0),
        Some(0),
        2,
    );
    assert_eq!(display.state_to_node.len(), 1);
    assert_eq!(display.portals.len(), 1);
    assert_eq!(display.portals.values().next().unwrap().targets, [1, 2]);
    assert_eq!(display.edge_members[&501], [501, 777]);
    assert_eq!(display.boundary_edges, 2);
}

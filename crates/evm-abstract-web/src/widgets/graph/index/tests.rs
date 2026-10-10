use evm_abstract_protocol::{
    CfgEdge, DeferredEdge, DeferredReason, EdgeKind, Frontier, FrontierDetails, FrontierKind,
    MachineSnapshot, SsaTransition, StateDetails,
};

use super::ReportIndex;

#[test]
fn sparse_ids_resolve_original_records_and_keep_adjacency_positions() {
    let mut report = crate::tests::report();
    for (position, id) in [91, 7, 205].into_iter().enumerate() {
        report.cfg[position].id = id;
        report.ssa.blocks[position].state = id;
    }
    report.ssa.blocks.swap(0, 2);
    report.states = vec![StateDetails {
        state: 7,
        entry: MachineSnapshot {
            frames: vec![],
            store: 0,
        },
        exit: None,
    }];
    report.edges = vec![
        CfgEdge {
            id: 800,
            from: 7,
            to: 91,
            kind: EdgeKind::Call,
        },
        CfgEdge {
            id: 42,
            from: 7,
            to: 91,
            kind: EdgeKind::Failure,
        },
        CfgEdge {
            id: 999,
            from: 205,
            to: 7,
            kind: EdgeKind::Return,
        },
    ];
    report.ssa.transitions = vec![SsaTransition {
        edge: 999,
        kind: EdgeKind::Return,
        operands: vec![7],
        stacks: vec![vec![9]],
        effect_input: 2,
        effect_result: 8,
        result: Some(9),
    }];
    report.ssa.deferred_edges = vec![DeferredEdge {
        edge: 42,
        reason: DeferredReason::SourceStale,
    }];
    report.frontiers = [Some(7), Some(7), Some(205), None]
        .into_iter()
        .map(|from| Frontier {
            from,
            pc: Some(5),
            kind: FrontierKind::Work,
            reason: FrontierDetails::Work,
            detail: "test work boundary".into(),
        })
        .collect();
    let index = ReportIndex::new(&report);
    assert!(std::ptr::eq(index.cfg(&report, 7).unwrap(), &report.cfg[1]));
    assert!(std::ptr::eq(
        index.ssa(&report, 91).unwrap(),
        &report.ssa.blocks[2]
    ));
    assert!(std::ptr::eq(
        index.details(&report, 7).unwrap(),
        &report.states[0]
    ));
    assert!(std::ptr::eq(
        index.edge(&report, 42).unwrap(),
        &report.edges[1]
    ));
    assert_eq!(index.transition(&report, 999).unwrap().stacks, [vec![9]]);
    assert_eq!(
        index.deferred(&report, 42).unwrap().reason,
        DeferredReason::SourceStale
    );
    assert_eq!(index.outgoing(7), [0, 1]);
    assert_eq!(index.incoming(91), [0, 1]);
    assert_eq!(index.incoming(7), [2]);
    assert_eq!(index.frontier_count(7), 2);
    assert_eq!(index.frontier_count(205), 1);
    assert!(index.has_frontier(205));
    assert!(!index.has_frontier(91));
    assert!(index.cfg(&report, 0).is_none());
    assert!(index.ssa(&report, 0).is_none());
    assert!(index.details(&report, 91).is_none());
    assert!(index.edge(&report, 0).is_none());
    assert!(index.transition(&report, 42).is_none());
    assert!(index.deferred(&report, 999).is_none());
    assert!(index.outgoing(12345).is_empty());
    assert!(index.incoming(12345).is_empty());
}

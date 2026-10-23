use evm_abstract_protocol::{
    CfgEdge, DeferredEdge, DeferredReason, EdgeKind, Frontier, FrontierDetails, FrontierKind,
    MachineSnapshot, Outcome, OutcomeKind, SsaTransition, StateDetails,
};

use super::ReportIndex;

#[test]
fn source_current_index_distinguishes_superseded_from_latest_uncertified_and_certified() {
    use evm_abstract_protocol::{ProcedureSummaryKind as Kind, SummarySourceStatus as Source};
    let mut report = crate::tests::procedure_report("5f5f5f5f5f305af150602a5f5260205ff3");
    report.abstract_summaries.truncate(1);
    let source = report.abstract_summaries[0].source_state;
    report.abstract_summaries[0].kind = Kind::KnownCall;
    report.abstract_summaries[0].source_current = false;
    assert_eq!(
        ReportIndex::new(&report).summary_source(source, Kind::KnownCall),
        Some(Source::Uncertified)
    );
    let mut latest = report.abstract_summaries[0].clone();
    latest.kind = Kind::KnownRecursion;
    report.abstract_summaries.push(latest);
    let index = ReportIndex::new(&report);
    assert_eq!(
        index.summary_source(source, Kind::KnownCall),
        Some(Source::Historical)
    );
    assert_eq!(
        index.summary_source(source, Kind::KnownRecursion),
        Some(Source::Uncertified)
    );
    report.abstract_summaries.last_mut().unwrap().source_current = true;
    let index = ReportIndex::new(&report);
    assert_eq!(
        index.summary_source(source, Kind::KnownCall),
        Some(Source::Historical)
    );
    assert_eq!(
        index.summary_source(source, Kind::KnownRecursion),
        Some(Source::Current)
    );
}

#[test]
fn regional_known_call_index_never_implies_recursive_or_native_termination_evidence() {
    use evm_abstract_protocol::ProcedureSummaryKind;
    let mut report = crate::tests::procedure_report("5f5f5f5f5f305af150602a5f5260205ff3");
    let source = report.abstract_summaries[0].source_state;
    let original = ReportIndex::new(&report).termination_summary(&[source]);
    for summary in &mut report.abstract_summaries {
        summary.kind = ProcedureSummaryKind::KnownCall;
    }
    let index = ReportIndex::new(&report);
    assert!(
        index
            .summary_source(source, ProcedureSummaryKind::KnownCall)
            .is_some()
    );
    assert!(
        !index
            .summary_source(source, ProcedureSummaryKind::KnownRecursion)
            .is_some()
    );
    assert_eq!(index.termination_summary(&[source]), original);
    for summary in &mut report.abstract_summaries {
        summary.kind = ProcedureSummaryKind::KnownRecursion;
    }
    let index = ReportIndex::new(&report);
    assert!(
        index
            .summary_source(source, ProcedureSummaryKind::KnownRecursion)
            .is_some()
    );
    assert!(
        !index
            .summary_source(source, ProcedureSummaryKind::KnownCall)
            .is_some()
    );
    for summary in &mut report.abstract_summaries {
        summary.kind = ProcedureSummaryKind::KnownProcess;
    }
    let index = ReportIndex::new(&report);
    assert!(
        index
            .summary_source(source, ProcedureSummaryKind::KnownProcess)
            .is_some()
    );
    assert!(
        index
            .summary_source(source, ProcedureSummaryKind::KnownRecursion)
            .is_none()
    );
    assert_eq!(index.termination_summary(&[source]), original);
}

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
            path: evm_abstract_protocol::PathRef { node: 4 },
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
            payload: None,
            range: None,
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

#[test]
fn cached_termination_summary_resolves_sparse_native_members() {
    let mut report = crate::tests::report();
    for (position, id) in [91, 7, 205].into_iter().enumerate() {
        report.cfg[position].id = id;
        report.ssa.blocks[position].state = id;
    }
    report.cfg[1].frame_depth = 2;
    report.edges = vec![CfgEdge {
        id: 999,
        from: 7,
        to: 91,
        kind: EdgeKind::Return,
    }];
    report.outcomes = vec![Outcome {
        state: 205,
        kind: OutcomeKind::Return,
        data: 0,
        store: 0,
    }];
    let summary = ReportIndex::new(&report).termination_summary(&[205, 7]);
    assert_eq!(summary.members, 2);
    assert_eq!(summary.root.returns, 1);
    assert_eq!(summary.frame.returns, 1);
    assert_eq!(summary.single_kind(), Some(OutcomeKind::Return));
}

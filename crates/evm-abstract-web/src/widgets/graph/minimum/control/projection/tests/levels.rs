use super::super::super::{
    Control,
    scope::{
        CardKind, ControlOrigin, Destination, EdgeLevel, FlowOrigin, GraphLevel, GraphProjection,
        GraphScope, GraphWindow,
    },
};
use super::{report, scope};
use evm_abstract_protocol as api;

fn block(projection: &GraphProjection, index: usize) -> usize {
    projection
        .cards
        .iter()
        .position(|card| {
            matches!(card.kind,
        CardKind::Source { location: api::ProcessLocation::Block(block), .. } if block == index)
        })
        .unwrap()
}

fn source_index(projection: &GraphProjection, id: usize) -> Option<usize> {
    match projection.cards[id].kind {
        CardKind::Source {
            location: api::ProcessLocation::Block(index),
            ..
        } => Some(index),
        _ => None,
    }
}

#[test]
fn original_weth_entry_has_two_source_successors_and_retains_all_macro_relations() {
    let report = report(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layout/ethereum-weth9.hex"
        ))
        .trim(),
    );
    let scope = scope(&report);
    let projected = GraphProjection::build(&report, scope).unwrap();
    assert_eq!(projected.level, GraphLevel::Source);
    let entry = block(&projected, 0);
    let outgoing: Vec<_> = projected
        .edges
        .iter()
        .filter(|edge| edge.from == entry)
        .collect();
    assert_eq!(outgoing.len(), 2);
    assert!(outgoing.iter().all(|edge| edge.level == EdgeLevel::Source));
    assert!(
        outgoing
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::BranchTrue
                && source_index(&projected, edge.to) == Some(12))
    );
    assert!(
        outgoing
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::BranchFalse
                && source_index(&projected, edge.to) == Some(1))
    );
    assert!(
        projected
            .edges
            .iter()
            .all(|edge| edge.kind != api::EdgeKind::ProcessControl)
    );
    let root = projected.cards[entry]
        .origins
        .iter()
        .find(|origin| origin.target.origin.location.is_none())
        .unwrap()
        .target
        .origin
        .node;
    let cfg = &report.minimum.as_ref().unwrap().summaries[scope.summary()]
        .relation
        .regional_cfg;
    let body = cfg.nodes.iter().find(|node| node.id == root).unwrap();
    let macros: Vec<_> = body
        .transfer
        .as_ref()
        .unwrap()
        .successors
        .iter()
        .filter(|edge| edge.kind == api::EdgeKind::ProcessControl)
        .collect();
    assert!(
        macros.len() > 2,
        "the original WETH macro fanout is the regression trigger"
    );
    for edge in macros {
        assert!(
            projected
                .model_relations
                .iter()
                .any(|relation| relation.kind == Some(edge.kind)
                    && relation.origin
                        == FlowOrigin::Regional {
                            from: root,
                            to: edge.to
                        })
        );
    }
    let overview = GraphProjection::build(
        &report,
        GraphScope::Overview {
            summary: scope.summary(),
        },
    )
    .unwrap();
    assert_eq!(overview.level, GraphLevel::Model);
    assert!(
        overview.edges.iter().any(
            |edge| edge.kind == api::EdgeKind::ProcessControl && edge.level == EdgeLevel::Model
        )
    );
    assert!(projected.model_set.is_some());
    assert!(projected.counts.summarized_model_relations > 0);
}

#[test]
fn original_six_hundred_window_has_adjacent_first_blocks_and_no_terminal_shortcut() {
    let report = report(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layout/long-linear-chain-600.hex"
        ))
        .trim(),
    );
    let scope = scope(&report);
    let projected =
        GraphProjection::with_window(&report, scope, GraphWindow::Nearby { point_limit: 12 })
            .unwrap();
    let mut blocks: Vec<_> = (0..projected.cards.len())
        .filter_map(|id| source_index(&projected, id))
        .collect();
    blocks.sort_unstable();
    assert_eq!(blocks, (0..12).collect::<Vec<_>>());
    let entry = block(&projected, 0);
    let outgoing: Vec<_> = projected
        .edges
        .iter()
        .filter(|edge| edge.from == entry)
        .collect();
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].kind, api::EdgeKind::Fallthrough);
    assert_eq!(source_index(&projected, outgoing[0].to), Some(1));
    assert_eq!(projected.counts.displayed_receipts, 1);
    assert_eq!(projected.counts.hidden_receipts, 1);
    assert_eq!(projected.counts.hidden_points, 588);
    let macro_relation = projected
        .model_relations
        .iter()
        .find(|relation| relation.kind == Some(api::EdgeKind::ProcessControl))
        .unwrap();
    let cfg = &report.minimum.as_ref().unwrap().summaries[scope.summary()]
        .relation
        .regional_cfg;
    let terminal = cfg
        .nodes
        .iter()
        .find(|node| node.id == macro_relation.to.origin.node)
        .unwrap();
    assert_eq!(
        terminal.frame.process.as_ref().unwrap().location,
        api::ProcessLocation::Block(599)
    );
    assert!(
        projected.target_sets[projected.model_set.unwrap()]
            .entries
            .iter()
            .any(|entry| entry.destination == Destination::Control(macro_relation.to))
    );
}

#[test]
fn pending_topology_is_untyped_model_membership_and_never_fallthrough() {
    let mut report = report("36565b36565b00");
    let scope = scope(&report);
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    let hub = cfg
        .nodes
        .iter_mut()
        .find(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == api::ProcessLocation::JumpTargets)
        })
        .unwrap();
    let id = hub.id;
    let targets = hub.successors.clone();
    assert!(!targets.is_empty());
    hub.transfer = None;
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    assert!(
        projection
            .edges
            .iter()
            .flat_map(|edge| &edge.origins)
            .all(|origin| !matches!(origin,
        FlowOrigin::Regional { from, .. } | FlowOrigin::Topology { from, .. } if *from == id))
    );
    for target in targets {
        assert!(
            projection
                .model_relations
                .iter()
                .any(|relation| relation.kind.is_none()
                    && relation.origin
                        == FlowOrigin::Topology {
                            from: id,
                            to: target
                        })
        );
    }
    assert!(projection.counts.retained_topology > 0);
}

fn precompile_report() -> api::AnalysisReport {
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "60006000600060006000600161fffff100".into(),
            address: Some(format!("0x{}0101", "00".repeat(18))),
        }),
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    report
}

#[test]
fn actual_call_delivery_binds_callsite_and_continuation_and_static_position_creates_no_edge() {
    let mut report = precompile_report();
    let scope = scope(&report);
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let caller = block(&projection, 0);
    assert!(
        projection
            .edges
            .iter()
            .filter(|edge| edge.from == caller)
            .all(|edge| matches!(
                edge.kind,
                api::EdgeKind::Call | api::EdgeKind::Process(api::ProcessAction::Call)
            ) && edge.level == EdgeLevel::CallBoundary)
    );
    let deliveries: Vec<_> = projection
        .edges
        .iter()
        .filter(|edge| {
            edge.origins
                .iter()
                .any(|origin| matches!(origin, FlowOrigin::Delivery { .. }))
        })
        .collect();
    assert!(!deliveries.is_empty());
    assert!(
        deliveries
            .iter()
            .all(|edge| edge.level == EdgeLevel::CallBoundary
                && source_index(&projection, edge.to) == Some(1))
    );
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    let from = cfg.calls[0].from;
    // Retain actual admitted continuation IDs but remove their local-action
    // and delivered receipts. This is a display counterexample for historical
    // membership, not a new producer certificate or a typed Fallthrough.
    let body = cfg.nodes.iter_mut().find(|node| node.id == from).unwrap();
    assert!(!body.successors.is_empty());
    let transfer = body.transfer.as_mut().unwrap();
    transfer.successors.clear();
    transfer.complete = false;
    for call in &mut cfg.calls {
        call.deliveries.clear();
    }
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    assert!(
        projection
            .edges
            .iter()
            .flat_map(|edge| &edge.origins)
            .all(|origin| !matches!(origin, FlowOrigin::Delivery { .. }))
    );
    assert!(
        projection
            .edges
            .iter()
            .all(|edge| edge.kind != api::EdgeKind::Fallthrough)
    );
    assert!(
        projection
            .model_relations
            .iter()
            .any(|relation| relation.kind.is_none()
                && matches!(relation.origin, FlowOrigin::Topology { .. }))
    );
}

#[test]
fn independent_body_and_subscriber_clocks_retain_current_call_and_delivery() {
    let mut report = precompile_report();
    let scope = scope(&report);
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let via = projection
        .model_relations
        .iter()
        .map(|relation| relation.origin)
        .find(|origin| {
            matches!(origin, FlowOrigin::Delivery { .. })
                && origin.qualified_current(&report, scope.summary())
        })
        .unwrap();
    let FlowOrigin::Delivery { call, delivery, .. } = via else {
        unreachable!()
    };
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    let subscriber = cfg.calls.iter_mut().find(|row| row.id == call).unwrap();
    let body = cfg
        .nodes
        .iter()
        .find(|node| node.id == subscriber.from)
        .unwrap();
    let stamp = api::DecimalU64(body.input_revision.0 + 101);
    subscriber.input_revision = stamp;
    for receipt in &mut subscriber.deliveries {
        if receipt.current {
            receipt.input_revision = stamp;
        }
    }
    // A validated display counterexample: the public API permits independent
    // clocks. No solver stamp or source certification is produced by this test.
    report.validate().unwrap();
    assert!(FlowOrigin::CallSite { call }.qualified_current(&report, scope.summary()));
    assert!(via.qualified_current(&report, scope.summary()));
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    model.summaries[scope.summary()]
        .relation
        .regional_cfg
        .calls
        .iter_mut()
        .find(|row| row.id == call)
        .unwrap()
        .deliveries[delivery]
        .input_revision = api::DecimalU64(stamp.0 + 1);
    assert!(!via.qualified_current(&report, scope.summary()));
}

#[test]
fn opaque_callee_has_an_abstract_boundary_graph_and_no_source_edge_claim() {
    let report = precompile_report();
    let scope = scope(&report);
    let projection = GraphProjection::build(&report, scope).unwrap();
    let target = projection
        .model_relations
        .iter()
        .find(|relation| matches!(relation.origin, FlowOrigin::Call { .. }))
        .unwrap()
        .to;
    let boundary = Control::scoped(
        &report,
        GraphScope::Context {
            summary: target.summary,
            procedure: target.procedure,
            anchor: Some(ControlOrigin {
                node: target.origin.node,
                location: None,
            }),
        },
    )
    .unwrap();
    assert_eq!(boundary.level, GraphLevel::AbstractBoundary);
    assert!(
        boundary
            .edges
            .iter()
            .all(|edge| edge.level != EdgeLevel::Source)
    );
}

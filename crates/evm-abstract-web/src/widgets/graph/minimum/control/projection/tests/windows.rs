use super::super::super::scope::{
    CardKind, Destination, EdgeLevel, FlowOrigin, FrontierDirection, GraphProjection, GraphWindow,
    ProjectionError,
};
use super::{report, scope};
use evm_abstract_protocol as api;

fn point(graph: &GraphProjection, block: usize) -> usize {
    graph
        .cards
        .iter()
        .position(|card| {
            matches!(card.kind,
        CardKind::Source { location: api::ProcessLocation::Block(index), .. } if index == block)
        })
        .unwrap()
}

#[test]
fn original_weth_entry_has_only_forward_core_and_keeps_all_scope_evidence() {
    let report = report(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layout/ethereum-weth9.hex"
        ))
        .trim(),
    );
    let scope = scope(&report);
    let full = GraphProjection::with_window(&report, scope, GraphWindow::FullSource).unwrap();
    let entry = GraphProjection::build(&report, scope).unwrap();
    assert_eq!(entry.window, GraphWindow::Entry);
    let mut blocks: Vec<_> = entry
        .cards
        .iter()
        .filter_map(|card| match card.kind {
            CardKind::Source {
                location: api::ProcessLocation::Block(index),
                ..
            } => Some(index),
            _ => None,
        })
        .collect();
    blocks.sort_unstable();
    assert_eq!(blocks, [0, 1, 12]);
    assert_eq!(entry.cards.len(), 3);
    assert_eq!(entry.core.len(), 3);
    assert!(entry.cards.iter().all(|card| card.kind != CardKind::Folded));
    assert_eq!(entry.model_relations, full.model_relations);
    assert_eq!(entry.counts.total_points, full.counts.total_points);
    assert_eq!(entry.counts.context_receipts, full.counts.context_receipts);
    assert_eq!(
        &entry.target_sets[..full.target_sets.len()],
        full.target_sets.as_slice()
    );
    let hidden = &entry.target_sets[entry.hidden_set.unwrap()];
    for origin in full.cards.iter().flat_map(|card| &card.origins) {
        assert!(
            entry
                .cards
                .iter()
                .flat_map(|card| &card.origins)
                .any(|shown| shown == origin)
                || hidden
                    .entries
                    .iter()
                    .any(|row| row.destination == Destination::Control(origin.target))
        );
    }
}

#[test]
fn incoming_and_outgoing_crossings_are_neutral_and_keep_exact_receipt_associations() {
    let report = report("6003565b6007565b00");
    let scope = scope(&report);
    let mut full = GraphProjection::with_window(&report, scope, GraphWindow::FullSource).unwrap();
    let middle = full.cards[point(&full, 1)].origins[0].target;
    // The boundary point has a separate actual Body and an origin obligation.
    // Reverse its rows: a first-member fallback would now choose the wrong ID.
    let boundary = point(&full, 2);
    assert_eq!(full.cards[boundary].origins.len(), 2);
    full.cards[boundary].origins.reverse();
    let entry = super::super::window::project(&report, full, GraphWindow::Entry).unwrap();
    assert_eq!(entry.cards.len(), 2);
    assert!(entry.cards.iter().all(|card| card.kind != CardKind::Folded));
    let outgoing = entry
        .frontiers
        .iter()
        .find(|row| row.direction == FrontierDirection::Outgoing)
        .unwrap();
    let flow = outgoing.flows[0];
    assert_eq!(flow.level, EdgeLevel::Source);
    assert_eq!(flow.kind, api::EdgeKind::Jump);
    assert_eq!(flow.from, middle);
    assert_eq!(flow.to.origin.node, middle.origin.node);
    assert_eq!(
        flow.to.origin.location,
        Some(api::ProcessLocation::Block(2))
    );
    assert!(
        entry.target_sets[outgoing.target_set]
            .entries
            .iter()
            .any(|row| row.destination == Destination::Control(flow.to) && row.via == flow.origin)
    );
    let around_middle = GraphProjection::build(&report, middle.scope()).unwrap();
    assert_eq!(around_middle.cards.len(), 2);
    assert!(
        around_middle
            .cards
            .iter()
            .all(|card| card.kind != CardKind::Folded)
    );
    let incoming = around_middle
        .frontiers
        .iter()
        .find(|row| row.direction == FrontierDirection::Incoming)
        .unwrap();
    assert_eq!(incoming.card, around_middle.anchor.unwrap());
    assert_eq!(incoming.flows[0].to, middle);
    assert_eq!(incoming.flows[0].from.origin.node, middle.origin.node);
    assert_eq!(incoming.flows[0].from.origin.location, None);
    assert_eq!(around_middle.counts.incoming_flows, 1);
    assert!(
        around_middle.target_sets[incoming.target_set]
            .entries
            .iter()
            .any(|row| row.destination == Destination::Control(incoming.flows[0].from))
    );
}

#[test]
fn full_source_includes_disconnected_pending_scope_points_and_original_target_sets() {
    let mut report = report("36565b36565b00");
    let scope = scope(&report);
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    cfg.nodes
        .iter_mut()
        .find(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == api::ProcessLocation::JumpTargets)
        })
        .unwrap()
        .transfer = None;
    let nearby = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let full = GraphProjection::with_window(&report, scope, GraphWindow::FullSource).unwrap();
    assert!(nearby.counts.hidden_points > 0);
    assert_eq!(full.counts.displayed_points, full.counts.total_points);
    assert_eq!(full.counts.hidden_points, 0);
    assert_eq!(full.counts.hidden_receipts, 0);
    assert!(full.hidden_set.is_none());
    assert!(full.frontiers.is_empty());
    assert!(full.cards.len() > nearby.cards.len());
    assert!(full.cards.iter().any(|card| matches!(
        card.kind,
        CardKind::Source {
            location: api::ProcessLocation::Block(2),
            ..
        }
    )));
    assert_eq!(full.model_relations, nearby.model_relations);
    assert_eq!(
        &nearby.target_sets[..full.target_sets.len()],
        full.target_sets.as_slice()
    );
    assert!(
        full.cards
            .iter()
            .flat_map(|card| &card.origins)
            .any(|origin| !origin.transferred && !origin.through)
    );
}

#[test]
fn missing_crossing_origin_is_rejected_instead_of_manufacturing_node_zero() {
    let report = report("6003565b6007565b00");
    let mut full =
        GraphProjection::with_window(&report, scope(&report), GraphWindow::FullSource).unwrap();
    let from = point(&full, 1);
    let edge = full
        .edges
        .iter_mut()
        .find(|edge| edge.from == from)
        .unwrap();
    let FlowOrigin::Source { node, .. } = &mut edge.origins[0] else {
        unreachable!()
    };
    *node = usize::MAX;
    assert_eq!(
        super::super::window::project(&report, full, GraphWindow::Entry).unwrap_err(),
        ProjectionError::MissingFlowOrigin
    );
}

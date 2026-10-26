use super::super::scope::{
    CardKind, ControlOrigin, DEFAULT_POINT_LIMIT, Destination, FlowOrigin, GraphProjection,
    GraphScope, ProjectionCard, ProjectionError, TargetSetKind,
};
use evm_abstract_protocol as api;

fn report(code: &str) -> api::AnalysisReport {
    // Keep the real UI defaults, including the original work/region policy.
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: None,
        }),
        ..Default::default()
    };
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    report
}

fn scope(report: &api::AnalysisReport) -> GraphScope {
    GraphScope::Entry {
        summary: crate::app::minimum::initial_summary(report.minimum.as_ref().unwrap()),
    }
}

fn source(projection: &GraphProjection, location: api::ProcessLocation) -> Option<&ProjectionCard> {
    projection.cards.iter().find(
        |card| matches!(card.kind, CardKind::Source { location: point, .. } if point == location),
    )
}

#[test]
fn grouped_source_keeps_boundary_obligations_separate_from_body_receipts() {
    let report = report("6003565b6007565b00");
    let scope = scope(&report);
    let projection = GraphProjection::build(&report, scope).unwrap();
    let boundary = source(&projection, api::ProcessLocation::Block(2)).unwrap();
    assert!(
        boundary
            .origins
            .iter()
            .any(|origin| origin.boundary && origin.target.origin.location.is_some())
    );
    assert!(
        boundary
            .origins
            .iter()
            .any(|origin| origin.target.origin.location.is_none())
    );
    assert!(
        boundary
            .origins
            .iter()
            .all(|origin| origin.target.summary == scope.summary())
    );
    assert_eq!(projection.counts.hidden_points, 0);
    assert!(
        projection
            .edges
            .iter()
            .flat_map(|edge| &edge.origins)
            .any(|origin| matches!(
                origin,
                FlowOrigin::Source {
                    from: api::ProcessLocation::Block(0),
                    to: api::ProcessLocation::Block(1),
                    ..
                }
            ))
    );
    let cfg = &report.minimum.as_ref().unwrap().summaries[scope.summary()]
        .relation
        .regional_cfg;
    for card in &projection.cards {
        for origin in &card.origins {
            let node = cfg
                .nodes
                .iter()
                .find(|node| node.id == origin.target.origin.node)
                .unwrap();
            assert_eq!(node.procedure, origin.target.procedure);
            assert!(node.frame.basic_block.is_none());
            assert!(
                node.transfer
                    .as_ref()
                    .is_none_or(|transfer| !transfer.visited_block)
            );
        }
    }
}

#[test]
fn unknown_jump_is_one_inspectable_set_with_real_loop_provenance() {
    let report = report("36565b36565b00");
    let projection = GraphProjection::with_limit(&report, scope(&report), 100).unwrap();
    let hub = source(&projection, api::ProcessLocation::JumpTargets).unwrap();
    let targets = &projection.target_sets[hub.target_set.unwrap()];
    assert!(matches!(targets.kind, TargetSetKind::JumpTargets { .. }));
    let mut legal = targets
        .entries
        .iter()
        .map(|entry| match entry.via {
            FlowOrigin::Jump { target, .. } => target,
            _ => panic!("jump set must retain its exact hub/target association"),
        })
        .collect::<Vec<_>>();
    legal.sort_unstable();
    legal.dedup();
    assert_eq!(legal, [1, 2]);
    assert!(
        projection
            .edges
            .iter()
            .flat_map(|edge| &edge.origins)
            .any(|origin| matches!(
                origin,
                FlowOrigin::Source {
                    from: api::ProcessLocation::Block(1),
                    to: api::ProcessLocation::JumpTargets,
                    ..
                }
            ))
    );
    assert_eq!(
        projection
            .cards
            .iter()
            .filter(|card| matches!(
                card.kind,
                CardKind::Source {
                    location: api::ProcessLocation::JumpTargets,
                    ..
                }
            ))
            .count(),
        1
    );
}

#[test]
fn six_hundred_block_source_is_bounded_and_every_frontier_is_navigable() {
    let report = report(
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/layout/long-linear-chain-600.hex"
        ))
        .trim(),
    );
    let scope = scope(&report);
    let full = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let projected = GraphProjection::build(&report, scope).unwrap();
    assert_eq!(full.counts.total_points, 600);
    assert_eq!(projected.counts.displayed_points, DEFAULT_POINT_LIMIT);
    assert_eq!(projected.counts.hidden_points, 600 - DEFAULT_POINT_LIMIT);
    assert!(projected.cards.len() <= DEFAULT_POINT_LIMIT + 2);
    let frontier = projected
        .target_sets
        .iter()
        .find(|set| set.kind == TargetSetKind::Folded)
        .unwrap();
    assert!(!frontier.entries.is_empty());
    for entry in &frontier.entries {
        let Destination::Control(target) = entry.destination else {
            panic!("retained source frontier has a regional origin")
        };
        let next = GraphProjection::build(&report, target.scope()).unwrap();
        let anchor = &next.cards[next.anchor.unwrap()];
        assert!(anchor.origins.iter().any(|origin| origin.target == target));
        assert!(next.counts.displayed_points <= DEFAULT_POINT_LIMIT);
    }
    let hidden = &projected.target_sets[projected.hidden_set.unwrap()];
    let original_origins = full.cards.iter().flat_map(|card| &card.origins).count();
    let shown_origins = projected
        .cards
        .iter()
        .flat_map(|card| &card.origins)
        .count();
    assert_eq!(hidden.entries.len(), original_origins - shown_origins);
    // Every original origin is either displayed or explicitly selectable in
    // omitted scope; no synthetic receipt fills gaps in the window.
    for origin in full.cards.iter().flat_map(|card| &card.origins) {
        assert!(
            projected
                .cards
                .iter()
                .flat_map(|card| &card.origins)
                .any(|shown| shown.target == origin.target)
                || hidden
                    .entries
                    .iter()
                    .any(|entry| entry.destination == Destination::Control(origin.target))
        );
    }
    for flow in projected.edges.iter().flat_map(|edge| &edge.origins) {
        assert!(
            full.edges
                .iter()
                .flat_map(|edge| &edge.origins)
                .any(|original| original == flow)
        );
    }
}

#[test]
fn same_source_under_another_owner_or_static_mode_is_not_grouped() {
    let mut report = report("6003565b6007565b00");
    let scope = scope(&report);
    let baseline = GraphProjection::build(&report, scope).unwrap();
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    let mut other = cfg
        .nodes
        .iter()
        .find(|node| node.frame.process.is_some())
        .unwrap()
        .clone();
    other.id = 10_007;
    let frame = other.frame.process.as_mut().unwrap();
    frame.owner = api::ProcessOwner::Residual;
    frame.is_static = !frame.is_static;
    let other_procedure = other.procedure;
    cfg.nodes.push(other);
    let projected = GraphProjection::build(&report, scope).unwrap();
    assert_eq!(
        projected.counts.displayed_points,
        baseline.counts.displayed_points
    );
    assert_eq!(
        projected.counts.other_context_receipts,
        baseline.counts.other_context_receipts + 1
    );
    assert!(
        projected
            .cards
            .iter()
            .flat_map(|card| &card.origins)
            .all(|origin| origin.target.origin.node != 10_007)
    );
    let bad = GraphScope::Context {
        summary: scope.summary(),
        procedure: other_procedure,
        anchor: Some(ControlOrigin {
            node: 10_007,
            location: None,
        }),
    };
    assert_eq!(
        GraphProjection::build(&report, bad).unwrap_err(),
        ProjectionError::InvalidAnchor
    );
}

#[test]
fn full_weth_source_uses_original_defaults_and_source_grouping_instead_of_receipt_copies() {
    let code = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/layout/ethereum-weth9.hex"
    ))
    .trim();
    let report = report(code);
    let scope = scope(&report);
    let old = super::super::Control::new(&report, scope.summary());
    let projected = GraphProjection::build(&report, scope).unwrap();
    let control = super::super::Control::scoped(&report, scope).unwrap();
    assert!(
        old.cards.len() > 100,
        "full WETH must exercise the original large projection"
    );
    assert!(projected.counts.total_points > DEFAULT_POINT_LIMIT);
    assert!(projected.counts.displayed_points <= DEFAULT_POINT_LIMIT);
    assert!(projected.counts.hidden_points > 0);
    assert!(control.cards.len() < old.cards.len());
    assert!(
        control
            .cards
            .iter()
            .filter(|card| matches!(
                card.kind,
                CardKind::Source {
                    location: api::ProcessLocation::Block(_),
                    ..
                }
            ))
            .all(|card| card.title.starts_with("0x") && !card.title.contains("procedure"))
    );
    for card in &projected.cards {
        if let Some(origin) = card.origins.first() {
            let target_scope = origin.target.scope();
            let reopened = GraphProjection::build(&report, target_scope).unwrap();
            assert!(
                reopened.cards[reopened.anchor.unwrap()]
                    .origins
                    .iter()
                    .any(|row| row.target == origin.target)
            );
        }
    }
}

#[test]
fn unadmitted_coordinates_and_wrong_epoch_directory_are_rejected() {
    let mut report = report("6003565b6007565b00");
    let initial = GraphProjection::build(&report, scope(&report)).unwrap();
    let origin = initial.cards[initial.anchor.unwrap()].origins[0].target;
    let bad = GraphScope::Context {
        summary: origin.summary,
        procedure: origin.procedure,
        anchor: Some(ControlOrigin {
            node: origin.origin.node,
            location: Some(api::ProcessLocation::Block(99_999)),
        }),
    };
    assert_eq!(
        GraphProjection::build(&report, bad).unwrap_err(),
        ProjectionError::InvalidAnchor
    );
    let relation = std::sync::Arc::make_mut(&mut report.process_controls[0]);
    relation.epoch = api::ProcessDigest([0x77; 32]);
    assert_eq!(
        GraphProjection::build(&report, scope(&report)).unwrap_err(),
        ProjectionError::SourceMismatch
    );
}

#[test]
fn historical_receipts_remain_visible_without_becoming_current() {
    let mut report = report("6003565b6007565b00");
    let summary = scope(&report).summary();
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let later = model.summaries[summary].clone();
    model.summaries.push(later);
    let projection = GraphProjection::build(&report, GraphScope::Entry { summary }).unwrap();
    assert_eq!(
        projection.source_status,
        api::SummarySourceStatus::Historical
    );
    assert!(
        projection
            .cards
            .iter()
            .flat_map(|card| &card.origins)
            .all(|origin| !origin.current)
    );
}

#[test]
fn call_portals_preserve_target_delivery_and_rollback_associations() {
    let report = report(
        "6000600060006000600061020161fffff1600060006000600061020161fffffa600060006000600061020161fffff400",
    );
    let scope = scope(&report);
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let cfg = &report.minimum.as_ref().unwrap().summaries[scope.summary()]
        .relation
        .regional_cfg;
    for set in projection
        .target_sets
        .iter()
        .filter(|set| matches!(set.kind, TargetSetKind::Calls { .. }))
    {
        for entry in &set.entries {
            match entry.via {
                FlowOrigin::Call {
                    call,
                    entry: target,
                } => {
                    let call = cfg.calls.iter().find(|row| row.id == call).unwrap();
                    assert!(call.entries.contains(&target));
                    let Destination::Control(destination) = entry.destination else {
                        panic!()
                    };
                    assert_eq!(destination.procedure, call.callee);
                    assert_eq!(destination.origin.node, target);
                }
                FlowOrigin::Delivery {
                    call,
                    delivery,
                    successor,
                } => {
                    let call = cfg.calls.iter().find(|row| row.id == call).unwrap();
                    assert!(
                        call.deliveries[delivery]
                            .successors
                            .iter()
                            .any(|target| target.to == successor)
                    );
                }
                _ => panic!("callee and delivered continuation targets must stay distinguished"),
            }
        }
    }
    assert!(projection.counts.calls > 0);
    assert!(
        projection
            .target_sets
            .iter()
            .any(|set| matches!(set.kind, TargetSetKind::Calls { .. }))
    );
}

#[test]
fn a_retained_delivery_needs_its_own_current_input_and_output_generation() {
    // A physically bound root calling a known precompile has a stable source;
    // the unknown-callee fixture above deliberately includes other observations.
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "60006000600060006000600161fffff100".into(),
            address: Some(format!("0x{}0101", "00".repeat(18))),
        }),
        ..Default::default()
    };
    let mut report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    let scope = scope(&report);
    let projection = GraphProjection::with_limit(&report, scope, usize::MAX).unwrap();
    let via = projection
        .target_sets
        .iter()
        .flat_map(|set| &set.entries)
        .map(|entry| entry.via)
        .find(|via| {
            matches!(via, FlowOrigin::Delivery { .. })
                && via.qualified_current(&report, scope.summary())
        })
        .unwrap_or_else(|| {
            let observation = &report.minimum.as_ref().unwrap().summaries[scope.summary()];
            panic!("an actual source-qualified current delivery: source {:?}; calls {:#?}; nodes {:#?}; bounds {:#?}",
                projection.source_status, observation.relation.regional_cfg.calls, observation.relation.regional_cfg.nodes, observation.bounds)
        });
    let details = via.detail(&report, scope.summary());
    assert!(details.contains("output generation"));
    assert!(details.contains("qualified current true"));
    let FlowOrigin::Delivery { call, delivery, .. } = via else {
        unreachable!()
    };
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    cfg.calls
        .iter_mut()
        .find(|row| row.id == call)
        .unwrap()
        .deliveries[delivery]
        .output_generation = usize::MAX;
    assert!(!via.qualified_current(&report, scope.summary()));
    assert!(
        via.detail(&report, scope.summary())
            .contains("qualified current false")
    );
}

#[test]
fn pending_unknown_jump_exposes_decoded_candidates_without_a_fake_expansion() {
    let mut report = report("36565b36565b00");
    let scope = scope(&report);
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let cfg = &mut model.summaries[scope.summary()].relation.regional_cfg;
    // A pending-display collision control over real retained topology, not a
    // newly certified closure: remove only the hub's actual transfer receipt.
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
    hub.transfer = None;
    let projection = GraphProjection::build(&report, scope).unwrap();
    let hub = source(&projection, api::ProcessLocation::JumpTargets).unwrap();
    assert!(
        hub.origins
            .iter()
            .all(|origin| !origin.through && !origin.transferred)
    );
    assert!(projection.counts.unexpanded_points > 0);
    let set = &projection.target_sets[hub.target_set.unwrap()];
    assert_eq!(set.entries.len(), 2);
    for entry in &set.entries {
        assert!(matches!(entry.destination, Destination::Source { .. }));
        assert!(matches!(entry.via, FlowOrigin::DecodedJump { .. }));
        assert!(!entry.via.qualified_current(&report, scope.summary()));
    }
    assert!(
        projection
            .edges
            .iter()
            .flat_map(|edge| &edge.origins)
            .all(|origin| !matches!(
                origin,
                FlowOrigin::Jump { .. } | FlowOrigin::DecodedJump { .. }
            ))
    );
}

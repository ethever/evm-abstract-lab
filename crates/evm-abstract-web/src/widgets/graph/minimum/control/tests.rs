use super::*;
use evm_abstract_protocol as api;

fn report(code: &str) -> api::AnalysisReport {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: code.into(),
            address: None,
        }),
        ..Default::default()
    };
    request.limits.root_terminal_coverage = false;
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    report
}

#[test]
fn real_pure_interiors_render_source_edges_but_do_not_expand_unadmitted_code() {
    let report = report("6003565b6007565b00");
    let model = report.minimum.as_ref().unwrap();
    let summary = crate::app::minimum::initial_summary(model);
    let control = Control::new(&report, summary);
    let cfg = &model.summaries[summary].relation.regional_cfg;
    let node = cfg
        .nodes
        .iter()
        .position(|node| {
            node.transfer
                .as_ref()
                .and_then(|t| t.process.as_ref())
                .is_some_and(|p| p.control.is_some())
        })
        .unwrap();
    let from = control
        .cards
        .iter()
        .position(|card| card.node == node && card.location.is_none())
        .unwrap();
    let interior = control
        .cards
        .iter()
        .position(|card| card.node == node && card.location == Some(ProcessLocation::Block(1)))
        .unwrap();
    let boundary = control
        .cards
        .iter()
        .position(|card| card.node == node && card.location == Some(ProcessLocation::Block(2)))
        .unwrap();
    assert!(
        control
            .edges
            .iter()
            .any(|edge| edge.from == from && edge.to == interior && edge.kind == EdgeKind::Jump)
    );
    assert!(
        control.edges.iter().any(|edge| edge.from == interior
            && edge.to == boundary
            && edge.kind == EdgeKind::Jump)
    );
    assert!(control.cards[boundary].boundary);
    assert!(control.cards[interior].caption.contains("source 0x0003"));
    assert!(
        control
            .edges
            .iter()
            .any(|edge| edge.kind == EdgeKind::ProcessControl)
    );
    // STOP is a separate actual Body transfer; directory presence cannot give
    // its boundary point an invented local continuation.
    assert!(!control.edges.iter().any(|edge| edge.from == boundary));
    assert!(cfg.nodes.iter().all(|node| node.frame.basic_block.is_none()
        && node.transfer.as_ref().is_none_or(|t| !t.visited_block)));
}

#[test]
fn real_unknown_jump_expands_only_the_actual_hub_receipt_and_retains_its_loop() {
    let report = report("36565b36565b00");
    let model = report.minimum.as_ref().unwrap();
    let summary = crate::app::minimum::initial_summary(model);
    let control = Control::new(&report, summary);
    let cfg = &model.summaries[summary].relation.regional_cfg;
    let hub_node = cfg
        .nodes
        .iter()
        .position(|node| {
            node.frame
                .process
                .as_ref()
                .is_some_and(|frame| frame.location == ProcessLocation::JumpTargets)
        })
        .unwrap();
    let hub = control
        .cards
        .iter()
        .position(|card| card.node == hub_node && card.location.is_none())
        .unwrap();
    let targets: Vec<_> = control
        .edges
        .iter()
        .filter(|edge| edge.from == hub && edge.label == "MAY legal JUMPDEST")
        .map(|edge| control.cards[edge.to].location.unwrap())
        .collect();
    assert_eq!(
        targets,
        [ProcessLocation::Block(1), ProcessLocation::Block(2)]
    );
    assert_eq!(control.hubs, 1);
    let loop_block = control
        .cards
        .iter()
        .position(|card| card.node == hub_node && card.location == Some(ProcessLocation::Block(1)))
        .unwrap();
    assert!(
        control
            .edges
            .iter()
            .any(|edge| edge.from == loop_block && edge.to == hub)
    );
    assert!(control.cards[hub].caption.contains("unknown-jump hub"));
    // Origins only admit the hub boundary; they do not each duplicate its
    // target fanout (which would form a Cartesian unknown-jump expansion).
    assert_eq!(
        control
            .edges
            .iter()
            .filter(|edge| edge.label == "MAY legal JUMPDEST")
            .count(),
        2
    );
}

#[test]
fn sparse_regional_id_and_same_source_under_another_owner_do_not_share_display_nodes() {
    let mut report = report("6003565b6007565b00");
    let model = std::sync::Arc::make_mut(report.minimum.as_mut().unwrap());
    let summary = crate::app::minimum::initial_summary(model);
    let cfg = &mut model.summaries[summary].relation.regional_cfg;
    // A renderer collision control over an actual retained receipt, not an
    // independent semantic certificate: vary table ID/owner/static only.
    let mut other = cfg
        .nodes
        .iter()
        .find(|node| {
            node.transfer
                .as_ref()
                .and_then(|t| t.process.as_ref())
                .is_some_and(|p| p.control.is_some())
        })
        .unwrap()
        .clone();
    let original_id = other.id;
    other.id = 10_007;
    let frame = other.frame.process.as_mut().unwrap();
    frame.owner = api::ProcessOwner::Residual;
    frame.is_static = !frame.is_static;
    cfg.nodes.push(other);
    let control = Control::new(&report, summary);
    let cfg = &report.minimum.as_ref().unwrap().summaries[summary]
        .relation
        .regional_cfg;
    let originals: Vec<_> = control
        .cards
        .iter()
        .enumerate()
        .filter(|(_, card)| cfg.nodes[card.node].id == original_id)
        .collect();
    let copies: Vec<_> = control
        .cards
        .iter()
        .enumerate()
        .filter(|(_, card)| cfg.nodes[card.node].id == 10_007)
        .collect();
    assert_eq!(originals.len(), copies.len());
    assert!(originals.len() > 1);
    for ((id, original), (other_id, copy)) in originals.iter().zip(&copies) {
        assert_ne!(id, other_id);
        assert_eq!(original.location, copy.location);
        assert_ne!(original.qualifier, copy.qualifier);
        assert!(copy.title.contains("r10007"));
    }
}

#[test]
fn actual_calls_and_joint_deliveries_keep_call_ids_and_do_not_invent_local_returns() {
    let mut request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "6000600060006000600061020161fffff1600060006000600061020161fffffa600060006000600061020161fffff400".into(),
            address: Some(format!("0x{}0101", "00".repeat(18))),
        }),
        ..Default::default()
    };
    request.limits.root_terminal_coverage = false;
    let report = evm_abstract_server::analyze::analyze(request).unwrap();
    report.validate().unwrap();
    let model = report.minimum.as_ref().unwrap();
    let summary = crate::app::minimum::initial_summary(model);
    let cfg = &model.summaries[summary].relation.regional_cfg;
    let control = Control::new(&report, summary);
    for action in [
        api::ProcessAction::Call,
        api::ProcessAction::StaticCall,
        api::ProcessAction::DelegateCall,
    ] {
        let call = cfg
            .calls
            .iter()
            .find(|call| call.action == Some(action))
            .expect("actual call-family receipt");
        for &entry in &call.entries {
            assert!(
                control
                    .edges
                    .iter()
                    .any(
                        |edge| cfg.nodes[control.cards[edge.from].node].id == call.from
                            && cfg.nodes[control.cards[edge.to].node].id == entry
                            && edge.label.starts_with(&format!("call c{} ·", call.id))
                    )
            );
        }
        assert!(!call.deliveries.is_empty());
        for delivery in &call.deliveries {
            for target in &delivery.successors {
                assert!(control.edges.iter().any(
                    |edge| cfg.nodes[control.cards[edge.from].node].id == call.from
                        && cfg.nodes[control.cards[edge.to].node].id == target.to
                        && edge.kind == target.kind
                        && edge.label
                            == format!(
                                "c{} {:?} · input {} · generation {}",
                                call.id,
                                delivery.kind,
                                delivery.input_revision,
                                delivery.output_generation
                            )
                ));
            }
        }
    }
    assert!(
        control
            .edges
            .iter()
            .all(|edge| !edge.label.contains("continuation")),
        "potential source continuation is not a delivered return"
    );
}

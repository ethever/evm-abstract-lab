//! The actual recursive engine result survives SSA and the browser protocol.
use evm_abstract::domain::NumericValue;

use super::*;
use evm_abstract::{U256, domain::EvmValue};

#[test]
fn closed_unknown_interactions_project_as_summaries_without_invented_opcodes() {
    let root = Address::repeat_byte(0x11);
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, "unknown static interaction projection");
    // STATICCALL missing code, then return the preserved owner slot.
    let code = "5f5f5f5f6102015afa505f545f5260205ff3";
    let expected_hash =
        alloy_primitives::keccak256(alloy_primitives::hex::decode(code).unwrap()).to_string();
    let mut account = EvmAccount::<NumericValue>::from_hex(code, Fork::Osaka).unwrap();
    account.storage.insert(
        U256::ZERO,
        EvmValue::<NumericValue>::constant(U256::from(42)),
    );
    world.insert(root, account).unwrap();
    let report = project_world(world, root);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.selected_graph().unwrap().frontiers.is_empty());
    assert!(
        !report
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .is_empty()
    );
    assert!(
        report
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .iter()
            .all(|summary| {
                summary.kind == api::ProcedureSummaryKind::UnknownEnvironment
                    && summary.root_procedure.is_some()
            })
    );
    assert!(
        report
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .iter()
            .all(|summary| summary.closed)
    );
    let root_address = root.to_string();
    let captured: Vec<_> = report
        .programs
        .iter()
        .filter(|program| {
            program.code_address.as_deref() == Some(root_address.as_str())
                && program.code_hash == expected_hash
                && program.fork == api::Fork::Osaka
                && program.kind == api::CodeKind::Runtime
        })
        .collect();
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].bytecode.trim_start_matches("0x"), code);
    let decoded_blocks: std::collections::BTreeSet<_> =
        captured[0].blocks.iter().map(|block| block.id).collect();
    assert_eq!(decoded_blocks, [0, 1].into_iter().collect());
    for summary in &report.selected_graph().unwrap().abstract_summaries {
        let environment = summary.root_procedure.unwrap();
        let root_procedure = &summary.procedures[environment];
        assert_eq!(root_procedure.kind, api::CodeKind::AbstractEnvironment);
        assert!(root_procedure.covered_blocks.is_empty());
        assert_eq!(
            root_procedure.code_address.as_deref(),
            Some("0x0000000000000000000000000000000000000201")
        );
        assert!(root_procedure.code_hash.is_none());
        // The unknown static implementation can call the one captured root.
        // Its real callback and recursive return dependency must survive export.
        assert_eq!(summary.procedures.len(), 2);
        let callback = 1 - environment;
        let callback_procedure = &summary.procedures[callback];
        assert_eq!(callback_procedure.kind, api::CodeKind::Runtime);
        assert_eq!(
            callback_procedure.code_address.as_deref(),
            Some(root_address.as_str())
        );
        assert_eq!(
            callback_procedure.code_hash.as_deref(),
            Some(expected_hash.as_str())
        );
        assert_eq!(
            callback_procedure.storage_address.as_deref(),
            Some(root_address.as_str())
        );
        assert_eq!(callback_procedure.covered_blocks, [0, 1]);
        assert!(root_procedure.is_static && callback_procedure.is_static);
        assert_eq!(root_procedure.dependencies, [callback]);
        assert_eq!(callback_procedure.dependencies, [environment]);
        assert!(!summary.regional_cfg.nodes.is_empty());
        assert!(!summary.regional_cfg.calls.is_empty());
        let mut environment_nodes = 0;
        let mut callback_blocks = std::collections::BTreeSet::new();
        for node in &summary.regional_cfg.nodes {
            let transfer = node.transfer.as_ref().unwrap();
            assert!(transfer.current && transfer.complete);
            if node.procedure == environment {
                environment_nodes += 1;
                assert!(node.frame.is_static);
                assert_eq!(node.frame.kind, api::CodeKind::AbstractEnvironment);
                assert!(node.frame.basic_block.is_none() && node.frame.code_hash.is_none());
                assert!(!transfer.visited_block);
                assert!(!transfer.calls.is_empty());
                assert!(
                    transfer
                        .calls
                        .iter()
                        .all(|call| { summary.regional_cfg.calls[*call].callee == callback })
                );
            } else {
                assert_eq!(node.procedure, callback);
                assert_eq!(node.frame.kind, api::CodeKind::Runtime);
                assert_eq!(node.frame.code_address, callback_procedure.code_address);
                assert_eq!(
                    node.frame.code_hash.as_deref(),
                    Some(expected_hash.as_str())
                );
                assert_eq!(
                    node.frame.storage_address.as_deref(),
                    Some(root_address.as_str())
                );
                assert!(node.frame.is_static);
                let block = node.frame.basic_block.unwrap();
                assert!(decoded_blocks.contains(&block));
                callback_blocks.insert(block);
                assert!(transfer.visited_block);
            }
        }
        assert!(environment_nodes > 0);
        assert_eq!(callback_blocks, [0, 1].into_iter().collect());
        for (caller, callee) in [(environment, callback), (callback, environment)] {
            let calls: Vec<_> = summary
                .regional_cfg
                .calls
                .iter()
                .filter(|call| {
                    summary.regional_cfg.nodes[call.from].procedure == caller
                        && call.callee == callee
                })
                .collect();
            assert!(!calls.is_empty());
            for call in calls {
                let origin = &summary.regional_cfg.nodes[call.from];
                assert!(origin.transfer.as_ref().unwrap().calls.contains(&call.id));
                assert!(!call.entries.is_empty());
                assert!(
                    call.entries
                        .iter()
                        .all(|entry| summary.regional_cfg.nodes[*entry].procedure == callee)
                );
                let deliveries: Vec<_> = call
                    .deliveries
                    .iter()
                    .filter(|delivery| delivery.current)
                    .collect();
                assert!(!deliveries.is_empty());
                assert!(
                    deliveries
                        .iter()
                        .any(|delivery| delivery.kind == api::OutcomeKind::Return)
                );
                for delivery in deliveries {
                    assert_eq!(delivery.input_revision, call.input_revision);
                    assert!(delivery.output_generation > 0);
                    assert!(!delivery.successors.is_empty());
                    assert!(
                        delivery
                            .successors
                            .iter()
                            .all(|edge| summary.regional_cfg.nodes[edge.to].procedure == caller)
                    );
                }
            }
        }
    }
    assert!(
        report
            .selected_graph()
            .unwrap()
            .edges
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::SummaryReturn)
    );
    let unknown_states: Vec<_> = report
        .selected_graph()
        .unwrap()
        .states
        .iter()
        .filter(|state| {
            state
                .entry
                .frames
                .last()
                .is_some_and(|frame| frame.kind == api::CodeKind::AbstractEnvironment)
        })
        .collect();
    assert!(!unknown_states.is_empty());
    for unknown in unknown_states {
        assert!(unknown.entry.frames.last().unwrap().program.is_none());
        let block = report
            .selected_graph()
            .unwrap()
            .cfg
            .iter()
            .find(|block| block.id == unknown.state)
            .unwrap();
        assert!(block.source_block().is_none());
        assert!(block.executed_pcs.is_empty());
    }
    report.validate().unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    parsed.validate().unwrap();
    assert_eq!(
        parsed.selected_graph().unwrap().abstract_summaries,
        report.selected_graph().unwrap().abstract_summaries
    );

    let mut invalid = parsed;
    let summary =
        &mut std::sync::Arc::make_mut(invalid.native.as_mut().unwrap()).abstract_summaries[0];
    let invalid_dependency = summary.procedures.len();
    summary.procedures[0].dependencies.push(invalid_dependency);
    assert!(matches!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Pool(
            api::PoolReferenceFailure {
                kind: api::PoolReferenceKind::AbstractProcedure,
                ..
            }
        ))
    ));
}

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
    let mut account =
        EvmAccount::<NumericValue>::from_hex("5f5f5f5f6102015afa505f545f5260205ff3", Fork::Osaka)
            .unwrap();
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
        assert_eq!(callback_procedure.covered_blocks, [0, 1]);
        assert!(root_procedure.is_static && callback_procedure.is_static);
        assert_eq!(root_procedure.dependencies, [callback]);
        assert_eq!(callback_procedure.dependencies, [environment]);
        for node in &summary.regional_cfg.nodes {
            let transfer = node.transfer.as_ref().unwrap();
            assert!(transfer.current && transfer.complete);
            if node.procedure == environment {
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
                assert!(node.frame.basic_block.is_some());
                assert!(transfer.visited_block);
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
    let unknown = report
        .selected_graph()
        .unwrap()
        .states
        .iter()
        .find(|state| {
            state
                .entry
                .frames
                .last()
                .is_some_and(|frame| frame.kind == api::CodeKind::AbstractEnvironment)
        })
        .unwrap();
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

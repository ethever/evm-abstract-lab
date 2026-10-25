//! A real known-code cycle keeps its execution identity through SSA and the wire.
use evm_abstract::domain::NumericValue;

use super::*;

#[test]
fn recursive_bytecode_projects_a_closed_relation_without_fabricating_instructions() {
    let root = Address::repeat_byte(0x11);
    let mut world =
        EvmWorld::<NumericValue>::new(Fork::Osaka, "known recursive procedure projection");
    // CALL ADDRESS with empty input, ignore the status, return 42. Call rejection
    // supplies the finite continuation even though there is no recursive base case.
    world
        .insert(
            root,
            EvmAccount::<NumericValue>::from_hex("5f5f5f5f5f305af150602a5f5260205ff3", Fork::Osaka)
                .unwrap(),
        )
        .unwrap();
    let report = project_world(world, root);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.selected_graph().unwrap().frontiers.is_empty());
    assert!(report.selected_graph().unwrap().ssa.complete);
    assert!(
        !report
            .selected_graph()
            .unwrap()
            .abstract_summaries
            .is_empty()
    );
    for summary in &report.selected_graph().unwrap().abstract_summaries {
        assert_eq!(summary.kind, api::ProcedureSummaryKind::KnownRecursion);
        assert!(summary.closed);
        let procedure = &summary.procedures[summary.root_procedure.unwrap()];
        assert_eq!(procedure.kind, api::CodeKind::Runtime);
        assert!(procedure.code_hash.is_some());
        assert!(!procedure.covered_blocks.is_empty());
        assert!(procedure.outcomes.contains(&api::OutcomeKind::Return));
        let state = report
            .selected_graph()
            .unwrap()
            .states
            .iter()
            .find(|state| state.state == summary.source_state)
            .unwrap();
        let frame = state.entry.frames.last().unwrap();
        assert_eq!(frame.kind, api::CodeKind::Runtime);
        assert!(frame.program.is_some());
        let block = report
            .selected_graph()
            .unwrap()
            .cfg
            .iter()
            .find(|block| block.id == state.state)
            .unwrap();
        assert!(
            !report
                .source_block(block.source_block().unwrap())
                .unwrap()
                .instructions
                .is_empty()
        );
        assert!(block.executed_pcs.is_empty());
    }
    assert!(
        report
            .selected_graph()
            .unwrap()
            .edges
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::SummaryReturn)
    );
    assert!(
        report
            .selected_graph()
            .unwrap()
            .states
            .iter()
            .all(|state| state.entry.frames.len() <= 3)
    );
    assert!(
        report
            .selected_graph()
            .unwrap()
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == api::OutcomeKind::Return)
    );
    report.validate().unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    parsed.validate().unwrap();
    assert_eq!(
        parsed.selected_graph().unwrap().abstract_summaries,
        report.selected_graph().unwrap().abstract_summaries
    );
    let mut invalid = parsed;
    std::sync::Arc::make_mut(invalid.native.as_mut().unwrap()).abstract_summaries[0]
        .root_procedure = Some(
        std::sync::Arc::make_mut(invalid.native.as_mut().unwrap()).abstract_summaries[0]
            .procedures
            .len(),
    );
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

//! A real known-code cycle keeps its execution identity through SSA and the wire.

use super::*;

#[test]
fn recursive_bytecode_projects_a_closed_relation_without_fabricating_instructions() {
    let root = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "known recursive procedure projection");
    // CALL ADDRESS with empty input, ignore the status, return 42. Call rejection
    // supplies the finite continuation even though there is no recursive base case.
    world
        .insert(
            root,
            Account::from_hex("5f5f5f5f5f305af150602a5f5260205ff3", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let report = project_world(world, root);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(report.ssa.complete);
    assert!(!report.abstract_summaries.is_empty());
    for summary in &report.abstract_summaries {
        assert_eq!(summary.kind, api::ProcedureSummaryKind::KnownRecursion);
        assert!(summary.closed);
        let procedure = &summary.procedures[summary.root_procedure.unwrap()];
        assert_eq!(procedure.kind, api::CodeKind::Runtime);
        assert!(procedure.code_hash.is_some());
        assert!(!procedure.covered_blocks.is_empty());
        assert!(procedure.outcomes.contains(&api::OutcomeKind::Return));
        let state = report
            .states
            .iter()
            .find(|state| state.state == summary.source_state)
            .unwrap();
        let frame = state.entry.frames.last().unwrap();
        assert_eq!(frame.kind, api::CodeKind::Runtime);
        assert!(frame.program.is_some());
        let block = report
            .cfg
            .iter()
            .find(|block| block.id == state.state)
            .unwrap();
        assert!(!block.instructions.is_empty());
        assert!(block.executed_pcs.is_empty());
    }
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::SummaryReturn)
    );
    assert!(
        report
            .states
            .iter()
            .all(|state| state.entry.frames.len() <= 3)
    );
    assert!(
        report
            .outcomes
            .iter()
            .any(|outcome| outcome.kind == api::OutcomeKind::Return)
    );
    report.validate().unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.abstract_summaries, report.abstract_summaries);
    let mut invalid = parsed;
    invalid.abstract_summaries[0].root_procedure =
        Some(invalid.abstract_summaries[0].procedures.len());
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

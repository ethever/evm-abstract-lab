//! The actual recursive engine result survives SSA and the browser protocol.

use super::*;
use evm_abstract::{U256, domain::AbstractValue};

#[test]
fn closed_unknown_interactions_project_as_summaries_without_invented_opcodes() {
    let root = Address::repeat_byte(0x11);
    let mut world = World::new(Fork::Osaka, "unknown static interaction projection");
    // STATICCALL missing code, then return the preserved owner slot.
    let mut account =
        Account::from_hex("5f5f5f5f6102015afa505f545f5260205ff3", Fork::Osaka).unwrap();
    account
        .storage
        .insert(U256::ZERO, AbstractValue::constant(U256::from(42)));
    world.insert(root, account).unwrap();
    let report = project_world(world, root);
    assert_eq!(report.status, api::AnalysisStatus::Converged);
    assert!(report.frontiers.is_empty());
    assert!(!report.abstract_summaries.is_empty());
    assert!(report.abstract_summaries.iter().all(|summary| {
        summary.kind == api::ProcedureSummaryKind::UnknownEnvironment
            && summary.root_procedure.is_some()
    }));
    assert!(
        report
            .abstract_summaries
            .iter()
            .all(|summary| summary.closed)
    );
    assert!(
        report
            .abstract_summaries
            .iter()
            .all(|summary| summary.procedures.len() == 1
                && summary.procedures[0].kind == api::CodeKind::AbstractEnvironment
                && summary.procedures[0].covered_blocks.is_empty()
                && summary.procedures[0].dependencies.is_empty())
    );
    assert!(
        report
            .edges
            .iter()
            .any(|edge| edge.kind == api::EdgeKind::SummaryReturn)
    );
    let unknown = report
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
        .cfg
        .iter()
        .find(|block| block.id == unknown.state)
        .unwrap();
    assert!(block.instructions.is_empty());
    assert!(block.executed_pcs.is_empty());
    report.validate().unwrap();
    let encoded = serde_json::to_vec(&report).unwrap();
    let parsed: api::AnalysisReport = serde_json::from_slice(&encoded).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed.abstract_summaries, report.abstract_summaries);

    let mut invalid = parsed;
    let summary = &mut invalid.abstract_summaries[0];
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

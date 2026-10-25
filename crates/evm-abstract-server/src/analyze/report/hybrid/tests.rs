//! Reusing a publication retains its exact graph and source identities.
use super::*;
use crate::analyze::input;
use evm_abstract::{
    analysis::{AnalysisEngine, EnginePolicies, HybridConfig, cooperation::NoExtension},
    domain::{NumericValue, ProductAlgebra},
};
use std::sync::Arc;

fn fixture() -> (
    analysis::HybridAnalysis<NumericValue>,
    api::AnalyzeRequest,
    ValueAlgebra<ProductAlgebra>,
) {
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        input::spec(&request.limits).unwrap(),
    ));
    let api::AnalysisInput::Bytecode(source) = &request.input else {
        unreachable!()
    };
    let source = input::bytecode(
        source,
        input::fork(request.fork),
        input::environment(&request.environment).unwrap(),
    )
    .unwrap();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let report = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            source,
            input::config(&request.limits).unwrap(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    assert!(report.coverage().baseline_current);
    assert!(report.detail().is_none());
    (report, request, algebra)
}

#[test]
fn repeated_publication_keeps_exact_global_and_catalog_under_one_allowance() {
    let (source, request, algebra) = fixture();
    let control = Control::default();
    let input = ProjectionInput {
        request: &request,
        scope: api::AnalysisScope::SingleProgram,
        numeric_model: api::NumericModel::Product,
    };
    let mut measurement = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let measured =
        project_with_retained_global(&source, &algebra, &control, &mut measurement, input, None)
            .unwrap();
    let first_work = measurement.used();
    let handle_work = measured.programs.len() + 1;
    let allowance = first_work + handle_work;
    assert!(allowance < first_work * 2);
    // This is the actual single ledger for both publications. No refusal is
    // cleared and no second execution/export allowance is introduced.
    let mut budget = WorkBudget::new(allowance);
    let early = project_with_retained_global(&source, &algebra, &control, &mut budget, input, None)
        .unwrap();
    assert_eq!(budget.used(), first_work);
    let final_report = project_with_retained_global(
        &source,
        &algebra,
        &control,
        &mut budget,
        input,
        Some(&early),
    )
    .unwrap();
    assert_eq!(budget.used(), allowance);
    assert!(!budget.exhausted());
    assert!(Arc::ptr_eq(
        early.global.as_ref().unwrap(),
        final_report.global.as_ref().unwrap()
    ));
    for (old, new) in early.programs.iter().zip(&final_report.programs) {
        assert!(Arc::ptr_eq(old, new));
        assert_eq!(old.id, new.id);
    }
    early.validate().unwrap();
    final_report.validate().unwrap();
    // Independent control: rebuilding the unchanged Global consumes a second
    // full projection and must be refused under the same allowance.
    let mut rebuilding = WorkBudget::new(allowance);
    project_with_retained_global(&source, &algebra, &control, &mut rebuilding, input, None)
        .unwrap();
    assert!(
        project_with_retained_global(&source, &algebra, &control, &mut rebuilding, input, None)
            .is_err()
    );
}

#[test]
fn retained_projection_cannot_relabel_snapshot_or_fact_revision() {
    let (source, request, algebra) = fixture();
    let control = Control::default();
    let input = ProjectionInput {
        request: &request,
        scope: api::AnalysisScope::SingleProgram,
        numeric_model: api::NumericModel::Product,
    };
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let early = project_with_retained_global(&source, &algebra, &control, &mut budget, input, None)
        .unwrap();
    let mut wrong = early.clone();
    Arc::make_mut(wrong.global.as_mut().unwrap())
        .metadata
        .revision = api::DecimalU64(u64::MAX);
    let error = project_with_retained_global(
        &source,
        &algebra,
        &control,
        &mut budget,
        input,
        Some(&wrong),
    )
    .unwrap_err();
    assert_eq!(
        error.details,
        api::ErrorDetails::Report(api::ReportValidationFailure::Hybrid(
            api::HybridValidationKind::Source
        ))
    );
    let mut wrong = early;
    let api::RootTerminalSnapshot::Offline(snapshot) =
        &mut Arc::make_mut(wrong.global.as_mut().unwrap())
            .metadata
            .source_snapshot
    else {
        unreachable!()
    };
    snapshot.namespace = api::DecimalU64(u64::MAX);
    assert!(
        project_with_retained_global(
            &source,
            &algebra,
            &control,
            &mut budget,
            input,
            Some(&wrong)
        )
        .is_err()
    );
}

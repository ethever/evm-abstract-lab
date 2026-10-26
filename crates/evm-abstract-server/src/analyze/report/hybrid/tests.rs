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
    fixture_code("00")
}

fn fixture_code(
    code: &str,
) -> (
    analysis::HybridAnalysis<NumericValue>,
    api::AnalyzeRequest,
    ValueAlgebra<ProductAlgebra>,
) {
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    let api::AnalysisInput::Bytecode(bytes) = &mut request.input else {
        unreachable!()
    };
    bytes.bytecode = code.into();
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
fn actual_minimum_control_directory_is_retained_on_rich_attachment_and_paid_refusal() {
    let (source, request, algebra) = fixture_code("6003565b6007565b00");
    let control = Control::default();
    let input = ProjectionInput {
        request: &request,
        scope: api::AnalysisScope::SingleProgram,
        numeric_model: api::NumericModel::Product,
    };
    let mut measurement = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let early =
        super::super::minimum::project(&source, &algebra, &control, &mut measurement, input)
            .unwrap();
    let minimum_work = measurement.used();
    assert_eq!(early.process_controls.len(), 1);
    let complete = project_with_retained_global(
        &source,
        &algebra,
        &control,
        &mut measurement,
        input,
        Some(&early),
    )
    .unwrap();
    let full_work = measurement.used();
    assert_eq!(complete.rich_export, api::RichExportOutcome::Complete);
    assert!(Arc::ptr_eq(
        early.minimum.as_ref().unwrap(),
        complete.minimum.as_ref().unwrap()
    ));
    for (old, new) in early
        .process_controls
        .iter()
        .zip(&complete.process_controls)
    {
        assert!(Arc::ptr_eq(old, new));
        assert_eq!(old.id, new.id);
    }
    complete.validate().unwrap();
    let cap = minimum_work + (full_work - minimum_work) / 2;
    assert!(minimum_work + early.programs.len() + early.process_controls.len() + 2 < cap);
    assert!(cap < full_work);
    let mut budget = WorkBudget::new(cap);
    let early =
        super::super::minimum::project(&source, &algebra, &control, &mut budget, input).unwrap();
    let refused = project_with_retained_global(
        &source,
        &algebra,
        &control,
        &mut budget,
        input,
        Some(&early),
    )
    .unwrap();
    assert!(matches!(
        refused.rich_export,
        api::RichExportOutcome::Refused(_)
    ));
    assert!(refused.global.is_none() && refused.native.is_none());
    assert!(Arc::ptr_eq(
        early.minimum.as_ref().unwrap(),
        refused.minimum.as_ref().unwrap()
    ));
    assert_eq!(early.process_controls.len(), refused.process_controls.len());
    for (old, new) in early.process_controls.iter().zip(&refused.process_controls) {
        assert!(Arc::ptr_eq(old, new));
    }
    assert_eq!(refused.postprocessing_work, budget.used() as u64);
    assert!(budget.exhausted());
    early.validate().unwrap();
    refused.validate().unwrap();
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

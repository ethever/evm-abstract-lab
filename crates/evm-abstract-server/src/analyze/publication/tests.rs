use super::Projection;
use crate::analyze::api;
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, ExecutionConfig, HybridConfig,
        control::Control, cooperation::NoExtension, publication::StagePublisher,
    },
    bytecode::Program,
    domain::{ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
};

mod rpc;

#[test]
fn minimum_retention_payment_preserves_cancellation_owners_and_work_refusal() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let model = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::from_program(Program::from_hex("00").unwrap(), Default::default())
                .unwrap(),
            ExecutionConfig::default(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    let request = api::AnalyzeRequest::default();
    let original_input = request.clone();
    let producer = model.baseline_handle();
    let input = super::report::ProjectionInput {
        request: &request,
        scope: api::AnalysisScope::SingleProgram,
        numeric_model: api::NumericModel::Product,
    };
    let mut measurement = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    super::report::minimum::project(
        &model,
        &algebra,
        &Control::default(),
        &mut measurement,
        input,
    )
    .unwrap();
    for cancelled_owner in [Some(true), Some(false), None] {
        let captured = Control::new(Default::default());
        let current = Control::new(Default::default());
        let limit = if cancelled_owner.is_some() {
            crate::analyze::DEFAULT_POSTPROCESS_WORK
        } else {
            measurement.used() + 1
        };
        let mut budget = captured.scope(|| WorkBudget::new(limit));
        let mut report = current
            .scope(|| {
                super::report::minimum::project(&model, &algebra, &current, &mut budget, input)
            })
            .unwrap();
        let before = report.clone();
        let minimum = std::sync::Arc::clone(report.minimum.as_ref().unwrap());
        let paid = budget.used();
        assert!(paid > 0 && !budget.exhausted());
        assert_eq!(report.postprocessing_work, paid as u64);
        assert!(captured.checkpoint().is_ok() && current.checkpoint().is_ok());
        if let Some(cancel_captured) = cancelled_owner {
            assert!(limit - paid >= 2);
            if cancel_captured {
                captured.cancellation().cancel();
                assert!(current.checkpoint().is_ok());
            } else {
                current.cancellation().cancel();
                assert!(captured.checkpoint().is_ok());
            }
        }
        let error = current
            .scope(|| super::retain_minimum(&mut report, &current, &mut budget))
            .unwrap_err();
        if cancelled_owner.is_some() {
            assert_eq!(error, crate::analyze::cancelled());
        } else {
            assert_eq!(error.code, api::ApiErrorCode::Export);
            let api::ErrorDetails::Export(failure) = error.details else {
                panic!("missing typed retention work refusal")
            };
            assert_eq!(failure.kind, api::ExportFailureKind::Work);
            assert_eq!(failure.required, 2);
            assert_eq!(failure.usage, minimum.export_usage);
            assert!(captured.checkpoint().is_ok() && current.checkpoint().is_ok());
        }
        assert_eq!(budget.used(), paid);
        assert_eq!(budget.limit(), limit);
        assert_eq!(report, before);
        assert!(std::sync::Arc::ptr_eq(
            report.minimum.as_ref().unwrap(),
            &minimum
        ));
        assert!(model.baseline_handle().same_model(&producer));
        assert_eq!(request, original_input);
        report.validate().unwrap();
    }
}

#[test]
fn required_minimum_refusal_is_latched_even_when_the_payer_accepts_later_small_charges() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let model = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::from_program(Program::from_hex("00").unwrap(), Default::default())
                .unwrap(),
            ExecutionConfig::default(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &control,
        )
        .unwrap();
    assert!(model.coverage().baseline_current);
    let request = api::AnalyzeRequest::default();
    let mut budget = WorkBudget::new(1_000_000);
    assert!(!budget.charge(1_000_001));
    let mut projection = Projection::new(
        &request,
        api::AnalysisScope::SingleProgram,
        &algebra,
        api::NumericModel::Product,
        &control,
        None,
        &mut budget,
    );
    projection.publish(&model);
    assert!(projection.checkpoint().is_err());
    let failure = projection.finish(&model).unwrap_err();
    assert_eq!(failure.code, api::ApiErrorCode::Export);
    assert!(
        budget.charge(1),
        "charge alone is not a required-stage refusal latch"
    );
    assert!(budget.used() < budget.limit());
}

#[test]
fn required_minimum_abort_preserves_the_export_cause_without_user_cancellation() {
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::new(Default::default());
    let request = api::AnalyzeRequest::default();
    let mut budget = WorkBudget::new(1);
    let mut projection = Projection::new(
        &request,
        api::AnalysisScope::SingleProgram,
        &algebra,
        api::NumericModel::Product,
        &control,
        None,
        &mut budget,
    );
    let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            AnalysisInput::from_program(Program::from_hex("00").unwrap(), Default::default())
                .unwrap(),
            ExecutionConfig::default(),
            HybridConfig::default(),
            &control,
            &mut projection,
        );
    assert!(matches!(
        result,
        Err(evm_abstract::analysis::cooperation::CooperationError::Cancelled)
    ));
    let original = projection.failure.as_ref().unwrap().clone();
    let error = projection
        .resolve(result.map_err(crate::analyze::errors::execution))
        .unwrap_err();
    assert_eq!(error, original);
    assert_eq!(error.code, api::ApiErrorCode::Export);
    assert!(!control.cancellation().is_cancelled());
    assert!(budget.used() <= 1);
}

#[test]
fn synchronous_api_keeps_the_canonical_minimum_without_an_external_publisher() {
    let report = crate::analyze::analyze(api::AnalyzeRequest::default()).unwrap();
    let minimum = report
        .minimum
        .as_ref()
        .expect("required minimum must not be skipped");
    assert!(minimum.current);
    assert!(!minimum.summaries.is_empty());
    minimum.validate().unwrap();
    report.validate().unwrap();
}

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
    let failure = projection.finish(&model).unwrap_err();
    assert_eq!(failure.code, api::ApiErrorCode::Export);
    assert!(
        budget.charge(1),
        "charge alone is not a required-stage refusal latch"
    );
    assert!(budget.used() < budget.limit());
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

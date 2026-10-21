//! Actual product and finite carriers share source binding and report projection.
use super::*;
use evm_abstract::domain::{FiniteSetAlgebra, WordElement};

fn typed_terminal<D: EvmNumericAlgebra>(
    algebra: ValueAlgebra<D>,
    numeric_model: api::NumericModel,
) {
    let owner = Address::repeat_byte(0x11);
    let request = api::AnalyzeRequest {
        input: api::AnalysisInput::Bytecode(api::BytecodeInput {
            bytecode: "6003565b602a5f5260205ff3".into(),
            address: Some(owner.to_string()),
        }),
        limits: api::AnalysisLimits {
            numeric_model,
            max_transfers: 1,
            ..api::AnalysisLimits::default()
        },
        ..api::AnalyzeRequest::default()
    };
    let mut world = EvmWorld::<D::Element>::offline(
        Fork::Osaka,
        "typed terminal projection",
        "product and finite source validity",
    );
    let api::AnalysisInput::Bytecode(source) = &request.input else {
        unreachable!()
    };
    world
        .insert(
            owner,
            EvmAccount::from_hex(&source.bytecode, Fork::Osaka).unwrap(),
        )
        .unwrap();
    let native = execute(
        world,
        EvmEntry {
            address: owner,
            environment: input::environment(&request.environment).unwrap(),
        },
        input::config(&request.limits).unwrap(),
        &algebra,
    );
    let binding = native.root_terminal_coverage().unwrap().input();
    assert_eq!(binding.domain_spec(), algebra.spec());
    assert!(binding.call_value().numeric().observation().may_be_zero());
    assert!(binding.input_scope().is_some());
    assert!(binding.snapshot().offline_namespace().is_some());
    let report = report::report(
        &native,
        &request,
        api::AnalysisScope::SingleProgram,
        &algebra,
        numeric_model,
        &Control::default(),
    )
    .unwrap();
    report.validate().unwrap();
    assert_eq!(
        report.metadata.postprocessing_max_work,
        DEFAULT_POSTPROCESS_WORK as u64
    );
    assert!(report.metadata.postprocessing_work <= report.metadata.postprocessing_max_work);
    let projected = &report.root_terminal_coverage.as_ref().unwrap().input;
    let failure_data =
        &report.byte_arrays[report.root_terminal_coverage.as_ref().unwrap().outcomes[2].data];
    let failure_length = native.root_terminal_coverage().unwrap().outcomes()[2]
        .data()
        .len()
        .numeric()
        .observation();
    assert_eq!(failure_length.singleton(), Some(evm_abstract::U256::ZERO));
    assert_eq!(failure_data.length.constants, Some(vec!["0x0".into()]));
    let (lower, upper) = failure_length.interval().unsigned_bounds();
    assert_eq!(
        failure_data.length.unsigned.lower,
        crate::analyze::value::word(lower)
    );
    assert_eq!(
        failure_data.length.unsigned.upper,
        crate::analyze::value::word(upper)
    );
    assert_eq!(projected.numeric_model, numeric_model);
    assert_eq!(projected.revision.0, binding.revision().value());
    assert_eq!(
        projected.input_scope,
        binding.input_scope().map(api::DecimalU64)
    );
    assert!(
        report
            .symbolic
            .nodes
            .get(projected.storage_source.node)
            .is_some()
    );
    assert!(!report.ssa.complete);

    // The public request dispatcher reaches the same semantic boundary for
    // either selected model, with its actual numeric policy in the binding.
    let dispatched = analyze(request).unwrap();
    dispatched.validate().unwrap();
    assert_eq!(
        dispatched
            .root_terminal_coverage
            .unwrap()
            .input
            .numeric_model,
        numeric_model
    );
}

#[test]
fn product_and_finite_carriers_preserve_terminal_sources_through_request_and_projection() {
    let spec = input::spec(&api::AnalysisLimits::default()).unwrap();
    typed_terminal(
        ValueAlgebra::new(ProductAlgebra::from_spec(spec)),
        api::NumericModel::Product,
    );
    typed_terminal(
        ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
        api::NumericModel::FiniteSet,
    );
}

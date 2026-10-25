//! The application callback publishes numerical/control data before rich work.
use super::*;
use evm_abstract::{
    Address, U256,
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, HybridConfig,
        cooperation::NoExtension,
        publication::{ModelHandle, StagePublisher},
    },
    domain::{
        EvmValue, FiniteSetAlgebra, NumericValue, ProductAlgebra, WordObservation,
        identity::Symbol,
        symbolic::{ExprId, ExprLimits},
    },
    world::{EvmAccount, EvmEntry, EvmWorld},
};

fn request() -> api::AnalyzeRequest {
    let mut request = api::AnalyzeRequest::default();
    request.limits.root_terminal_coverage = false;
    request.input = api::AnalysisInput::Bytecode(api::BytecodeInput {
        // A real CALL capability supplies subscriptions and return deliveries.
        bytecode: "6000600060006000600061020161fffff100".into(),
        address: Some(Address::from_word(U256::from(0x101).into()).to_string()),
    });
    request
}

fn input(request: &api::AnalyzeRequest, deep_display: bool) -> AnalysisInput<NumericValue> {
    let fork = crate::analyze::input::fork(request.fork);
    let address = Address::from_word(U256::from(0x101).into());
    let mut world = EvmWorld::new(fork, "minimum application publication");
    let api::AnalysisInput::Bytecode(source) = &request.input else {
        unreachable!()
    };
    let mut root = EvmAccount::from_hex(&source.bytecode, fork).unwrap();
    root.storage
        .insert(U256::from(7), EvmValue::constant(U256::from(42)));
    world.insert(address, root).unwrap();
    world
        .insert(
            Address::from_word(U256::from(0x201).into()),
            EvmAccount::from_hex("600160005560006000f3", fork).unwrap(),
        )
        .unwrap();
    let mut entry = EvmEntry::new(address);
    entry.environment = crate::analyze::input::environment(&request.environment).unwrap();
    entry.environment.to = address.into();
    if deep_display {
        let mut expression = ExprId::input(77, Symbol::GasPrice);
        let limits = ExprLimits {
            max_nodes: 1024,
            max_depth: 512,
        };
        for _ in 1..300 {
            expression =
                ExprId::operation(0x01, &[expression, ExprId::constant(U256::ONE)], limits)
                    .unwrap();
        }
        assert_eq!(expression.depth(), 300);
        entry.environment.gas_price = EvmValue::top().with_expression(expression);
    }
    AnalysisInput::new(world, entry).unwrap()
}

struct Capture<'a> {
    request: &'a api::AnalyzeRequest,
    algebra: &'a ValueAlgebra<ProductAlgebra>,
    control: &'a Control,
    budget: &'a mut WorkBudget,
    report: Option<api::AnalysisReport>,
    model: Option<ModelHandle<NumericValue>>,
    cancel: bool,
    refuse_rich: bool,
    first_work: usize,
}

impl StagePublisher<NumericValue> for Capture<'_> {
    fn publish(&mut self, source: &analysis::HybridAnalysis<NumericValue>) {
        assert!(self.report.is_none());
        assert!(source.detail().is_none());
        assert_eq!(source.coverage().work.preparation, 0);
        assert_eq!(source.coverage().work.detail, 0);
        let input = ProjectionInput {
            request: self.request,
            scope: api::AnalysisScope::SingleProgram,
            numeric_model: api::NumericModel::Product,
        };
        let mut report = project(source, self.algebra, self.control, self.budget, input).unwrap();
        assert!(report.global.is_none() && report.native.is_none());
        let minimum = report.minimum.as_ref().unwrap();
        minimum.validate().unwrap();
        assert!(minimum.current && minimum.status == api::AnalysisStatus::Converged);
        assert!(!minimum.summaries.is_empty());
        assert!(
            minimum
                .summaries
                .iter()
                .any(|summary| !summary.relation.regional_cfg.calls.is_empty())
        );
        self.first_work = self.budget.used();
        self.model = Some(source.baseline_handle());
        if self.refuse_rich {
            let retained = Arc::clone(minimum);
            let error = super::super::hybrid::project_with_retained_global(
                source,
                self.algebra,
                self.control,
                self.budget,
                input,
                None,
            )
            .unwrap_err();
            assert!(
                matches!(&error.details, api::ErrorDetails::Export(failure)
                if failure.kind == api::ExportFailureKind::Depth),
                "{error:?}"
            );
            report.rich_export = api::RichExportOutcome::Refused(error);
            assert!(Arc::ptr_eq(&retained, report.minimum.as_ref().unwrap()));
        }
        report.postprocessing_work = self.budget.used() as u64;
        self.report = Some(report);
        if self.cancel {
            self.control.cancellation().cancel();
        }
    }
}

#[test]
fn first_callback_exposes_control_and_joint_bounds_before_optional_rich_refusal() {
    let request = request();
    let input = input(&request, true);
    let snapshot = hybrid::snapshot(input.snapshot()).unwrap();
    let revision = input.revision().value();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        crate::analyze::input::spec(&request.limits).unwrap(),
    ));
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let mut capture = Capture {
        request: &request,
        algebra: &algebra,
        control: &control,
        budget: &mut budget,
        report: None,
        model: None,
        cancel: false,
        refuse_rich: true,
        first_work: 0,
    };
    let final_result = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            input,
            crate::analyze::input::config(&request.limits).unwrap(),
            HybridConfig::default(),
            &control,
            &mut capture,
        )
        .unwrap();
    assert!(
        final_result.detail().is_some(),
        "optional refusal must not stop Native preparation"
    );
    assert!(final_result.coverage().work.preparation > 0);
    let report = capture.report.take().unwrap();
    let model = capture.model.take().unwrap();
    assert!(model.same_model(&final_result.baseline_handle()));
    let minimum = report.minimum.as_ref().unwrap();
    assert_eq!(minimum.source_snapshot, snapshot);
    assert_eq!(minimum.revision.0, revision);
    assert_eq!(
        minimum.environment.input_scope.map(|scope| scope.0),
        model.entry().environment.input_scope_id()
    );
    assert_eq!(
        minimum.environment.origin, minimum.environment.caller,
        "omitted origin preserves the original caller alias"
    );
    assert_eq!(
        minimum.root_binding,
        mapping::root_binding(model.root_binding())
    );
    assert!(capture.budget.used() >= capture.first_work);
    assert!(matches!(
        report.rich_export,
        api::RichExportOutcome::Refused(_)
    ));
    assert!(
        minimum
            .accounts
            .iter()
            .any(|account| account.storage.iter().any(|slot| {
                slot.slot == "0x7" && slot.value.constants.as_deref() == Some(&["0x2a".into()])
            })),
        "the original numerical input must remain inspectable alongside joined invariants"
    );
    for (source, projected) in model.procedure_summaries().iter().zip(&minimum.summaries) {
        assert_eq!(source.closed, projected.relation.closed);
        assert_eq!(source.source_current, projected.relation.source_current);
        assert_eq!(source.version.generation, projected.version.generation);
        for (row, bound) in source.procedures.iter().zip(projected.bounds.iter()) {
            for (outcome, exported) in row.outputs.iter().zip(&bound.outputs) {
                assert_eq!(outcome.generation, exported.generation);
                assert_eq!(summaries::outcome(outcome.kind), exported.kind);
                let expected = value::observation(&outcome.data.len().numeric().observation());
                assert_eq!(expected.constants, exported.data.length.constants);
                assert_eq!(expected.unsigned, exported.data.length.unsigned);
                assert_eq!(expected.known_one, exported.data.length.known_one);
                assert_eq!(expected.known_zero, exported.data.length.known_zero);
            }
        }
    }
    let decoded: api::AnalysisReport =
        serde_json::from_slice(&serde_json::to_vec(&report).unwrap()).unwrap();
    assert_eq!(decoded, report);
    decoded.validate().unwrap();
    if let Some(path) = std::env::var_os("EVM_MINIMUM_FIXTURE_PATH") {
        std::fs::write(path, serde_json::to_vec(&report).unwrap()).unwrap();
    }
}

#[test]
fn cancellation_after_first_callback_retains_the_same_minimum_and_model_identity() {
    let request = request();
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::new(analysis::progress::Observer::default());
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let mut capture = Capture {
        request: &request,
        algebra: &algebra,
        control: &control,
        budget: &mut budget,
        report: None,
        model: None,
        cancel: true,
        refuse_rich: false,
        first_work: 0,
    };
    let result = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid_with_publisher(
            input(&request, false),
            crate::analyze::input::config(&request.limits).unwrap(),
            HybridConfig::default(),
            &control,
            &mut capture,
        );
    assert!(matches!(
        result,
        Err(analysis::cooperation::CooperationError::Cancelled)
    ));
    let report = capture.report.take().unwrap();
    let minimum = Arc::clone(report.minimum.as_ref().unwrap());
    let paid = capture.budget.used();
    assert_eq!(
        capture.model.as_ref().unwrap().status(),
        analysis::Status::Converged
    );
    assert!(Arc::ptr_eq(&minimum, report.minimum.as_ref().unwrap()));
    assert_eq!(capture.budget.used(), paid);
    minimum.validate().unwrap();
}

fn baseline_fixture() -> (
    analysis::HybridAnalysis<NumericValue>,
    api::AnalyzeRequest,
    ValueAlgebra<ProductAlgebra>,
) {
    let request = request();
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(
        crate::analyze::input::spec(&request.limits).unwrap(),
    ));
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let source = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            input(&request, false),
            crate::analyze::input::config(&request.limits).unwrap(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
    (source, request, algebra)
}

#[test]
fn minimum_rejects_empty_control_private_roots_and_unbound_numerical_rows() {
    let (source, request, algebra) = baseline_fixture();
    let control = Control::default();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let report = project(
        &source,
        &algebra,
        &control,
        &mut budget,
        ProjectionInput {
            request: &request,
            scope: api::AnalysisScope::SingleProgram,
            numeric_model: api::NumericModel::Product,
        },
    )
    .unwrap();
    let minimum = report.minimum.as_ref().unwrap();
    let mut invalid = (**minimum).clone();
    invalid.summaries.clear();
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Minimum(
            api::MinimumValidationKind::Closure
        ))
    );
    let mut invalid = (**minimum).clone();
    invalid.environment.call_value.expression = Some(api::ExpressionRef { node: 0 });
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Minimum(
            api::MinimumValidationKind::PrivateState
        ))
    );
    let mut invalid = (**minimum).clone();
    let summary = invalid
        .summaries
        .iter_mut()
        .find(|summary| summary.relation.closed)
        .unwrap();
    Arc::make_mut(&mut summary.bounds).clear();
    assert_eq!(
        invalid.validate(),
        Err(api::ReportValidationFailure::Minimum(
            api::MinimumValidationKind::Bounds
        ))
    );
}

#[test]
fn unchanged_numerical_table_uses_paid_handles_without_projecting_the_rows_again() {
    let (source, _, _) = baseline_fixture();
    let model = source.baseline_handle();
    let control = Control::default();
    let mut tables = Vec::new();
    let mut second_budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let mut pools = Pools::with_programs(&control, Vec::new());
    pools.symbolic = symbolic::Pool::minimum(&control, &mut second_budget);
    let observation = &model.procedure_summaries()[0];
    let first = bounds(observation, &mut tables, &mut pools).unwrap();
    let work = pools.symbolic.usage.work;
    let second = bounds(observation, &mut tables, &mut pools).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    assert_eq!(
        pools.symbolic.usage.work - work,
        5,
        "unchanged immutable numerical rows cost only table lookup/handles"
    );
}

#[test]
fn required_minimum_refusal_does_not_invent_a_public_model_or_reset_payment() {
    let (source, request, algebra) = baseline_fixture();
    let control = Control::default();
    let mut budget = WorkBudget::new(1);
    let error = project(
        &source,
        &algebra,
        &control,
        &mut budget,
        ProjectionInput {
            request: &request,
            scope: api::AnalysisScope::SingleProgram,
            numeric_model: api::NumericModel::Product,
        },
    )
    .unwrap_err();
    assert!(
        matches!(error.details, api::ErrorDetails::Export(ref failure) if failure.kind == api::ExportFailureKind::Work)
    );
    assert!(budget.exhausted());
    assert_eq!(budget.used(), 0);
    assert!(source.coverage().baseline_current);
}

#[test]
fn original_namespace_template_is_distinct_from_physical_store_code_bounds() {
    let mut request = request();
    let api::AnalysisInput::Bytecode(bytecode) = &mut request.input else {
        unreachable!()
    };
    bytecode.address = None;
    let algebra = ValueAlgebra::new(ProductAlgebra::default());
    let input = crate::analyze::input::bytecode(
        bytecode,
        crate::analyze::input::fork(request.fork),
        crate::analyze::input::environment(&request.environment).unwrap(),
    )
    .unwrap();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let source = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            input,
            crate::analyze::input::config(&request.limits).unwrap(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &control,
        )
        .unwrap();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let report = project(
        &source,
        &algebra,
        &control,
        &mut budget,
        ProjectionInput {
            request: &request,
            scope: api::AnalysisScope::SingleProgram,
            numeric_model: api::NumericModel::Product,
        },
    )
    .unwrap();
    let minimum = report.minimum.as_ref().unwrap();
    let api::RootInputBinding::Namespace(root) = &minimum.root_binding else {
        panic!("namespace input");
    };
    let original = minimum
        .accounts
        .iter()
        .find(|row| row.root_template)
        .unwrap();
    assert_eq!(original.address, root.coordinate);
    assert!(
        report.programs[original.program.unwrap()]
            .code_address
            .is_none()
    );
    for summary in &minimum.summaries {
        for bounds in summary.bounds.iter() {
            for store in std::iter::once(&bounds.input)
                .chain(bounds.outputs.iter().map(|output| &output.store))
            {
                for code in &store.codes {
                    if let Some(program) = code.program {
                        assert_eq!(
                            report.programs[program].code_address.as_deref(),
                            Some(code.owner.as_str())
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn finite_set_minimum_keeps_the_actual_carrier_and_original_scalar_bound() {
    let mut request = api::AnalyzeRequest::default();
    request.limits.numeric_model = api::NumericModel::FiniteSet;
    request.limits.root_terminal_coverage = false;
    request.environment.call_value = api::WordInput::Concrete("0x2a".into());
    let algebra = ValueAlgebra::new(FiniteSetAlgebra::from_spec(
        crate::analyze::input::spec(&request.limits).unwrap(),
    ));
    let api::AnalysisInput::Bytecode(bytecode) = &request.input else {
        unreachable!()
    };
    let input = crate::analyze::input::bytecode(
        bytecode,
        crate::analyze::input::fork(request.fork),
        crate::analyze::input::environment(&request.environment).unwrap(),
    )
    .unwrap();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let control = Control::default();
    let source = AnalysisEngine::new(&algebra, &policies, &mut extension)
        .run_hybrid(
            input,
            crate::analyze::input::config(&request.limits).unwrap(),
            HybridConfig {
                max_detail_work: 0,
                ..Default::default()
            },
            &control,
        )
        .unwrap();
    let mut budget = WorkBudget::new(crate::analyze::DEFAULT_POSTPROCESS_WORK);
    let report = project(
        &source,
        &algebra,
        &control,
        &mut budget,
        ProjectionInput {
            request: &request,
            scope: api::AnalysisScope::SingleProgram,
            numeric_model: api::NumericModel::FiniteSet,
        },
    )
    .unwrap();
    report.validate().unwrap();
    let minimum = report.minimum.as_ref().unwrap();
    assert_eq!(minimum.numeric_model, api::NumericModel::FiniteSet);
    assert_eq!(
        minimum.domain_spec.capacity,
        source.baseline().domain_spec().capacity() as u64
    );
    assert_eq!(
        minimum.environment.call_value.constants,
        Some(vec!["0x2a".into()])
    );
    assert!(report.global.is_none() && report.native.is_none());
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct Counted {
    value: NumericValue,
    quote: usize,
    opaque: bool,
}
thread_local! { static OBSERVATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
impl std::fmt::Display for Counted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("counted")
    }
}
impl WordElement for Counted {
    fn top() -> Self {
        Self {
            value: NumericValue::top(),
            quote: 1,
            opaque: true,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            value: NumericValue::constant(value),
            ..Self::top()
        }
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            value: NumericValue::from_observation(value),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        OBSERVATIONS.with(|calls| calls.set(calls.get() + 1));
        self.value.observation()
    }
    fn requires_retained_binding(&self) -> bool {
        self.opaque
    }
    fn is_top(&self) -> bool {
        self.value.is_top()
    }
    fn work_size(&self) -> usize {
        self.quote
    }
}

#[test]
fn mandatory_numeric_refusals_never_query_an_unpaid_or_opaque_carrier() {
    let control = Control::default();
    for (value, expected) in [
        (
            Counted {
                value: NumericValue::top(),
                quote: usize::MAX,
                opaque: false,
            },
            api::ExportFailureKind::Work,
        ),
        (
            Counted {
                value: NumericValue::top(),
                quote: 1,
                opaque: true,
            },
            api::ExportFailureKind::Numeric,
        ),
    ] {
        OBSERVATIONS.with(|calls| calls.set(0));
        let mut budget = WorkBudget::new(10_000);
        let mut pool = symbolic::Pool::minimum(&control, &mut budget);
        let error = pool.value(&EvmValue::from_numeric(value)).unwrap_err();
        assert!(
            matches!(error.details, api::ErrorDetails::Export(ref failure) if failure.kind == expected)
        );
        OBSERVATIONS.with(|calls| assert_eq!(calls.get(), 0));
        assert!(pool.graph.nodes.is_empty());
    }
}

#[test]
fn mandatory_projection_checks_both_control_and_original_ledger_cancellation() {
    for cancel_ledger in [false, true] {
        let original = Control::new(analysis::progress::Observer::default());
        let current = Control::new(analysis::progress::Observer::default());
        let mut budget = original.scope(|| WorkBudget::new(10_000));
        if cancel_ledger {
            original.cancellation().cancel();
        } else {
            current.cancellation().cancel();
        }
        let mut pool = symbolic::Pool::minimum(&current, &mut budget);
        let error = pool.value(&EvmValue::<NumericValue>::top()).unwrap_err();
        assert!(
            matches!(error.details, api::ErrorDetails::Export(ref failure)
            if failure.kind == api::ExportFailureKind::Cancelled)
        );
        assert!(pool.graph.nodes.is_empty());
    }
}

//! Test/benchmark-only fixed-World seam. Request acquisition is bypassed;
//! execution and pooled projection use the actual default production paths.
//! This feature adds neither a public request mode nor an HTTP endpoint.
use super::{errors, input, mapping, report};
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, EnginePolicies, HybridAnalysis, control::Control,
        cooperation::NoExtension,
    },
    domain::{EvmNumericAlgebra, FiniteSetAlgebra, NumericValue, ProductAlgebra, ValueAlgebra},
};
use evm_abstract_protocol as api;

fn algebra(request: &api::AnalyzeRequest) -> Result<ValueAlgebra<ProductAlgebra>, api::ApiError> {
    if request.limits.numeric_model != api::NumericModel::Product {
        return Err(input::invalid(
            api::ValidationErrorKind::Limits,
            "numeric_model",
            "captured Product input requires the Product numeric model",
        ));
    }
    Ok(ValueAlgebra::new(ProductAlgebra::from_spec(input::spec(
        &request.limits,
    )?)))
}

/// Exact native compatibility control for consumer tests. It shares bytecode
/// admission and the real pooled projector, retaining the original native
/// ledger/transfer limits independently of the default Hybrid baseline.
pub fn native_bytecode(
    request: &api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    let timing = super::timing::Guard::start(control);
    let mut result = control.scope(|| {
        super::checkpoint(control)?;
        control
            .observer()
            .phase(evm_abstract::analysis::progress::Phase::Validating);
        let spec = input::spec(&request.limits)?;
        match request.limits.numeric_model {
            api::NumericModel::Product => native_typed(
                request,
                &ValueAlgebra::new(ProductAlgebra::from_spec(spec)),
                control,
            ),
            api::NumericModel::FiniteSet => native_typed(
                request,
                &ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
                control,
            ),
        }
    });
    drop(timing);
    if let Ok(report) = &mut result {
        report.timings = super::timing::project(control.observer().timing(), 0);
    }
    result
}

fn native_typed<D: EvmNumericAlgebra>(
    request: &api::AnalyzeRequest,
    algebra: &ValueAlgebra<D>,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    let api::AnalysisInput::Bytecode(source) = &request.input else {
        return Err(input::invalid(
            api::ValidationErrorKind::Field,
            "input",
            "native bytecode control requires bytecode input",
        ));
    };
    let config = input::config(&request.limits)?;
    let source = input::bytecode(
        source,
        input::fork(request.fork),
        input::environment(&request.environment)?,
    )?;
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let native = AnalysisEngine::new(algebra, &policies, &mut extension)
        .run(source, config, control)
        .map_err(errors::execution)?;
    report::report(
        &native,
        request,
        api::AnalysisScope::SingleProgram,
        algebra,
        request.limits.numeric_model,
        control,
    )
}

/// Analyze the supplied original World/Entry; never reacquire or reconstruct it
/// from bytecode. The caller records the captured input's complete identity.
pub fn analyze(
    input: AnalysisInput<NumericValue>,
    request: &api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    control.scope(|| {
        let algebra = algebra(request)?;
        if input.world().fork() != input::fork(request.fork) {
            return Err(input::invalid(
                api::ValidationErrorKind::Limits,
                "fork",
                "captured World and request forks differ",
            ));
        }
        let config = input::config(&request.limits)?;
        let policies = EnginePolicies::default();
        let mut extension = NoExtension;
        let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
        let hybrid = engine
            .run_hybrid(
                input,
                config,
                input::hybrid_config(&request.limits)?,
                control,
            )
            .map_err(errors::execution)?;
        report::hybrid::project(
            &hybrid,
            request,
            api::AnalysisScope::RpcWorld,
            &algebra,
            api::NumericModel::Product,
            control,
        )
    })
}

/// Pool an already retained actual Hybrid result without running the analyzer
/// again. This isolates export measurement and retains both native namespaces.
pub fn project(
    source: &HybridAnalysis<NumericValue>,
    request: &api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    control.scope(|| {
        let algebra = algebra(request)?;
        if source.baseline().world().fork() != input::fork(request.fork) {
            return Err(input::invalid(
                api::ValidationErrorKind::Limits,
                "fork",
                "captured result and export forks differ",
            ));
        }
        if source.baseline().domain_spec() != input::spec(&request.limits)? {
            return Err(input::invalid(
                api::ValidationErrorKind::Limits,
                "numeric_limits",
                "captured result and export numeric policies differ",
            ));
        }
        let mut expected = mapping::execution_config(&input::config(&request.limits)?);
        expected.root_terminal_coverage = false;
        if mapping::execution_config(source.baseline().config()) != expected {
            return Err(input::invalid(
                api::ValidationErrorKind::Limits,
                "execution_limits",
                "captured result and export execution policies differ",
            ));
        }
        report::hybrid::project(
            source,
            request,
            api::AnalysisScope::RpcWorld,
            &algebra,
            api::NumericModel::Product,
            control,
        )
    })
}

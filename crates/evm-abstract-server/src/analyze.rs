//! Typed request admission and complete immutable world-report projection.
//! Native state/edge IDs and SSA coverage survive without a local-CFG projection.
#[cfg(any(test, feature = "capture-replay"))]
pub mod captured;
mod errors;
pub(crate) use errors::fingerprint as fingerprint_error;
mod input;
mod mapping;
mod pools;
mod process;
mod report;
mod rpc;
mod ssa;
mod symbolic;
pub(crate) mod timing;
mod value;
#[cfg(test)]
use evm_abstract::{
    Address,
    analysis::AnalysisInput,
    world::{Code, EvmAccount, EvmWorld},
};
use evm_abstract::{
    analysis::{
        self, AnalysisEngine, EnginePolicies, control::Control, cooperation::NoExtension,
        progress::Phase,
    },
    domain::{EvmNumericAlgebra, FiniteSetAlgebra, ProductAlgebra, ValueAlgebra},
    world::EvmEntry,
};
use evm_abstract_protocol as api;

/// Independent application allowance shared by requested native postprocessing.
/// It matches the CLI policy and remains available after execution stops.
pub const DEFAULT_POSTPROCESS_WORK: usize = 2_000_000_000;

/// Analyze bytecode without an RPC registry; RPC inputs require `analyze_with_providers`.
pub fn analyze(request: api::AnalyzeRequest) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_with_providers(request, &crate::rpc_providers::Registry::default())
}
/// Analyze bytecode with cancellation and progress; no RPC providers are configured.
pub fn analyze_with_control(
    request: api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_with_control_and_providers(request, control, &crate::rpc_providers::Registry::default())
}
/// Analyze using only explicitly configured backend RPC providers.
pub fn analyze_with_providers(
    request: api::AnalyzeRequest,
    providers: &crate::rpc_providers::Registry,
) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_with_control_and_providers(request, &Control::default(), providers)
}
/// Run with cancellation and the same provider registry used for job admission.
pub fn analyze_with_control_and_providers(
    request: api::AnalyzeRequest,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
) -> Result<api::AnalysisReport, api::ApiError> {
    let timing = timing::Guard::start(control);
    let mut result = control.scope(|| {
        providers.validate_request(&request)?;
        run(request, control, providers)
    });
    drop(timing);
    if let Ok(report) = &mut result {
        report.metadata.timings = timing::project(control.observer().timing(), 0);
    }
    result
}
fn cancelled() -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Cancelled,
        message: "analysis cancelled".into(),
        details: api::ErrorDetails::Task(api::TaskFailure {
            kind: api::TaskErrorKind::Cancelled,
            id: None,
        }),
    }
}
fn checkpoint(control: &Control) -> Result<(), api::ApiError> {
    control.checkpoint().map_err(|_| cancelled())
}
fn config_error(error: analysis::ConfigError) -> api::ApiError {
    errors::configuration(error)
}
fn rpc_error(error: analysis::RpcAnalysisError<std::convert::Infallible>) -> api::ApiError {
    match error {
        analysis::RpcAnalysisError::Session(error) => errors::session_error(error),
        analysis::RpcAnalysisError::Cancelled(_) => cancelled(),
        analysis::RpcAnalysisError::Config(error) => config_error(error),
        analysis::RpcAnalysisError::Rpc(error) => rpc::error(error),
    }
}
fn run(
    request: api::AnalyzeRequest,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
) -> Result<api::AnalysisReport, api::ApiError> {
    checkpoint(control)?;
    control.observer().phase(Phase::Validating);
    let config = input::config(&request.limits)?;
    let spec = input::spec(&request.limits)?;
    // This is the sole numeric-model dispatch. Every downstream input, engine,
    // RPC snapshot and projection retains the selected algebra's real carrier.
    match request.limits.numeric_model {
        api::NumericModel::Product => run_typed(
            request,
            config,
            ValueAlgebra::new(ProductAlgebra::from_spec(spec)),
            api::NumericModel::Product,
            control,
            providers,
        ),
        api::NumericModel::FiniteSet => run_typed(
            request,
            config,
            ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
            api::NumericModel::FiniteSet,
            control,
            providers,
        ),
    }
}

fn run_typed<D: EvmNumericAlgebra>(
    request: api::AnalyzeRequest,
    config: analysis::ExecutionConfig,
    algebra: ValueAlgebra<D>,
    numeric_model: api::NumericModel,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
) -> Result<api::AnalysisReport, api::ApiError> {
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let mut engine = AnalysisEngine::new(&algebra, &policies, &mut extension);
    let mut environment = input::environment::<D::Element>(&request.environment)?;
    let fork = input::fork(request.fork);
    match &request.input {
        api::AnalysisInput::Bytecode(source) => {
            let input = input::bytecode(source, fork, environment)?;
            checkpoint(control)?;
            control.observer().phase(Phase::Analyzing);
            let hybrid = engine
                .run_hybrid(input, config, analysis::HybridConfig::default(), control)
                .map_err(errors::execution)?;
            report::hybrid::project(
                &hybrid,
                &request,
                api::AnalysisScope::SingleProgram,
                &algebra,
                numeric_model,
                control,
            )
        }
        api::AnalysisInput::Rpc(source) => {
            let address = input::address(&source.address, "address")?;
            environment.to = address.into();
            let endpoint = providers.endpoint(&source.provider_id)?;
            let input = input::rpc(source, endpoint, fork, &request.limits)?;
            checkpoint(control)?;
            let hybrid = analysis::analyze_rpc_hybrid(
                engine,
                &input,
                EvmEntry {
                    address,
                    environment,
                },
                config,
                analysis::HybridConfig::default(),
                control,
            )
            .map_err(rpc_error)?;
            report::hybrid::rpc::project(hybrid, &request, &algebra, numeric_model, control)
        }
    }
}

#[cfg(test)]
mod tests;

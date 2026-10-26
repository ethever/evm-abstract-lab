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
mod publication;
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
pub const DEFAULT_POSTPROCESS_WORK: usize = api::application_defaults::DEFAULT_POSTPROCESS_WORK;

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
    analyze_publishing(
        request,
        control,
        providers,
        publication::Options {
            publisher: None,
            max_work: DEFAULT_POSTPROCESS_WORK,
        },
    )
}

/// Run with configured providers and commit a closed Global publication before
/// optional Native work. Custom pool executors can preserve staged delivery.
pub fn analyze_with_publisher(
    request: api::AnalyzeRequest,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
    publisher: &crate::jobs::Publisher,
) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_with_publisher_and_limit(
        request,
        control,
        providers,
        publisher,
        DEFAULT_POSTPROCESS_WORK,
    )
}

/// Set one application allowance shared by minimum and optional rich export.
/// Execution and fixed-snapshot RPC retain the request's separate original payer.
pub fn analyze_with_publisher_and_limit(
    request: api::AnalyzeRequest,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
    publisher: &crate::jobs::Publisher,
    postprocess_work: usize,
) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_publishing(
        request,
        control,
        providers,
        publication::Options {
            publisher: Some(publisher),
            max_work: postprocess_work,
        },
    )
}

fn analyze_publishing(
    request: api::AnalyzeRequest,
    control: &Control,
    providers: &crate::rpc_providers::Registry,
    publishing: publication::Options<'_>,
) -> Result<api::AnalysisReport, api::ApiError> {
    let timing = timing::Guard::start(control);
    let mut result = control.scope(|| {
        providers.validate_request(&request)?;
        run(request, control, providers, publishing)
    });
    drop(timing);
    if let Ok(report) = &mut result {
        report.timings = timing::project(control.observer().timing(), 0);
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
    publishing: publication::Options<'_>,
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
            publishing,
        ),
        api::NumericModel::FiniteSet => run_typed(
            request,
            config,
            ValueAlgebra::new(FiniteSetAlgebra::from_spec(spec)),
            api::NumericModel::FiniteSet,
            control,
            providers,
            publishing,
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
    publishing: publication::Options<'_>,
) -> Result<api::AnalysisReport, api::ApiError> {
    let mut budget = evm_abstract::resource::WorkBudget::new(publishing.max_work);
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
            let mut staged = publication::Projection::new(
                &request,
                api::AnalysisScope::SingleProgram,
                &algebra,
                numeric_model,
                control,
                publishing.publisher,
                &mut budget,
            );
            let hybrid = engine
                .run_hybrid_with_publisher(
                    input,
                    config,
                    analysis::HybridConfig::default(),
                    control,
                    &mut staged,
                )
                .map_err(errors::execution);
            let hybrid = staged.resolve(hybrid)?;
            let retained = staged.finish(&hybrid)?;
            report::hybrid::project_with_retained_global(
                &hybrid,
                &algebra,
                control,
                &mut budget,
                report::ProjectionInput {
                    request: &request,
                    scope: api::AnalysisScope::SingleProgram,
                    numeric_model,
                },
                retained.as_deref(),
            )
        }
        api::AnalysisInput::Rpc(source) => {
            let address = input::address(&source.address, "address")?;
            environment.to = address.into();
            let endpoint = providers.endpoint(&source.provider_id)?;
            let input = input::rpc(source, endpoint, fork, &request.limits)?;
            checkpoint(control)?;
            let mut staged = publication::Projection::new(
                &request,
                api::AnalysisScope::RpcWorld,
                &algebra,
                numeric_model,
                control,
                publishing.publisher,
                &mut budget,
            );
            let hybrid = analysis::analyze_rpc_hybrid_with_publisher(
                engine,
                &input,
                EvmEntry {
                    address,
                    environment,
                },
                config,
                analysis::HybridConfig::default(),
                control,
                &mut staged,
            )
            .map_err(rpc_error);
            let hybrid = staged.resolve(hybrid)?;
            let retained = staged.finish(hybrid.baseline())?;
            report::hybrid::rpc::project_with_retained_global(
                hybrid,
                &algebra,
                control,
                &mut budget,
                report::ProjectionInput {
                    request: &request,
                    scope: api::AnalysisScope::RpcWorld,
                    numeric_model,
                },
                retained.as_deref(),
            )
        }
    }
}

#[cfg(test)]
mod tests;

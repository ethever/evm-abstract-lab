//! Typed request admission and complete immutable world-report projection.
//! Native state/edge IDs and SSA coverage survive without a local-CFG projection.
mod errors;
mod input;
mod mapping;
mod pools;
mod report;
mod rpc;
mod ssa;
mod value;
use evm_abstract::{
    Address,
    analysis::{self, control::Control, progress::Phase},
    bytecode::Program,
    world::{Account, Code, Entry, World},
};
use evm_abstract_protocol as api;

/// Analyze bytecode or an explicitly selected trusted RPC snapshot.
pub fn analyze(request: api::AnalyzeRequest) -> Result<api::AnalysisReport, api::ApiError> {
    analyze_with_control(request, &Control::default())
}
/// Run with cooperative cancellation and typed bounded progress events.
pub fn analyze_with_control(
    request: api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    control.scope(|| run(request, control))
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
fn run(
    request: api::AnalyzeRequest,
    control: &Control,
) -> Result<api::AnalysisReport, api::ApiError> {
    checkpoint(control)?;
    control.observer().phase(Phase::Validating);
    let config = input::config(&request.limits)?;
    let mut environment = input::environment(&request.environment)?;
    let fork = input::fork(request.fork);
    let (native, scope) = match &request.input {
        api::AnalysisInput::Bytecode(source) => {
            if source.bytecode.len() > api::MAX_REQUEST_BYTES {
                return Err(input::invalid(
                    api::ValidationErrorKind::Bytecode,
                    "bytecode",
                    "bytecode text exceeds the request bound",
                ));
            }
            let program =
                Program::from_hex_with_fork(&source.bytecode, fork).map_err(errors::bytecode)?;
            if program.byte_len() > 65_536 {
                return Err(input::invalid(
                    api::ValidationErrorKind::Bytecode,
                    "bytecode",
                    "runtime bytecode exceeds 65536 bytes",
                ));
            }
            // An internal owner is required by Store, but unspecified logical ADDRESS remains symbolic.
            let address = source
                .address
                .as_ref()
                .map(|text| input::address(text, "address"))
                .transpose()?;
            if let Some(address) = address {
                environment.to = address.into();
            }
            let address = address.unwrap_or_else(|| Address::repeat_byte(0x11));
            let mut world = World::offline(fork, "web-bytecode", "explicit browser bytecode");
            let mut account = Account::unknown();
            account.code = Code::Runtime(program);
            world.insert(address, account).map_err(errors::world)?;
            checkpoint(control)?;
            control.observer().phase(Phase::Analyzing);
            let native = analysis::analyze_world_with_control(
                world,
                Entry {
                    address,
                    environment,
                },
                config,
                control,
            )
            .map_err(|error| match error {
                analysis::ControlledAnalysisError::Config(error) => config_error(error),
                analysis::ControlledAnalysisError::Cancelled => cancelled(),
            })?;
            (native, api::AnalysisScope::SingleProgram)
        }
        api::AnalysisInput::Rpc(source) => {
            let address = input::address(&source.address, "address")?;
            environment.to = address.into();
            let input = input::rpc(source, fork, &request.limits)?;
            checkpoint(control)?;
            let native = analysis::analyze_rpc_with_control(
                &input,
                Entry {
                    address,
                    environment,
                },
                config,
                control,
            )
            .map_err(|error| match error {
                analysis::RpcAnalysisError::Cancelled(_) => cancelled(),
                analysis::RpcAnalysisError::Config(error) => config_error(error),
                analysis::RpcAnalysisError::Rpc(error) => rpc::error(error),
            })?
            .into_analysis();
            (native, api::AnalysisScope::RpcWorld)
        }
    };
    report::report(&native, &request, scope, control)
}

#[cfg(test)]
mod tests;

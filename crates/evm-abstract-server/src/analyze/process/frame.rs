//! Map only authoritative process sources and actual bound invocation operands.
use super::{Pool, analysis, api, owner};
use evm_abstract::domain::{EvmValue, WordElement};

fn fork(native: evm_abstract::Fork) -> api::Fork {
    match native {
        evm_abstract::Fork::Cancun => api::Fork::Cancun,
        evm_abstract::Fork::Prague => api::Fork::Prague,
        evm_abstract::Fork::Osaka => api::Fork::Osaka,
    }
}

pub(in crate::analyze) fn key(native: &analysis::ProcessFrameKey) -> api::ProcessFrameKey {
    // Exhaustive destructuring makes a future key coordinate an explicit API handoff.
    let analysis::ProcessFrameKey {
        epoch,
        owner: physical_owner,
        source: execution_source,
        is_static,
    } = native;
    let source = source(*execution_source);
    api::ProcessFrameKey {
        epoch: api::ProcessDigest(epoch.0),
        owner: owner(*physical_owner),
        source,
        is_static: *is_static,
    }
}

pub(in crate::analyze) fn source(native: analysis::ProcessSource) -> api::ProcessSource {
    match native {
        analysis::ProcessSource::InputProgram { hash, fork: rules } => {
            api::ProcessSource::InputProgram(api::ProcessInputProgramSource {
                hash: api::ProcessDigest(hash.0),
                fork: fork(rules),
            })
        }
        analysis::ProcessSource::InputDelegation {
            target,
            hash,
            fork: rules,
        } => api::ProcessSource::InputDelegation(api::ProcessInputDelegationSource {
            target: target.to_string(),
            hash: api::ProcessDigest(hash.0),
            fork: fork(rules),
        }),
        analysis::ProcessSource::Runtime {
            source,
            hash,
            fork: rules,
        } => api::ProcessSource::Runtime(api::ProcessCapturedSource {
            source: source.to_string(),
            hash: api::ProcessDigest(hash.0),
            fork: fork(rules),
        }),
        analysis::ProcessSource::InvalidDelegation {
            source,
            hash,
            fork: rules,
        } => api::ProcessSource::InvalidDelegation(api::ProcessCapturedSource {
            source: source.to_string(),
            hash: api::ProcessDigest(hash.0),
            fork: fork(rules),
        }),
        analysis::ProcessSource::InitCode { hash, fork: rules } => {
            api::ProcessSource::InitCode(api::ProcessInitCodeSource {
                hash: api::ProcessDigest(hash.0),
                fork: fork(rules),
            })
        }
        analysis::ProcessSource::AnyRuntime { fork: rules } => {
            api::ProcessSource::AnyRuntime(api::ProcessForkSource { fork: fork(rules) })
        }
        analysis::ProcessSource::AnyInitCode { fork: rules } => {
            api::ProcessSource::AnyInitCode(api::ProcessForkSource { fork: fork(rules) })
        }
        analysis::ProcessSource::Empty { fork: rules } => {
            api::ProcessSource::Empty(api::ProcessForkSource { fork: fork(rules) })
        }
        analysis::ProcessSource::Precompile {
            address,
            fork: rules,
        } => api::ProcessSource::Precompile(api::ProcessPrecompileSource {
            address: address.to_string(),
            fork: fork(rules),
        }),
    }
}

pub(in crate::analyze) fn operands<N: WordElement>(
    address: &EvmValue<N>,
    caller: &EvmValue<N>,
    value: &EvmValue<N>,
    export: &mut Pool<'_>,
) -> Result<api::ProcessOperands, api::ApiError> {
    export.charge(3)?;
    Ok(api::ProcessOperands {
        address: export.value(address)?,
        caller: export.value(caller)?,
        value: export.value(value)?,
    })
}

pub(in crate::analyze) fn invocation<N: WordElement>(
    native: &analysis::EvmProcessInvocation<N>,
    export: &mut Pool<'_>,
) -> Result<api::ProcessOperands, api::ApiError> {
    let analysis::EvmProcessInvocation {
        address,
        caller,
        value,
    } = native;
    operands(address, caller, value, export)
}

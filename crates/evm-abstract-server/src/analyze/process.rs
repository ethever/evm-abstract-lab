//! Typed process projection; preserve action, target and uncovered-rule distinctions.
use super::symbolic::Pool;
use evm_abstract::analysis;
use evm_abstract_protocol as api;

pub(super) fn semantics(native: analysis::TransferSemantics) -> api::TransferSemantics {
    match native {
        analysis::TransferSemantics::Native => api::TransferSemantics::Native,
        analysis::TransferSemantics::Environment => api::TransferSemantics::Environment,
        analysis::TransferSemantics::KnownProcess => api::TransferSemantics::KnownProcess,
    }
}

pub(super) fn action(native: analysis::ProcessAction) -> api::ProcessAction {
    match native {
        analysis::ProcessAction::Call => api::ProcessAction::Call,
        analysis::ProcessAction::StaticCall => api::ProcessAction::StaticCall,
        analysis::ProcessAction::DelegateCall => api::ProcessAction::DelegateCall,
        analysis::ProcessAction::CallCode => api::ProcessAction::CallCode,
        analysis::ProcessAction::Create => api::ProcessAction::Create,
        analysis::ProcessAction::Create2 => api::ProcessAction::Create2,
    }
}

pub(super) fn owner(native: analysis::ProcessOwner) -> api::ProcessOwner {
    match native {
        analysis::ProcessOwner::Named(address) => api::ProcessOwner::Named(address.to_string()),
        analysis::ProcessOwner::Residual => api::ProcessOwner::Residual,
    }
}

pub(super) fn boundary(native: analysis::ProcessBoundary) -> api::ProcessBoundary {
    match native {
        analysis::ProcessBoundary::ResidualOwner(kind) => {
            api::ProcessBoundary::ResidualOwner(action(kind))
        }
        analysis::ProcessBoundary::Creation(kind) => api::ProcessBoundary::Creation(action(kind)),
        analysis::ProcessBoundary::CooperativeInputs => api::ProcessBoundary::CooperativeInputs,
        analysis::ProcessBoundary::SymbolicOwner => api::ProcessBoundary::SymbolicOwner,
        analysis::ProcessBoundary::NativeInstantiation => api::ProcessBoundary::NativeInstantiation,
    }
}

pub(super) fn transfer(
    native: &analysis::EvmProcessTransfer,
    export: &mut Pool<'_>,
) -> Result<api::ProcessTransfer, api::ApiError> {
    // Pay for both variable collections before allocation; traversal checkpoints
    // keep cancellation observable even when a receipt is large or incomplete.
    export.charge(
        3usize
            .saturating_add(native.actions.len())
            .saturating_add(native.unresolved.len()),
    )?;
    let mut actions = Vec::with_capacity(native.actions.len());
    for kind in &native.actions {
        export.charge(0)?;
        actions.push(action(*kind));
    }
    let mut unresolved = Vec::with_capacity(native.unresolved.len());
    for reason in &native.unresolved {
        export.charge(0)?;
        unresolved.push(boundary(*reason));
    }
    export.charge(0)?;
    Ok(api::ProcessTransfer {
        repeat: native.repeat,
        actions,
        unresolved,
    })
}

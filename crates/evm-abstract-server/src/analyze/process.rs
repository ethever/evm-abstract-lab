//! Typed process projection; preserve action, target and uncovered-rule distinctions.
mod creation;
mod frame;
use super::symbolic::Pool;
pub(super) use creation::{completion, continuation};
use evm_abstract::analysis;
use evm_abstract_protocol as api;
pub(super) use frame::{invocation, key, location, operands, source};

pub(super) fn semantics(native: analysis::TransferSemantics) -> api::TransferSemantics {
    match native {
        analysis::TransferSemantics::Native => api::TransferSemantics::Native,
        analysis::TransferSemantics::Environment => api::TransferSemantics::Environment,
        analysis::TransferSemantics::KnownProcess => api::TransferSemantics::KnownProcess,
        analysis::TransferSemantics::ConstructorProcess => {
            api::TransferSemantics::ConstructorProcess
        }
        analysis::TransferSemantics::BoundaryProcess => api::TransferSemantics::BoundaryProcess,
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
        analysis::ProcessOwner::RootNamespace => api::ProcessOwner::RootNamespace,
        analysis::ProcessOwner::Named(address) => api::ProcessOwner::Named(address.to_string()),
        analysis::ProcessOwner::Residual => api::ProcessOwner::Residual,
    }
}

pub(super) fn boundary(native: analysis::ProcessBoundary) -> api::ProcessBoundary {
    match native {
        analysis::ProcessBoundary::Catalogue => api::ProcessBoundary::Catalogue,
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
        4usize
            .saturating_add(native.creations.len())
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
        control: native
            .control
            .as_ref()
            .map(|region| export.control_region(region))
            .transpose()?,
        repeat: native.repeat,
        actions,
        creations: native
            .creations
            .iter()
            .map(|receipt| creation::transfer(receipt, export))
            .collect::<Result<_, _>>()?,
        unresolved,
    })
}

pub(super) fn scope(native: analysis::ProcessControlScope) -> api::ProcessControlScope {
    match native {
        analysis::ProcessControlScope::Primitive => api::ProcessControlScope::Primitive,
    }
}

pub(super) fn invalid(kind: api::ProcessValidationKind) -> api::ApiError {
    api::ApiError {
        code: api::ApiErrorCode::Internal,
        message: format!("native process representation is inconsistent: {kind:?}"),
        details: api::ErrorDetails::Report(api::ReportValidationFailure::Process(kind)),
    }
}

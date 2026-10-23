//! User-facing process terms keep possible control and uncovered rules explicit.
use evm_abstract_protocol::{ProcessAction, ProcessBoundary, ProcessOwner, TransferSemantics};

pub(super) fn interpretation(semantics: TransferSemantics) -> &'static str {
    match semantics {
        TransferSemantics::Native => "Native execution",
        TransferSemantics::Environment => "Environment abstraction",
        TransferSemantics::KnownProcess => "Conservative process · possible control",
    }
}

pub(super) fn action(kind: ProcessAction) -> &'static str {
    match kind {
        ProcessAction::Call => "CALL",
        ProcessAction::StaticCall => "STATICCALL",
        ProcessAction::DelegateCall => "DELEGATECALL",
        ProcessAction::CallCode => "CALLCODE",
        ProcessAction::Create => "CREATE",
        ProcessAction::Create2 => "CREATE2",
    }
}

pub(super) fn owner(target: &ProcessOwner) -> String {
    match target {
        ProcessOwner::Named(address) => address.clone(),
        ProcessOwner::Residual => "Residual owner class · unresolved aliases".into(),
    }
}

pub(super) fn boundary(reason: &ProcessBoundary) -> String {
    match reason {
        ProcessBoundary::ResidualOwner(kind) => format!(
            "{}: remaining owners and aliases are unexpanded",
            action(*kind)
        ),
        ProcessBoundary::Creation(kind) => format!(
            "{}: constructor and deployment are unexpanded",
            action(*kind)
        ),
        ProcessBoundary::CooperativeInputs => "Cooperative inputs are unexpanded".into(),
        ProcessBoundary::SymbolicOwner => {
            "Symbolic logical owner and aliases are unexpanded".into()
        }
        ProcessBoundary::NativeInstantiation => {
            "Process input has no native execution representation".into()
        }
    }
}

//! User-facing process terms keep possible control and uncovered rules explicit.
use evm_abstract_protocol::{ProcessAction, ProcessBoundary, ProcessOwner, TransferSemantics};

pub(super) fn interpretation(semantics: TransferSemantics) -> &'static str {
    match semantics {
        TransferSemantics::Native => "Native execution",
        TransferSemantics::Environment => "Environment abstraction",
        TransferSemantics::KnownProcess => "Conservative process · possible control",
        TransferSemantics::ConstructorProcess => "Constructor process · before runtime deployment",
        TransferSemantics::BoundaryProcess => "Process boundary · possible control",
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
        ProcessOwner::RootNamespace => "Root input owner · possible aliases".into(),
        ProcessOwner::Named(address) => address.clone(),
        ProcessOwner::Residual => "Residual owner class · unresolved aliases".into(),
    }
}

pub(super) fn boundary(reason: &ProcessBoundary) -> String {
    match reason {
        ProcessBoundary::Catalogue => {
            "Precise source requires a current owner/code catalogue".into()
        }
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

pub(super) fn frame(ui: &mut egui::Ui, key: &evm_abstract_protocol::ProcessFrameKey) {
    ui.label(interpretation(key.source.semantics()));
    ui.monospace(format!("Owner class: {}", owner(&key.owner)));
    ui.monospace(format!("Catalogue epoch: {}", key.epoch));
    ui.label(format!("Static: {}", key.is_static));
    source(ui, &key.source);
}

pub(super) fn source(ui: &mut egui::Ui, source: &evm_abstract_protocol::ProcessSource) {
    use evm_abstract_protocol::ProcessSource as Source;
    match source {
        Source::InputProgram(source) => {
            ui.label("Supplied input program · no physical code account");
            ui.monospace(format!("Input program hash: {}", source.hash));
            ui.label(format!("Fork: {:?}", source.fork));
        }
        Source::InputDelegation(source) => {
            ui.label("Supplied delegation marker · target code unobserved");
            ui.monospace(format!("Delegation target: {}", source.target));
            ui.monospace(format!("Marker hash: {}", source.hash));
            ui.label(format!("Fork: {:?}", source.fork));
        }
        Source::Runtime(source) => {
            ui.label("Captured runtime source");
            ui.monospace(format!("Code account: {}", source.source));
            ui.monospace(format!("Captured code hash: {}", source.hash));
            ui.label(format!("Fork: {:?}", source.fork));
        }
        Source::InitCode(source) => {
            ui.label("Captured constructor code · not yet deployed runtime");
            ui.monospace(format!("Initcode hash: {}", source.hash));
            ui.label(format!("Fork: {:?}", source.fork));
        }
        Source::AnyRuntime(source) => {
            ui.label(format!("Unobserved runtime · {:?}", source.fork));
        }
        Source::AnyInitCode(source) => {
            ui.label(format!("Unobserved constructor code · {:?}", source.fork));
        }
        Source::Empty(source) => {
            ui.label(format!("Empty execution · {:?}", source.fork));
        }
        Source::Precompile(source) => {
            ui.label(format!(
                "Native precompile: {} · {:?}",
                source.address, source.fork
            ));
        }
        Source::InvalidDelegation(source) => {
            ui.label("Invalid delegation source");
            ui.monospace(format!("Code account: {}", source.source));
            ui.monospace(format!("Source identity hash: {}", source.hash));
            ui.label(format!("Fork: {:?}", source.fork));
        }
    }
}

pub(super) fn creation(ui: &mut egui::Ui, source: &evm_abstract_protocol::CreationContinuation) {
    match source {
        evm_abstract_protocol::CreationContinuation::Native(source) => {
            ui.monospace(format!("Creation target: {}", source.destination));
        }
        evm_abstract_protocol::CreationContinuation::Process(source) => {
            ui.label(format!("Constructor action: {}", action(source.action)));
            ui.monospace(format!("Creator class: {}", owner(&source.creator)));
            ui.monospace(format!("Destination class: {}", owner(&source.destination)));
            ui.monospace(format!("Creation epoch: {}", source.epoch));
        }
    }
}

pub(super) fn completion(ui: &mut egui::Ui, case: &evm_abstract_protocol::ProcessCompletionCase) {
    let result = match &case.result {
        evm_abstract_protocol::ProcessCreationResult::Address(target) => {
            format!("Created address class: {}", owner(target))
        }
        evm_abstract_protocol::ProcessCreationResult::Zero => "Creation result: zero".into(),
    };
    ui.label(result);
    ui.label(format!(
        "Parent returndata: {:?} · Store: {:?} · {:?}",
        case.returndata, case.store, case.reason
    ));
    if let Some(deployment) = &case.deployment {
        ui.label(format!(
            "Deployed valid-runtime class: {} · size ≤ {} · first byte excludes 0x{:02x}",
            owner(&deployment.owner),
            deployment.maximum_runtime_size,
            deployment.forbidden_first_byte
        ));
    }
}

use evm_abstract::{analysis, bytecode};
use evm_abstract_protocol::{DiagnosticKind, DisasmInstruction, EdgeKind, FrontierKind};

pub(super) fn instruction(source: &bytecode::Instruction) -> DisasmInstruction {
    let (stack_inputs, stack_outputs) = source.stack_io();
    DisasmInstruction {
        pc: source.pc,
        opcode: source.opcode,
        name: source.name().into(),
        immediate: source.immediate.map(|value| format!("0x{value:x}")),
        size: source.size,
        valid: source.is_valid(),
        stack_inputs,
        stack_outputs,
    }
}

pub(super) fn edge(source: analysis::MachineEdgeKind) -> EdgeKind {
    match source {
        analysis::MachineEdgeKind::Intraprocedural(kind) => match kind {
            analysis::EdgeKind::Fallthrough => EdgeKind::Fallthrough,
            analysis::EdgeKind::Jump => EdgeKind::Jump,
            analysis::EdgeKind::BranchTrue => EdgeKind::BranchTrue,
            analysis::EdgeKind::BranchFalse => EdgeKind::BranchFalse,
        },
        analysis::MachineEdgeKind::Call => EdgeKind::Call,
        analysis::MachineEdgeKind::Return => EdgeKind::Return,
        analysis::MachineEdgeKind::Failure => EdgeKind::Failure,
        analysis::MachineEdgeKind::Revert => EdgeKind::Revert,
    }
}

pub(super) fn diagnostic(source: &analysis::DiagnosticKind) -> DiagnosticKind {
    match source {
        analysis::DiagnosticKind::UnknownJump => DiagnosticKind::UnknownJump,
        analysis::DiagnosticKind::InvalidJump => DiagnosticKind::InvalidJump,
        analysis::DiagnosticKind::StackUnderflow => DiagnosticKind::StackUnderflow,
        analysis::DiagnosticKind::StackOverflow => DiagnosticKind::StackOverflow,
        analysis::DiagnosticKind::InvalidOpcode => DiagnosticKind::InvalidOpcode,
        analysis::DiagnosticKind::OpaqueResult => DiagnosticKind::OpaqueResult,
        analysis::DiagnosticKind::FactExchangeLimited(_) => DiagnosticKind::FactExchangeLimited,
    }
}

fn limit(source: analysis::Limit) -> FrontierKind {
    match source {
        analysis::Limit::States => FrontierKind::States,
        analysis::Limit::Transfers => FrontierKind::Transfers,
        analysis::Limit::Work => FrontierKind::Work,
        analysis::Limit::CallDepth => FrontierKind::CallDepth,
        analysis::Limit::Memory => FrontierKind::Memory,
        analysis::Limit::Model => FrontierKind::Model,
    }
}

pub(super) fn frontier(source: &analysis::FrontierReason) -> FrontierKind {
    match source {
        analysis::FrontierReason::Budget(bound) => limit(*bound),
        analysis::FrontierReason::Work | analysis::FrontierReason::SummaryWork => {
            FrontierKind::Work
        }
        analysis::FrontierReason::CallDepth => FrontierKind::CallDepth,
        analysis::FrontierReason::Memory => FrontierKind::Memory,
        analysis::FrontierReason::Relations(_) => FrontierKind::Relations,
        analysis::FrontierReason::UnknownTarget => FrontierKind::UnknownTarget,
        analysis::FrontierReason::MissingCode(_) => FrontierKind::MissingCode,
        analysis::FrontierReason::MissingStorage { .. } => FrontierKind::MissingStorage,
        analysis::FrontierReason::RpcAcquisition { .. } => FrontierKind::RpcAcquisition,
        analysis::FrontierReason::Precompile(_) => FrontierKind::Precompile,
        analysis::FrontierReason::PrecompileInput(_) => FrontierKind::PrecompileInput,
        analysis::FrontierReason::Creation(_) => FrontierKind::Creation,
        analysis::FrontierReason::UnsupportedOpcode(_) => FrontierKind::UnsupportedOpcode,
    }
}

pub(super) fn reduction(
    source: &analysis::DiagnosticKind,
) -> Option<evm_abstract_protocol::ReductionReason> {
    use evm_abstract::domain::ReductionStatus as N;
    use evm_abstract_protocol::ReductionReason as P;
    let analysis::DiagnosticKind::FactExchangeLimited(reason) = source else {
        return None;
    };
    Some(match reason {
        N::Stable => P::Stable,
        N::RoundLimit => P::RoundLimit,
        N::FactLimit => P::FactLimit,
        N::Empty => P::Empty,
        N::OriginConflict => P::OriginConflict,
        N::SymbolicLimit => P::SymbolicLimit,
    })
}
pub(super) fn reason(source: &analysis::FrontierReason) -> evm_abstract_protocol::FrontierDetails {
    use evm_abstract::analysis::{CreationBoundary as NC, FrontierReason as N};
    use evm_abstract::domain::relational::QueryReason as NQ;
    use evm_abstract_protocol::{CreationReason as C, FrontierDetails as P, QueryBoundary as Q};
    match source {
        N::Budget(bound) => P::Budget(limit(*bound)),
        N::Work => P::Work,
        N::SummaryWork => P::SummaryWork,
        N::CallDepth => P::CallDepth,
        N::Memory => P::Memory,
        N::UnknownTarget => P::UnknownTarget,
        N::MissingCode(address) => P::MissingCode(address.to_string()),
        N::MissingStorage { address, slot } => {
            P::MissingStorage(evm_abstract_protocol::StorageLocation {
                address: address.to_string(),
                slot: super::value::word(*slot),
            })
        }
        N::RpcAcquisition { address, failure } => {
            let mut report = super::rpc::failure(failure);
            report.account = Some(address.to_string());
            P::RpcAcquisition(Box::new(report))
        }
        N::Precompile(address) => P::Precompile(address.to_string()),
        N::PrecompileInput(address) => P::PrecompileInput(address.to_string()),
        N::UnsupportedOpcode(op) => P::UnsupportedOpcode(*op),
        N::Relations(reason) => P::Relations(match reason {
            NQ::Cancelled => Q::Cancelled,
            NQ::Disabled => Q::Disabled,
            NQ::Configuration => Q::Configuration,
            NQ::ConstraintLimit => Q::ConstraintLimit,
            NQ::ScalarFactLimit => Q::ScalarFactLimit,
            NQ::ScalarFactError(message) => Q::ScalarFactError(message.clone()),
            NQ::ExpressionLimit => Q::ExpressionLimit,
            NQ::Unsupported(op) => Q::Unsupported(*op),
            NQ::ResourceLimit => Q::ResourceLimit,
            NQ::SolverUnknown(message) => Q::SolverUnknown(message.clone()),
            NQ::ModelUnavailable => Q::ModelUnavailable,
        }),
        N::Creation(reason) => P::Creation(match reason {
            NC::UnknownCreator => C::UnknownCreator,
            NC::UnknownNonce(address) => C::UnknownNonce(address.to_string()),
            NC::UnknownCollision(address) => C::UnknownCollision(address.to_string()),
            NC::UnknownEndowment => C::UnknownEndowment,
            NC::UnknownInitCode => C::UnknownInitCode,
            NC::UnknownRuntimeCode => C::UnknownRuntimeCode,
            NC::UnknownSalt => C::UnknownSalt,
            NC::NonceOverflow(address) => C::NonceOverflow(address.to_string()),
            NC::ReservedAddress(address) => C::ReservedAddress(address.to_string()),
        }),
    }
}

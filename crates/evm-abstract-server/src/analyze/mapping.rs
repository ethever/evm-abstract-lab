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

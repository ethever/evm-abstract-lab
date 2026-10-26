use evm_abstract::{analysis, bytecode};
use evm_abstract_protocol as api;
use evm_abstract_protocol::{DiagnosticKind, DisasmInstruction, EdgeKind, FrontierKind};

pub(super) fn fork(source: evm_abstract::Fork) -> api::Fork {
    match source {
        evm_abstract::Fork::Cancun => api::Fork::Cancun,
        evm_abstract::Fork::Prague => api::Fork::Prague,
        evm_abstract::Fork::Osaka => api::Fork::Osaka,
    }
}

pub(super) fn root_binding(source: evm_abstract::world::RootInputBinding) -> api::RootInputBinding {
    match source {
        evm_abstract::world::RootInputBinding::Physical { address } => {
            api::RootInputBinding::Physical(api::PhysicalRootInput {
                address: address.to_string(),
            })
        }
        evm_abstract::world::RootInputBinding::Namespace { coordinate } => {
            api::RootInputBinding::Namespace(api::NamespaceRootInput {
                coordinate: coordinate.to_string(),
            })
        }
    }
}

pub(super) fn execution_config(source: &analysis::ExecutionConfig) -> api::RootTerminalConfig {
    api::RootTerminalConfig {
        context_depth: source.analysis.context_depth as u64,
        max_states: source.analysis.max_states as u64,
        max_transfers: source.analysis.max_transfers as u64,
        max_work: source.max_work as u64,
        max_call_depth: source.max_call_depth as u64,
        max_memory_bytes: source.max_memory_bytes as u64,
        use_summaries: source.use_summaries,
        root_terminal_coverage: source.root_terminal_coverage,
    }
}

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
        analysis::MachineEdgeKind::ProcessControl => EdgeKind::ProcessControl,
        analysis::MachineEdgeKind::Process(action) => {
            EdgeKind::Process(super::process::action(action))
        }
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
        analysis::MachineEdgeKind::Summary(analysis::OutcomeKind::Return) => {
            EdgeKind::SummaryReturn
        }
        analysis::MachineEdgeKind::Summary(analysis::OutcomeKind::Revert) => {
            EdgeKind::SummaryRevert
        }
        analysis::MachineEdgeKind::Summary(analysis::OutcomeKind::Failure) => {
            EdgeKind::SummaryFailure
        }
    }
}

pub(super) fn diagnostic(source: &analysis::DiagnosticKind) -> DiagnosticKind {
    match source {
        analysis::DiagnosticKind::UnknownJump => DiagnosticKind::UnknownJump,
        analysis::DiagnosticKind::InvalidJump => DiagnosticKind::InvalidJump,
        analysis::DiagnosticKind::StackUnderflow => DiagnosticKind::StackUnderflow,
        analysis::DiagnosticKind::StackOverflow => DiagnosticKind::StackOverflow,
        analysis::DiagnosticKind::InvalidOpcode(_) => DiagnosticKind::InvalidOpcode,
        analysis::DiagnosticKind::OpaqueResult => DiagnosticKind::OpaqueResult,
        analysis::DiagnosticKind::RelationalPrecisionLost(_) => {
            DiagnosticKind::RelationalPrecisionLost
        }
        analysis::DiagnosticKind::FactExchangeLimited(_) => DiagnosticKind::FactExchangeLimited,
    }
}

pub(super) fn invalid_opcode(
    source: &analysis::DiagnosticKind,
) -> Option<evm_abstract_protocol::InvalidOpcodeDiagnostic> {
    use analysis::{InvalidOpcodeDiagnostic as N, InvalidOpcodeReason as NR};
    use evm_abstract_protocol::{
        InvalidDelegationEntry, InvalidInstruction, InvalidOpcodeDiagnostic as P,
        InvalidOpcodeReason as PR,
    };
    let analysis::DiagnosticKind::InvalidOpcode(source) = source else {
        return None;
    };
    Some(match source {
        N::Instruction {
            opcode,
            mnemonic,
            reason,
        } => P::Instruction(InvalidInstruction {
            opcode: *opcode,
            mnemonic: mnemonic.clone(),
            reason: match reason {
                NR::ExplicitInvalid => PR::ExplicitInvalid,
                NR::UndefinedOpcode => PR::UndefinedOpcode,
                NR::UnavailableInFork(fork) => PR::UnavailableInFork(match fork {
                    evm_abstract::Fork::Cancun => evm_abstract_protocol::Fork::Cancun,
                    evm_abstract::Fork::Prague => evm_abstract_protocol::Fork::Prague,
                    evm_abstract::Fork::Osaka => evm_abstract_protocol::Fork::Osaka,
                }),
            },
        }),
        N::DelegationEntry {
            opcode,
            detail,
            byte_unavailable,
        } => P::DelegationEntry(InvalidDelegationEntry {
            opcode: *opcode,
            detail: detail.clone(),
            byte_unavailable: byte_unavailable.clone(),
        }),
    })
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
        analysis::FrontierReason::RecursiveNoTerminal => FrontierKind::Model,
        analysis::FrontierReason::Process(_) => FrontierKind::Model,
        analysis::FrontierReason::Memory => FrontierKind::Memory,
        analysis::FrontierReason::ByteRange(_) => FrontierKind::ByteRange,
        analysis::FrontierReason::StateJoin(_) => FrontierKind::StateJoin,
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
        N::WorkLimit => P::WorkLimit,
        N::Cancelled => P::Cancelled,
        N::KernelFailure => P::KernelFailure,
    })
}
pub(super) fn reason(source: &analysis::FrontierReason) -> evm_abstract_protocol::FrontierDetails {
    use evm_abstract::analysis::{CreationBoundary as NC, FrontierReason as N};
    use evm_abstract_protocol::{CreationReason as C, FrontierDetails as P};
    match source {
        N::Process(reason) => P::Process(super::process::boundary(*reason)),
        N::Budget(bound) => P::Budget(limit(*bound)),
        N::Work => P::Work,
        N::SummaryWork => P::SummaryWork,
        N::RecursiveNoTerminal => P::RecursiveNoTerminal,
        N::CallDepth => P::CallDepth,
        N::Memory => P::Memory,
        N::ByteRange(reason) => P::ByteRange(match reason {
            analysis::ByteRangeBoundary::UnknownSize => {
                evm_abstract_protocol::ByteRangeBoundary::UnknownSize
            }
            analysis::ByteRangeBoundary::OffsetTooLarge(offset) => {
                evm_abstract_protocol::ByteRangeBoundary::OffsetTooLarge(super::value::word(
                    *offset,
                ))
            }
            analysis::ByteRangeBoundary::Overflow { offset, size } => {
                evm_abstract_protocol::ByteRangeBoundary::Overflow(
                    evm_abstract_protocol::ByteRangeOverflow {
                        offset: *offset,
                        size: *size,
                    },
                )
            }
            analysis::ByteRangeBoundary::TooLarge { requested, limit } => {
                evm_abstract_protocol::ByteRangeBoundary::TooLarge(
                    evm_abstract_protocol::ByteRangeLimit {
                        requested: super::value::word(*requested),
                        limit: *limit,
                    },
                )
            }
            analysis::ByteRangeBoundary::InputAlreadyBound => {
                evm_abstract_protocol::ByteRangeBoundary::InputAlreadyBound
            }
        }),
        N::StateJoin(reason) => P::StateJoin(match reason {
            analysis::JoinBoundary::Shape => evm_abstract_protocol::JoinBoundary::Shape,
            analysis::JoinBoundary::IdentityExhausted => {
                evm_abstract_protocol::JoinBoundary::IdentityExhausted
            }
            analysis::JoinBoundary::Alternatives { limit } => {
                evm_abstract_protocol::JoinBoundary::Alternatives(*limit)
            }
        }),
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
        N::Relations(reason) => P::Relations(query(reason)),
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

pub(super) fn fact_failure(
    source: &evm_abstract::domain::facts::FactError,
) -> evm_abstract_protocol::FactFailure {
    use evm_abstract::domain::facts::FactError as N;
    use evm_abstract_protocol::{FactCapacityFailure, FactFailure as P, FactOperationFailure};
    match source {
        N::InvalidBitIndex(index) => P::InvalidBitIndex(*index),
        N::ConflictingBits => P::ConflictingBits,
        N::InvalidBounds => P::InvalidBounds,
        N::EmptyFiniteSet => P::EmptyFiniteSet,
        N::InvalidModulus => P::InvalidModulus,
        N::UnsupportedOperation(opcode) => P::UnsupportedOperation(*opcode),
        N::InvalidOperation {
            opcode,
            expected,
            actual,
        } => P::InvalidOperation(FactOperationFailure {
            opcode: *opcode,
            expected: *expected,
            actual: *actual,
        }),
        N::Contradiction { subject } => P::Contradiction(subject.index()),
        N::OriginContradiction { subject } => P::OriginContradiction(subject.index()),
        N::Capacity {
            max_atoms,
            required,
        } => P::Capacity(FactCapacityFailure {
            max_atoms: *max_atoms,
            required: *required,
        }),
    }
}

pub(super) fn precision(
    source: &analysis::DiagnosticKind,
) -> Option<evm_abstract_protocol::QueryBoundary> {
    if let analysis::DiagnosticKind::RelationalPrecisionLost(reason) = source {
        Some(query(reason))
    } else {
        None
    }
}
pub(super) fn query(
    reason: &evm_abstract::domain::relational::QueryReason,
) -> evm_abstract_protocol::QueryBoundary {
    use evm_abstract::domain::relational::QueryReason as NQ;
    use evm_abstract_protocol::QueryBoundary as Q;
    match reason {
        NQ::Cancelled => Q::Cancelled,
        NQ::Disabled => Q::Disabled,
        NQ::Configuration => Q::Configuration,
        NQ::ConstraintLimit => Q::ConstraintLimit,
        NQ::ScalarFactLimit => Q::ScalarFactLimit,
        NQ::ScalarFactError(error) => Q::ScalarFactError(fact_failure(error)),
        NQ::ExpressionLimit => Q::ExpressionLimit,
    }
}
pub(super) fn instruction_progress(
    source: analysis::InstructionProgress,
) -> api::InstructionProgress {
    match source {
        analysis::InstructionProgress::Started => api::InstructionProgress::Started,
        analysis::InstructionProgress::OperandsConsumed => {
            api::InstructionProgress::OperandsConsumed
        }
        analysis::InstructionProgress::Completed => api::InstructionProgress::Completed,
        analysis::InstructionProgress::Dispatched => api::InstructionProgress::Dispatched,
        analysis::InstructionProgress::Faulted => api::InstructionProgress::Faulted,
    }
}

/// Public frame identity shared by local prefixes and regional control evidence.
/// Process frames expose only their authoritative key; Native anchor fields are masked.
pub(super) fn regional_frame(
    frame: &analysis::FrameKey,
    native_semantics: bool,
) -> api::RegionalFrame {
    let process = frame.process.as_deref().map(super::process::key);
    let is_native = process.is_none();
    let bytecode = is_native
        && native_semantics
        && matches!(
            frame.mode,
            analysis::FrameCode::Runtime | analysis::FrameCode::InitCode
        );
    api::RegionalFrame {
        code_address: is_native.then(|| frame.code_address.to_string()),
        code_hash: (is_native && frame.mode != analysis::FrameCode::Unknown)
            .then(|| frame.code_hash.to_string()),
        storage_address: is_native.then(|| frame.address.to_string()),
        address_value: is_native.then(|| super::value::address(frame.address_value)),
        caller: is_native.then(|| super::value::address(frame.caller)),
        kind: process
            .as_ref()
            .map_or_else(|| super::pools::mode(frame.mode), |key| key.source.kind()),
        is_static: process
            .as_ref()
            .map_or(frame.is_static, |key| key.is_static),
        basic_block: bytecode.then_some(frame.basic_block_index),
        stack_height: is_native.then_some(frame.stack_height),
        context: if is_native {
            frame.jump_history.clone()
        } else {
            Vec::new()
        },
        process,
    }
}

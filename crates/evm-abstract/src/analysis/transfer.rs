//! Basic-block execution on a transaction-wide machine. Calls suspend a frame;
//! returns resume it with the same transaction store and an explicit result.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::engine::policy::ContextPolicy;
use super::{
    DiagnosticKind, EdgeKind, InvalidOpcodeDiagnostic,
    machine::{
        EvmMachinePayload, ExecutionConfig, FrameCode, FrontierReason, MachineEdgeKind, MachineKey,
        OutcomeKind,
    },
};
use super::{EvmInstructionValue, InstructionProgress};

use crate::{
    domain::{AlgebraRef, EvmValue, provenance::Origin},
    world::{
        AddressInput, Code, EvmByteArray, EvmEntry, EvmLog, EvmStore, EvmWorld, LogKey,
        LogSourceKind, Symbol,
    },
};
use alloy_primitives::{Address, U256, keccak256};
use revm_bytecode::opcode;
use std::sync::atomic::{AtomicU64, Ordering};

// 仅用于本进程的临时复制身份。计数耗尽时丢弃这项可选精度，不复用编号。
static NEXT_IDENTITY_SCOPE: AtomicU64 = AtomicU64::new(1);
fn identity_scope() -> Option<u64> {
    NEXT_IDENTITY_SCOPE
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |v| v.checked_add(1))
        .ok()
}

mod budget;
mod calls;
mod candidates;
use candidates::complete as complete_candidates;
pub(super) mod create;
mod errors;
mod hash;
#[cfg(test)]
mod injection_tests;
mod late_failure;
mod precompile;
pub(super) mod procedure;
mod ranges;
mod relations;
#[cfg(test)]
mod resource_tests;
#[cfg(test)]
mod scalar_retention_tests;

pub(super) use crate::resource::WorkBudget;
use budget::{byte_work, operation_work_budgeted};
use calls::{call, failure, finish};

struct TransferContext<'a, D: EvmNumericAlgebra> {
    config: &'a ExecutionConfig,
    domain: AlgebraRef<'a, D>,
    budget: &'a mut WorkBudget,
    root_address: Option<(Address, AddressInput)>,
    input_scope: Option<u64>,
    query_cache: relations::QueryCache<D::Element>,
    /// Relative procedures finish without committing transaction lifecycle effects.
    procedure: bool,
    /// RPC discovery leaves concrete missing code for acquisition before abstraction.
    discover_code: bool,
}

/// RPC 发现先补初始事实；普通离线分析继续对缺失事实做保守传播。
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum StorageReadPolicy {
    Abstract,
    Discover,
}

pub(super) fn initial<D: EvmNumericAlgebra>(
    world: &EvmWorld<D::Element>,
    entry: &EvmEntry<D::Element>,
    domain: AlgebraRef<'_, D>,
) -> Result<EvmMachinePayload<D::Element>, FrontierReason> {
    calls::initial(world, entry, domain)
}

pub(super) struct EvmSuccessor<N: WordElement> {
    pub payload: EvmMachinePayload<N>,
    pub kind: MachineEdgeKind,
}
pub(super) struct EvmOutcome<N: WordElement> {
    pub kind: OutcomeKind,
    pub data: EvmByteArray<N>,
    pub store: EvmStore<N>,
}
pub(super) struct EvmExecution<N: WordElement> {
    pub payload: EvmMachinePayload<N>,
    pub executed_pcs: Vec<usize>,
    pub instruction_progress: Vec<InstructionProgress>,
    pub instruction_values: Vec<EvmInstructionValue<N>>,
    pub successors: Vec<EvmSuccessor<N>>,
    pub diagnostics: Vec<(usize, DiagnosticKind)>,
    pub frontiers: Vec<EvmExecutionFrontier<N>>,
    pub outcomes: Vec<EvmOutcome<N>>,
    pub completed_calls: Vec<EvmCompletedCall<N>>,
    /// Raw finite retention failures, indexed into this execution's frontiers.
    /// Public reasons alone cannot distinguish identity exhaustion or semantics.
    pub retention_failures: Vec<(usize, RetentionCause)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RetentionCause {
    Expression,
    State,
}

fn empty_execution<N: WordElement>(payload: EvmMachinePayload<N>) -> EvmExecution<N> {
    EvmExecution {
        payload,
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: Vec::new(),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct EvmExecutionFrontier<N: WordElement> {
    pub pc: usize,
    pub reason: FrontierReason,
    pub target: Option<MachineKey>,
    pub range: Option<super::EvmRangeWitness<N>>,
    /// Relative procedure state left unexpanded, when a folded execution stops.
    pub payload: Option<std::sync::Arc<EvmMachinePayload<N>>>,
}

impl<N: WordElement> EvmExecution<N> {
    fn mark_instruction(&mut self, progress: InstructionProgress) {
        *self
            .instruction_progress
            .last_mut()
            .expect("visited instruction") = progress;
    }
}

/// A specific child terminal before caller joins, with qualified world effects.
#[derive(Clone, Debug)]
pub(super) struct EvmCompletedCall<N: WordElement> {
    pub payload: EvmMachinePayload<N>,
    pub kind: OutcomeKind,
    pub data: EvmByteArray<N>,
    pub output_store: EvmStore<N>,
    pub output_kind: OutcomeKind,
    pub output_data: EvmByteArray<N>,
}

// Concrete aliases keep existing default regression fixtures unambiguous.
#[cfg(test)]
pub(super) type Execution = EvmExecution<crate::domain::NumericValue>;
#[cfg(test)]
pub(super) type Successor = EvmSuccessor<crate::domain::NumericValue>;
#[cfg(test)]
pub(super) type Outcome = EvmOutcome<crate::domain::NumericValue>;
#[cfg(test)]
pub(super) type ExecutionFrontier = EvmExecutionFrontier<crate::domain::NumericValue>;
#[cfg(test)]
pub(super) type CompletedCall = EvmCompletedCall<crate::domain::NumericValue>;

pub(super) fn resume_summary<D: EvmNumericAlgebra>(
    payload: EvmMachinePayload<D::Element>,
    kind: OutcomeKind,
    data: EvmByteArray<D::Element>,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
) -> EvmExecution<D::Element> {
    resume_summary_inner(payload, kind, data, config, domain, budget, false)
}

fn resume_summary_inner<D: EvmNumericAlgebra>(
    payload: EvmMachinePayload<D::Element>,
    kind: OutcomeKind,
    data: EvmByteArray<D::Element>,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    budget: &mut WorkBudget,
    procedure: bool,
) -> EvmExecution<D::Element> {
    let mut result = EvmExecution {
        payload: payload.clone(),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: Vec::new(),
    };
    let mut context = TransferContext {
        config,
        domain,
        budget,
        root_address: None,
        input_scope: None,
        query_cache: Default::default(),
        procedure,
        discover_code: false,
    };
    let pc = payload
        .active()
        .program
        .as_ref()
        .and_then(|p| p.blocks().get(payload.active().key.basic_block_index))
        .and_then(|b| b.instructions.last())
        .map_or(0, |i| i.pc);
    finish(&mut result, payload, kind, data, &mut context, pc);
    result
}

fn address(value: U256) -> Address {
    Address::from_slice(&value.to_be_bytes::<32>()[12..])
}
fn constant<N: WordElement>(value: usize) -> EvmValue<N> {
    EvmValue::<N>::constant(U256::from(value))
}
fn zero<N: WordElement>() -> EvmValue<N> {
    EvmValue::<N>::constant(U256::ZERO)
}
fn boundary<N: WordElement>(result: &mut EvmExecution<N>, pc: usize, reason: FrontierReason) {
    boundary_on(result, pc, reason, result.payload.key());
}

fn boundary_reason(reason: FrontierReason) -> FrontierReason {
    // The shared ledger declines both exhausted work and cancellation. Keep
    // the actual interruption distinct at every transfer frontier, including
    // callers that return a raw Work error before a frontier is constructed.
    if reason == FrontierReason::Work && analysis_control::current_cancellation().is_cancelled() {
        FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
    } else {
        reason
    }
}

fn boundary_on<N: WordElement>(
    result: &mut EvmExecution<N>,
    pc: usize,
    reason: FrontierReason,
    target: MachineKey,
) {
    result.frontiers.push(EvmExecutionFrontier {
        pc,
        reason: boundary_reason(reason),
        target: Some(target),
        range: None,
        payload: None,
    });
}

fn boundary_bytes<N: WordElement>(
    result: &mut EvmExecution<N>,
    pc: usize,
    error: crate::world::RangeError,
) {
    let cause = errors::retention_bytes(&error);
    let index = result.frontiers.len();
    boundary(result, pc, errors::bytes(error));
    if let Some(cause) = cause {
        result.retention_failures.push((index, cause));
    }
}

fn boundary_store<N: WordElement>(
    result: &mut EvmExecution<N>,
    pc: usize,
    error: crate::world::StoreError,
) {
    let cause = errors::retention_store(&error);
    let index = result.frontiers.len();
    boundary(result, pc, errors::store(error));
    if let Some(cause) = cause {
        result.retention_failures.push((index, cause));
    }
}

fn successor<N: WordElement>(
    result: &mut EvmExecution<N>,
    mut payload: EvmMachinePayload<N>,
    basic_block_index: usize,
    kind: EdgeKind,
) {
    payload.active_mut().key.basic_block_index = basic_block_index;
    payload.normalize();
    result.successors.push(EvmSuccessor {
        payload,
        kind: MachineEdgeKind::Intraprocedural(kind),
    });
}

fn touch_memory<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    if !ranges::memory(result, offset, size, context, pc) {
        return false;
    }
    expand_memory(result, offset, size, context, pc)
}

fn expand_memory<D: EvmNumericAlgebra>(
    result: &mut EvmExecution<D::Element>,
    offset: &EvmValue<D::Element>,
    size: &EvmValue<D::Element>,
    context: &mut TransferContext<'_, D>,
    pc: usize,
) -> bool {
    let config = context.config;
    let domain = context.domain;
    if !context.budget.charge(byte_work(
        &[offset, size],
        0,
        result.payload.active().memory.work_size(),
        domain,
    )) {
        boundary(result, pc, FrontierReason::Work);
        return false;
    }
    if let Err(error) = result.payload.active_mut().memory.expand_with(
        offset,
        size,
        config.max_memory_bytes,
        domain,
    ) {
        boundary_bytes(result, pc, error);
        return false;
    }
    true
}

fn code_bytes<N: WordElement>(store: &EvmStore<N>, target: Address) -> Option<Vec<u8>> {
    store.raw_account_code(target)
}

fn environment_targets<D: EvmNumericAlgebra>(
    value: &EvmValue<D::Element>,
    domain: AlgebraRef<'_, D>,
    entry: &EvmEntry<D::Element>,
    mut read: impl FnMut(Address) -> EvmValue<D::Element>,
) -> EvmValue<D::Element> {
    let symbolic_owner = entry.environment.to.as_concrete().is_none();
    if symbolic_owner
        && value.identity().same_identity(
            entry
                .environment
                .address_value(entry.environment.to)
                .identity(),
        )
    {
        return read(entry.address);
    }
    let Some(values) = complete_candidates(value) else {
        return EvmValue::<D::Element>::top();
    };
    values
        .iter()
        .map(|v| {
            let target = address(*v);
            if symbolic_owner && target == entry.address {
                EvmValue::<D::Element>::top()
            } else {
                read(target)
            }
        })
        .reduce(|a, b| domain.join(&a, &b))
        .expect("AbstractValue constant sets are nonempty")
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared metered transfer inputs and immutable context policy"
)]
pub(super) fn execute<D: EvmNumericAlgebra, C: ContextPolicy>(
    world: &EvmWorld<D::Element>,
    entry: &EvmEntry<D::Element>,
    payload: EvmMachinePayload<D::Element>,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    context_policy: &C,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
) -> EvmExecution<D::Element> {
    execute_inner(
        world,
        entry,
        payload,
        config,
        domain,
        context_policy,
        budget,
        storage_reads,
        false,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "shared metered transfer inputs and relative-frame mode"
)]
fn execute_inner<D: EvmNumericAlgebra, C: ContextPolicy>(
    world: &EvmWorld<D::Element>,
    entry: &EvmEntry<D::Element>,
    payload: EvmMachinePayload<D::Element>,
    config: &ExecutionConfig,
    domain: AlgebraRef<'_, D>,
    context_policy: &C,
    budget: &mut WorkBudget,
    storage_reads: StorageReadPolicy,
    procedure: bool,
) -> EvmExecution<D::Element> {
    let mut transfer_context = TransferContext {
        config,
        domain,
        budget,
        root_address: Some((entry.address, entry.environment.to)),
        input_scope: entry.environment.input_scope.id(),
        query_cache: Default::default(),
        procedure,
        discover_code: storage_reads == StorageReadPolicy::Discover,
    };
    let context = &mut transfer_context;
    let mut result = EvmExecution {
        payload,
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: Vec::new(),
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: Vec::new(),
    };
    if !late_failure::at_block_entry(&mut result, context) {
        return result;
    }
    let scope = identity_scope();
    let mut definition = 0_u32;
    // Metadata changes consume each owned value; neither operation needs a
    // duplicate of N. Taking the Vec constructs no placeholder numeric value.
    let stack = std::mem::take(&mut result.payload.active_mut().stack);
    result.payload.active_mut().stack = stack
        .into_iter()
        .map(|mut value| {
            value.forget_identity();
            let mut value = relations::identify(value, config.analysis.relations.enabled);
            if let Some(scope) = scope {
                value = value.with_identity(scope, definition);
            }
            definition = definition.saturating_add(1);
            value
        })
        .collect();
    let code = result.payload.active().code;
    if let FrameCode::Precompile(address) = code {
        precompile::execute(&mut result, world.fork(), address, context);
        return result;
    }
    if code == FrameCode::InvalidDelegation {
        let code_address = result.payload.active().key.code_address;
        let (opcode, byte_unavailable) = match result.payload.store.raw_account_code(code_address) {
            Some(bytes) => match bytes.first().copied() {
                Some(opcode) => (Some(opcode), None),
                None => (
                    None,
                    Some("active code account has no captured byte at entry".to_owned()),
                ),
            },
            None => (
                None,
                Some("active code account's raw bytes are unavailable".to_owned()),
            ),
        };
        result.diagnostics.push((
            0,
            DiagnosticKind::InvalidOpcode(InvalidOpcodeDiagnostic::DelegationEntry {
                opcode,
                detail: "nested delegation indicator cannot be executed".to_owned(),
                byte_unavailable,
            }),
        ));
        failure(&mut result, context, 0);
        return result;
    }
    if code == FrameCode::Empty {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            EvmByteArray::<D::Element>::empty(),
            context,
            0,
        );
        return result;
    }
    let program = result
        .payload
        .active()
        .program
        .clone()
        .expect("runtime and initcode frames capture their executable program");
    let program = &program;
    let basic_block_index = result.payload.active().key.basic_block_index;
    let Some(block) = program.blocks().get(basic_block_index) else {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            EvmByteArray::<D::Element>::empty(),
            context,
            program.byte_len(),
        );
        return result;
    };
    for instruction in &block.instructions {
        let pc = instruction.pc;
        let op = instruction.opcode;
        if !context.budget.charge(1) {
            boundary(&mut result, pc, FrontierReason::Work);
            return result;
        }
        result.executed_pcs.push(pc);
        result
            .instruction_progress
            .push(InstructionProgress::Started);
        if let Some(reason) = instruction.invalid_reason() {
            result.mark_instruction(InstructionProgress::Faulted);
            result.diagnostics.push((
                pc,
                DiagnosticKind::InvalidOpcode(InvalidOpcodeDiagnostic::Instruction {
                    opcode: instruction.opcode,
                    mnemonic: instruction.mnemonic().map(str::to_owned),
                    reason,
                }),
            ));
            failure(&mut result, context, pc);
            return result;
        }
        let (inputs, outputs) = instruction.stack_io();
        let stack = &mut result.payload.active_mut().stack;
        if stack.len() < inputs {
            result.mark_instruction(InstructionProgress::Faulted);
            result
                .diagnostics
                .push((pc, DiagnosticKind::StackUnderflow));
            failure(&mut result, context, pc);
            return result;
        }
        if stack.len() - inputs + outputs > 1024 {
            result.mark_instruction(InstructionProgress::Faulted);
            result.diagnostics.push((pc, DiagnosticKind::StackOverflow));
            failure(&mut result, context, pc);
            return result;
        }
        if let Some(value) = instruction.immediate {
            let construction = domain.literal_quote_work(value);
            if construction == usize::MAX || !context.budget.charge(construction) {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
            let value = EvmValue::<D::Element>::constant(value);
            // Preserve the separately paid receipt copy from the transfer
            // resource lane; neither reservation publishes a partial result.
            if !context.budget.charge(value.clone_work()) {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
            result
                .instruction_values
                .push(EvmInstructionValue::<D::Element>::new(
                    result.executed_pcs.len() - 1,
                    &value,
                ));
            stack.push(value);
            result.mark_instruction(InstructionProgress::Completed);
            continue;
        }
        if (opcode::DUP1..=opcode::DUP16).contains(&op) {
            let depth = usize::from(op - opcode::DUP1 + 1);
            if !context
                .budget
                .charge(stack[stack.len() - depth].clone_work())
            {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
            stack.push(stack[stack.len() - depth].clone());
            result.mark_instruction(InstructionProgress::Completed);
            continue;
        }
        if (opcode::SWAP1..=opcode::SWAP16).contains(&op) {
            let top = stack.len() - 1;
            stack.swap(top, top - usize::from(op - opcode::SWAP1 + 1));
            result.mark_instruction(InstructionProgress::Completed);
            continue;
        }
        let mut args: Vec<EvmValue<D::Element>> = stack.drain(stack.len() - inputs..).collect();
        result.mark_instruction(InstructionProgress::OperandsConsumed);
        args.reverse();
        if config.analysis.relations.enabled
            && args
                .iter()
                .any(EvmValue::<D::Element>::symbolic_limit_reached)
        {
            boundary(
                &mut result,
                pc,
                FrontierReason::Relations(crate::domain::relational::QueryReason::ExpressionLimit),
            );
        }
        // Refinement can make a flagged operand singleton. Its inherited
        // refusal must still be distinguished from this result's optional term.
        let inherited_symbolic_limit = args
            .iter()
            .any(EvmValue::<D::Element>::symbolic_limit_reached);
        for value in &mut args {
            match relations::refine(value, &result.payload.relations, context) {
                Ok(false) => return result,
                Ok(true) => {}
                Err(reason) => {
                    if relations::refinement_failure(&mut result, pc, reason) {
                        return result;
                    }
                }
            }
        }
        if op == opcode::SLOAD && matches!(storage_reads, StorageReadPolicy::Discover) {
            let owner = result.payload.active().key.address;
            // 不用抽象环境的内部占位地址向链上查询。
            let frame = result.payload.active();
            let observed_owner = frame.address_operand.as_ref().map_or_else(
                || frame.key.address_value.as_concrete(),
                |value| value.singleton().map(address),
            );
            if observed_owner == Some(owner) {
                let scan = args[0]
                    .work_size()
                    .saturating_add(result.payload.store.work_size());
                if !context.budget.charge(scan) {
                    boundary(&mut result, pc, FrontierReason::Work);
                    return result;
                }
                let missing = result
                    .payload
                    .store
                    .missing_initial_slots(world, owner, &args[0]);
                if !missing.is_empty() {
                    for slot in missing {
                        boundary(
                            &mut result,
                            pc,
                            FrontierReason::MissingStorage {
                                address: owner,
                                slot,
                            },
                        );
                    }
                    return result;
                }
            }
        }
        if matches!(op, opcode::MLOAD | opcode::CALLDATALOAD) {
            let scan = complete_candidates(&args[0])
                .map_or(1, |s| s.len())
                .saturating_mul(32)
                .saturating_mul(domain.capacity().saturating_add(32));
            if !context.budget.charge(scan) {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
        }
        let work = match operation_work_budgeted(
            &result,
            op,
            &args,
            program,
            domain,
            config,
            entry,
            context.budget,
        ) {
            Ok(work) => work,
            Err(_) => {
                boundary(&mut result, pc, FrontierReason::Work);
                return result;
            }
        };
        if !context.budget.charge(work) {
            boundary(&mut result, pc, FrontierReason::Work);
            return result;
        }
        if result.payload.active().key.is_static
            && matches!(
                op,
                opcode::SSTORE
                    | opcode::TSTORE
                    | opcode::CREATE
                    | opcode::CREATE2
                    | opcode::SELFDESTRUCT
                    | opcode::LOG0..=opcode::LOG4
            )
        {
            result.mark_instruction(InstructionProgress::Dispatched);
            failure(&mut result, context, pc);
            return result;
        }
        match op {
            opcode::JUMP | opcode::JUMPI => {
                let history_work =
                    context_policy.work(result.payload.active().key.jump_history.len());
                if !context.budget.charge(history_work) {
                    boundary(&mut result, pc, FrontierReason::Work);
                    return result;
                }
                result.mark_instruction(InstructionProgress::Dispatched);
                let frame = result.payload.active_mut();
                // Ordinary and relative execution share this immutable policy.
                context_policy.advance(
                    &mut frame.key.jump_history,
                    block.start_pc,
                    config.analysis.context_depth,
                );
                let condition = args.get(1);
                let constrained = if condition.is_none_or(EvmValue::<D::Element>::may_be_nonzero) {
                    if let Some(condition) = condition {
                        relations::branch(&mut result, condition, true, context, pc)
                    } else {
                        Some(result.payload.clone())
                    }
                } else {
                    None
                };
                if let Some(constrained) = constrained {
                    let kind = if op == opcode::JUMP {
                        EdgeKind::Jump
                    } else {
                        EdgeKind::BranchTrue
                    };
                    match complete_candidates(&args[0]) {
                        Some(targets) => {
                            for target in targets.iter() {
                                if !context.budget.charge(1) {
                                    boundary(&mut result, pc, FrontierReason::Work);
                                    break;
                                }
                                let valid = usize::try_from(*target).ok().and_then(|target| {
                                    program.jumpdest_blocks().get(&target).copied()
                                });
                                if let Some(target) = valid {
                                    successor(&mut result, constrained.clone(), target, kind);
                                } else {
                                    result.diagnostics.push((pc, DiagnosticKind::InvalidJump));
                                    failure(&mut result, context, pc);
                                }
                            }
                        }
                        None => {
                            result.diagnostics.push((pc, DiagnosticKind::UnknownJump));
                            failure(&mut result, context, pc);
                            for (target_pc, target) in program.jumpdest_blocks() {
                                if !args[0].contains(U256::from(*target_pc)) {
                                    continue;
                                }
                                if !context.budget.charge(1) {
                                    boundary(&mut result, pc, FrontierReason::Work);
                                    break;
                                }
                                successor(&mut result, constrained.clone(), *target, kind);
                            }
                        }
                    }
                }
                if condition.is_some_and(EvmValue::<D::Element>::may_be_zero) {
                    let Some(payload) = relations::branch(
                        &mut result,
                        condition.expect("JUMPI condition"),
                        false,
                        context,
                        pc,
                    ) else {
                        return result;
                    };
                    if basic_block_index + 1 < program.blocks().len() {
                        successor(
                            &mut result,
                            payload,
                            basic_block_index + 1,
                            EdgeKind::BranchFalse,
                        );
                    } else {
                        finish(
                            &mut result,
                            payload,
                            OutcomeKind::Return,
                            EvmByteArray::<D::Element>::empty(),
                            context,
                            pc,
                        );
                    }
                }
                return result;
            }
            opcode::CALL | opcode::CALLCODE | opcode::DELEGATECALL | opcode::STATICCALL => {
                result.mark_instruction(InstructionProgress::Dispatched);
                result.payload.active_mut().returndata = EvmByteArray::<D::Element>::empty();
                call(&mut result, world, op, &args, program, context, pc);
                return result;
            }
            opcode::CREATE | opcode::CREATE2 => {
                result.mark_instruction(InstructionProgress::Dispatched);
                result.payload.active_mut().returndata = EvmByteArray::<D::Element>::empty();
                create::create(&mut result, world, op, &args, program, context, pc);
                return result;
            }
            opcode::STOP => {
                result.mark_instruction(InstructionProgress::Dispatched);
                let payload = result.payload.clone();
                finish(
                    &mut result,
                    payload,
                    OutcomeKind::Return,
                    EvmByteArray::<D::Element>::empty(),
                    context,
                    pc,
                );
                return result;
            }
            opcode::RETURN | opcode::REVERT => {
                if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                    return result;
                }
                let data = match result.payload.active().memory.slice_with(
                    &args[0],
                    &args[1],
                    config.max_memory_bytes,
                    domain,
                ) {
                    Ok(data) => data,
                    Err(error) => {
                        boundary_bytes(&mut result, pc, error);
                        return result;
                    }
                };
                result.mark_instruction(InstructionProgress::Dispatched);
                let payload = result.payload.clone();
                finish(
                    &mut result,
                    payload,
                    if op == opcode::RETURN {
                        OutcomeKind::Return
                    } else {
                        OutcomeKind::Revert
                    },
                    data,
                    context,
                    pc,
                );
                return result;
            }
            opcode::SELFDESTRUCT => {
                if result
                    .payload
                    .active()
                    .key
                    .address_value
                    .as_concrete()
                    .is_none()
                {
                    // A literal beneficiary can alias an unknown logical owner;
                    // the internal store namespace is not an address observation.
                    boundary(&mut result, pc, FrontierReason::UnknownTarget);
                    return result;
                }
                let Some(beneficiaries) = complete_candidates(&args[0]) else {
                    boundary(&mut result, pc, FrontierReason::UnknownTarget);
                    return result;
                };
                let owner = result.payload.active().key.address;
                result.mark_instruction(InstructionProgress::Dispatched);
                for beneficiary in beneficiaries.iter() {
                    if !context
                        .budget
                        .charge(result.payload.work_size().saturating_add(1))
                    {
                        boundary(&mut result, pc, FrontierReason::Work);
                        break;
                    }
                    let mut payload = result.payload.clone();
                    payload
                        .store
                        .selfdestruct_with(owner, address(*beneficiary), domain);
                    if !late_failure::after_selfdestruct(&mut result, &payload, context, pc) {
                        break;
                    }
                    finish(
                        &mut result,
                        payload,
                        OutcomeKind::Return,
                        EvmByteArray::<D::Element>::empty(),
                        context,
                        pc,
                    );
                }
                return result;
            }
            opcode::SSTORE | opcode::TSTORE => {
                let state_address = result.payload.active().key.address;
                let write = if op == opcode::SSTORE {
                    result.payload.store.write_checked_with(
                        state_address,
                        &args[0],
                        &args[1],
                        domain,
                    )
                } else {
                    result.payload.store.write_transient_checked_with(
                        state_address,
                        &args[0],
                        &args[1],
                        domain,
                    )
                };
                if let Err(error) = write {
                    boundary_store(&mut result, pc, error);
                    return result;
                }
            }
            opcode::MSTORE | opcode::MSTORE8 => {
                let size = constant(if op == opcode::MSTORE { 32 } else { 1 });
                if !touch_memory(&mut result, &args[0], &size, context, pc) {
                    return result;
                }
                let memory = &mut result.payload.active_mut().memory;
                let write = if op == opcode::MSTORE {
                    memory.write_word_budgeted_with(
                        &args[0],
                        &args[1],
                        config.max_memory_bytes,
                        domain,
                        context.budget,
                    )
                } else {
                    memory.write_byte_budgeted_with(
                        &args[0],
                        &args[1],
                        config.max_memory_bytes,
                        domain,
                        context.budget,
                    )
                };
                match write {
                    Err(error) => {
                        boundary_bytes(&mut result, pc, error);
                        return result;
                    }
                    Ok(false) => {
                        boundary(&mut result, pc, FrontierReason::Work);
                        return result;
                    }
                    Ok(true) => {}
                }
                if memory.symbolic_limit_reached() {
                    boundary(
                        &mut result,
                        pc,
                        FrontierReason::Relations(
                            crate::domain::relational::QueryReason::ExpressionLimit,
                        ),
                    );
                }
            }
            opcode::CALLDATACOPY | opcode::CODECOPY | opcode::RETURNDATACOPY | opcode::MCOPY => {
                let source = match op {
                    opcode::CALLDATACOPY => result.payload.active().calldata.clone(),
                    opcode::CODECOPY => EvmByteArray::<D::Element>::exact(program.bytes()),
                    opcode::RETURNDATACOPY => result.payload.active().returndata.clone(),
                    _ => result.payload.active().memory.clone(),
                };
                if op == opcode::RETURNDATACOPY
                    && !ranges::returndata(
                        &mut result,
                        &args[1],
                        &args[2],
                        source.len(),
                        context,
                        pc,
                    )
                {
                    result.mark_instruction(InstructionProgress::Dispatched);
                    return result;
                }
                let admitted = if op == opcode::MCOPY {
                    ranges::memory_copy(&mut result, &args[1], &args[0], &args[2], context, pc)
                        && expand_memory(&mut result, &args[1], &args[2], context, pc)
                        && expand_memory(&mut result, &args[0], &args[2], context, pc)
                } else {
                    touch_memory(&mut result, &args[0], &args[2], context, pc)
                };
                if !admitted {
                    return result;
                }
                if let Err(error) = result.payload.active_mut().memory.copy_from_with(
                    &args[0],
                    &source,
                    &args[1],
                    &args[2],
                    config.max_memory_bytes,
                    domain,
                ) {
                    boundary_bytes(&mut result, pc, error);
                    return result;
                }
            }
            opcode::EXTCODECOPY => {
                if !touch_memory(&mut result, &args[1], &args[3], context, pc) {
                    return result;
                }
                let symbolic_owner = entry.environment.to.as_concrete().is_none();
                let exact_self = symbolic_owner
                    && args[0].identity().same_identity(
                        entry
                            .environment
                            .address_value(entry.environment.to)
                            .identity(),
                    );
                let source = if exact_self {
                    code_bytes(&result.payload.store, entry.address)
                        .map(|bytes| EvmByteArray::<D::Element>::exact(&bytes))
                        .unwrap_or_else(EvmByteArray::<D::Element>::unknown)
                } else {
                    match complete_candidates(&args[0]) {
                        Some(targets) => {
                            let mut sources = targets.iter().map(|target| {
                                let target = address(*target);
                                if symbolic_owner && target == entry.address {
                                    EvmByteArray::<D::Element>::unknown()
                                } else {
                                    code_bytes(&result.payload.store, target)
                                        .map(|bytes| EvmByteArray::<D::Element>::exact(&bytes))
                                        .unwrap_or_else(EvmByteArray::<D::Element>::unknown)
                                }
                            });
                            let first = sources.next().expect("nonempty value");
                            let joined = sources.try_fold(first, |left, right| {
                                let selector = crate::domain::symbolic::ExprId::fresh().ok_or(
                                    crate::world::RangeError::State(
                                        crate::domain::symbolic::StateError::IdentityExhausted,
                                    ),
                                )?;
                                left.join_guarded_with(
                                    &right,
                                    domain,
                                    selector,
                                    config.analysis.relations.expressions(),
                                )
                            });
                            match joined {
                                Ok(source) => source,
                                Err(error) => {
                                    boundary_bytes(&mut result, pc, error);
                                    return result;
                                }
                            }
                        }
                        None => EvmByteArray::<D::Element>::unknown(),
                    }
                };
                if let Err(error) = result.payload.active_mut().memory.copy_from_with(
                    &args[1],
                    &source,
                    &args[2],
                    &args[3],
                    config.max_memory_bytes,
                    domain,
                ) {
                    boundary_bytes(&mut result, pc, error);
                    return result;
                }
            }
            opcode::LOG0..=opcode::LOG4 => {
                if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                    return result;
                }
                let data = match result.payload.active().memory.slice_with(
                    &args[0],
                    &args[1],
                    config.max_memory_bytes,
                    domain,
                ) {
                    Ok(data) => data,
                    Err(error) => {
                        boundary_bytes(&mut result, pc, error);
                        return result;
                    }
                };
                let frame = result.payload.active();
                let source = LogKey {
                    address: frame.key.address,
                    code_address: frame.key.code_address,
                    code_hash: frame.key.code_hash,
                    source_kind: if frame.key.mode == FrameCode::InitCode {
                        LogSourceKind::Init
                    } else {
                        LogSourceKind::Runtime
                    },
                    pc,
                };
                let topic_copy_work = args[2..].iter().fold(0usize, |work, value| {
                    work.saturating_add(value.clone_work())
                });
                if !context.budget.charge(topic_copy_work) {
                    boundary(&mut result, pc, FrontierReason::Work);
                    return result;
                }
                if result
                    .payload
                    .store
                    .emit_log_with(
                        source,
                        EvmLog {
                            topics: args[2..].to_vec(),
                            data,
                        },
                        domain,
                    )
                    .is_err()
                {
                    boundary(&mut result, pc, FrontierReason::UnsupportedOpcode(op));
                    return result;
                }
            }
            _ if outputs > 0 => {
                let frame = result.payload.active();
                let copied = match op {
                    opcode::ADDRESS => frame.address_operand.as_ref(),
                    opcode::CALLER => frame.caller_operand.as_ref(),
                    opcode::CALLVALUE => Some(&frame.call_value),
                    opcode::GASPRICE => Some(&entry.environment.gas_price),
                    opcode::TIMESTAMP => Some(&entry.environment.timestamp),
                    opcode::NUMBER => Some(&entry.environment.number),
                    opcode::DIFFICULTY => Some(&entry.environment.prevrandao),
                    opcode::GASLIMIT => Some(&entry.environment.gas_limit),
                    opcode::CHAINID => entry.environment.chain_id.as_ref(),
                    opcode::BASEFEE => Some(&entry.environment.base_fee),
                    opcode::BLOBBASEFEE => Some(&entry.environment.blob_base_fee),
                    opcode::CALLDATASIZE => Some(frame.calldata.len()),
                    opcode::RETURNDATASIZE => Some(frame.returndata.len()),
                    opcode::MSIZE => Some(frame.memory.len()),
                    _ => None,
                };
                if copied.is_some_and(|value| !context.budget.charge(value.clone_work())) {
                    boundary(&mut result, pc, FrontierReason::Work);
                    return result;
                }
                let mut value = match op {
                    opcode::ADDRESS => frame.address_operand.clone().unwrap_or_else(|| {
                        entry.environment.address_value(frame.key.address_value)
                    }),
                    opcode::CALLER => frame
                        .caller_operand
                        .clone()
                        .unwrap_or_else(|| entry.environment.address_value(frame.key.caller)),
                    opcode::ORIGIN => entry
                        .environment
                        .address_value(entry.environment.resolved_origin()),
                    opcode::CALLVALUE => frame.call_value.clone(),
                    opcode::GASPRICE => entry
                        .environment
                        .gas_price
                        .clone()
                        .with_symbol(Symbol::GasPrice, entry.environment.input_scope.id()),
                    opcode::COINBASE => entry.environment.address_value(entry.environment.coinbase),
                    opcode::TIMESTAMP => entry
                        .environment
                        .timestamp
                        .clone()
                        .with_symbol(Symbol::Timestamp, entry.environment.input_scope.id()),
                    opcode::NUMBER => entry
                        .environment
                        .number
                        .clone()
                        .with_symbol(Symbol::Number, entry.environment.input_scope.id()),
                    opcode::DIFFICULTY => entry
                        .environment
                        .prevrandao
                        .clone()
                        .with_symbol(Symbol::Prevrandao, entry.environment.input_scope.id()),
                    opcode::GASLIMIT => entry
                        .environment
                        .gas_limit
                        .clone()
                        .with_symbol(Symbol::GasLimit, entry.environment.input_scope.id()),
                    opcode::CHAINID => entry.environment.chain_id(world.identity()),
                    opcode::BASEFEE => entry
                        .environment
                        .base_fee
                        .clone()
                        .with_symbol(Symbol::BaseFee, entry.environment.input_scope.id()),
                    opcode::BLOBBASEFEE => entry
                        .environment
                        .blob_base_fee
                        .clone()
                        .with_symbol(Symbol::BlobBaseFee, entry.environment.input_scope.id()),
                    opcode::BLOCKHASH => entry.environment.block_hash_with(&args[0], domain),
                    opcode::BLOBHASH => entry.environment.blob_hashes.get_with(
                        &args[0],
                        domain,
                        entry.environment.input_scope.id(),
                    ),
                    opcode::GAS => entry.environment.gas.value_with(),
                    opcode::PC => constant(pc),
                    opcode::CODESIZE => constant(program.byte_len()),
                    opcode::CALLDATASIZE => frame.calldata.len().clone(),
                    opcode::RETURNDATASIZE => frame.returndata.len().clone(),
                    opcode::MSIZE => frame.memory.len().clone(),
                    opcode::CALLDATALOAD => {
                        let plan = match frame.calldata.prepare_word_read_with(
                            &args[0],
                            domain,
                            context.budget,
                        ) {
                            Ok(Some(plan)) => plan,
                            Ok(None) => {
                                boundary(&mut result, pc, FrontierReason::Work);
                                return result;
                            }
                            Err(error) => {
                                boundary_bytes(&mut result, pc, error);
                                return result;
                            }
                        };
                        match plan.read_word_budgeted(context.budget) {
                            Ok(Some(value)) => value,
                            Ok(None) => {
                                boundary(&mut result, pc, FrontierReason::Work);
                                return result;
                            }
                            Err(error) => {
                                boundary_bytes(&mut result, pc, error);
                                return result;
                            }
                        }
                    }
                    opcode::SLOAD | opcode::TLOAD => {
                        let read = if op == opcode::SLOAD {
                            result.payload.store.read_checked_with(
                                frame.key.address,
                                &args[0],
                                domain,
                            )
                        } else {
                            result.payload.store.read_transient_checked_with(
                                frame.key.address,
                                &args[0],
                                domain,
                            )
                        };
                        match read {
                            Ok(value) => value,
                            Err(error) => {
                                boundary_store(&mut result, pc, error);
                                return result;
                            }
                        }
                    }
                    opcode::SELFBALANCE => result.payload.store.read_balance(frame.key.address),
                    opcode::BALANCE => environment_targets(&args[0], domain, entry, |address| {
                        result.payload.store.read_balance(address)
                    }),
                    opcode::EXTCODESIZE => environment_targets(&args[0], domain, entry, |target| {
                        match result.payload.store.code(target) {
                            Some(Code::Runtime(program)) => constant(program.byte_len()),
                            Some(Code::Delegation(_)) => constant(23),
                            Some(Code::Empty) => zero(),
                            _ => EvmValue::<D::Element>::top(),
                        }
                    }),
                    opcode::EXTCODEHASH => environment_targets(&args[0], domain, entry, |target| {
                        code_bytes(&result.payload.store, target)
                            .map(|bytes| {
                                let hash = EvmValue::<D::Element>::constant(U256::from_be_slice(
                                    keccak256(&bytes).as_slice(),
                                ));
                                if !bytes.is_empty() {
                                    return hash;
                                }
                                let balance = result.payload.store.read_balance(target);
                                let nonce = result.payload.store.nonce(target);
                                let may_be_empty = balance.may_be_zero() && nonce.may_be_zero();
                                let may_exist = balance.may_be_nonzero() || nonce.may_be_nonzero();
                                if may_be_empty && may_exist {
                                    domain.join(&hash, &zero())
                                } else if may_be_empty {
                                    zero()
                                } else {
                                    hash
                                }
                            })
                            .unwrap_or_else(EvmValue::<D::Element>::top)
                    }),
                    opcode::MLOAD => {
                        let plan = match frame.memory.prepare_word_read_with(
                            &args[0],
                            domain,
                            context.budget,
                        ) {
                            Ok(Some(plan)) => plan,
                            Ok(None) => {
                                boundary(&mut result, pc, FrontierReason::Work);
                                return result;
                            }
                            Err(error) => {
                                boundary_bytes(&mut result, pc, error);
                                return result;
                            }
                        };
                        if !touch_memory(&mut result, &args[0], &constant(32), context, pc) {
                            return result;
                        }
                        match plan.read_word_budgeted(context.budget) {
                            Ok(Some(value)) => value,
                            Ok(None) => {
                                boundary(&mut result, pc, FrontierReason::Work);
                                return result;
                            }
                            Err(error) => {
                                boundary_bytes(&mut result, pc, error);
                                return result;
                            }
                        }
                    }
                    opcode::KECCAK256 => {
                        if !touch_memory(&mut result, &args[0], &args[1], context, pc) {
                            return result;
                        }
                        match result.payload.active().memory.slice_with(
                            &args[0],
                            &args[1],
                            config.max_memory_bytes,
                            domain,
                        ) {
                            Ok(bytes) => match hash::evaluate(&bytes, &mut result, context, pc) {
                                Some(value) => value,
                                None => return result,
                            },
                            Err(error) => {
                                boundary_bytes(&mut result, pc, error);
                                return result;
                            }
                        }
                    }
                    _ => {
                        if !matches!(op, 0x01..=0x0b | 0x10..=0x1e) {
                            result.diagnostics.push((pc, DiagnosticKind::OpaqueResult));
                        }
                        let mut reduction = domain.apply_detailed(op, &args);
                        if matches!(
                            reduction.symbolic_error,
                            Some(crate::domain::symbolic::ExprError::Limit { .. })
                        ) {
                            if matches!(op, 0x01..=0x0b | 0x10..=0x1e) && !inherited_symbolic_limit
                            {
                                // Numeric transfer succeeded; only the current
                                // optional correlation failed to fit. Ordinary
                                // identify below gives this result its own leaf.
                                reduction.value.forget_expression_after_limit();
                                result.diagnostics.push((
                                    pc,
                                    DiagnosticKind::RelationalPrecisionLost(
                                        crate::domain::relational::QueryReason::ExpressionLimit,
                                    ),
                                ));
                            } else {
                                boundary(
                                    &mut result,
                                    pc,
                                    FrontierReason::Relations(
                                        crate::domain::relational::QueryReason::ExpressionLimit,
                                    ),
                                );
                            }
                        }
                        if matches!(
                            reduction.status,
                            crate::domain::ReductionStatus::RoundLimit
                                | crate::domain::ReductionStatus::FactLimit
                                | crate::domain::ReductionStatus::SymbolicLimit
                                | crate::domain::ReductionStatus::WorkLimit
                                | crate::domain::ReductionStatus::Cancelled
                                | crate::domain::ReductionStatus::KernelFailure
                        ) {
                            result
                                .diagnostics
                                .push((pc, DiagnosticKind::FactExchangeLimited(reduction.status)));
                        }
                        reduction.value
                    }
                };
                value = domain.project(&value);
                if value.symbolic_limit_reached() {
                    boundary(
                        &mut result,
                        pc,
                        FrontierReason::Relations(
                            crate::domain::relational::QueryReason::ExpressionLimit,
                        ),
                    );
                }
                value = relations::identify(value, config.analysis.relations.enabled);
                match relations::refine(&mut value, &result.payload.relations, context) {
                    Ok(false) => return result,
                    Ok(true) => {}
                    Err(reason) => {
                        if relations::refinement_failure(&mut result, pc, reason) {
                            return result;
                        }
                    }
                }
                {
                    let origin = match op {
                        opcode::ADDRESS | opcode::CALLER | opcode::ORIGIN => Some(Origin::Address),
                        opcode::CALLDATALOAD | opcode::CALLDATASIZE => Some(Origin::Calldata),
                        opcode::MLOAD | opcode::MSIZE => Some(Origin::Memory),
                        opcode::SLOAD => Some(Origin::Storage),
                        opcode::TLOAD => Some(Origin::TransientStorage),
                        opcode::CALLVALUE => Some(Origin::CallValue),
                        opcode::BALANCE | opcode::SELFBALANCE => Some(Origin::Balance),
                        opcode::RETURNDATASIZE => Some(Origin::Returndata),
                        0x01..=0x0b | 0x10..=0x1e => None,
                        _ => Some(Origin::Environment),
                    };
                    if let Some(origin) = origin {
                        value.set_origin(origin);
                    }
                    if let Some(scope) = scope {
                        value = value.with_identity(scope, definition);
                    }
                    if let Some(next) = definition.checked_add(1) {
                        definition = next;
                    } else {
                        value.forget_identity();
                    }
                }
                // One numeric copy belongs to the receipt; repeat_n may make
                // outputs - 1 further copies before moving the final value.
                if !context
                    .budget
                    .charge(value.clone_work().saturating_mul(outputs))
                {
                    boundary(&mut result, pc, FrontierReason::Work);
                    return result;
                }
                result
                    .instruction_values
                    .push(EvmInstructionValue::<D::Element>::new(
                        result.executed_pcs.len() - 1,
                        &value,
                    ));
                result
                    .payload
                    .active_mut()
                    .stack
                    .extend(std::iter::repeat_n(value, outputs));
            }
            _ => {}
        }
        result.mark_instruction(InstructionProgress::Completed);
    }
    if basic_block_index + 1 < program.blocks().len() {
        let payload = result.payload.clone();
        successor(
            &mut result,
            payload,
            basic_block_index + 1,
            EdgeKind::Fallthrough,
        );
    } else {
        let payload = result.payload.clone();
        finish(
            &mut result,
            payload,
            OutcomeKind::Return,
            EvmByteArray::<D::Element>::empty(),
            context,
            program.byte_len(),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::analysis::transfer::ExecutionFrontier;
    use crate::domain::ValueAlgebra;
    use crate::world::Entry;
    use crate::world::World;

    use super::*;
    use crate::{Fork, world::Account};

    #[test]
    fn cancelled_transfer_preserves_input_and_differs_from_exhausted_work() {
        let owner = Address::from([0x11; 20]);
        let mut world = World::new(Fork::Osaka, "transfer interruption");
        world
            .insert(owner, Account::from_hex("60015f5500", Fork::Osaka).unwrap())
            .unwrap();
        let entry = Entry::new(owner);
        let config = ExecutionConfig::default();
        let domain = config.domain().unwrap();
        let input = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        for cancelled in [false, true] {
            let control = crate::analysis::control::Control::new(Default::default());
            let (result, budget) = control.scope(|| {
                let mut budget = WorkBudget::new(if cancelled { config.max_work } else { 0 });
                if cancelled {
                    control.cancellation().cancel();
                }
                let result = execute(
                    &world,
                    &entry,
                    input.clone(),
                    &config,
                    ValueAlgebra::new(&domain),
                    &crate::analysis::RecentDistinctJumps,
                    &mut budget,
                    StorageReadPolicy::Abstract,
                );
                (result, budget)
            });
            assert!(budget.exhausted());
            assert_eq!(budget.used(), 0);
            assert_eq!(result.payload, input);
            assert!(result.executed_pcs.is_empty());
            assert!(result.instruction_progress.is_empty());
            assert!(result.instruction_values.is_empty());
            assert!(result.completed_calls.is_empty());
            assert!(result.diagnostics.is_empty());
            assert!(result.successors.is_empty());
            assert!(result.outcomes.is_empty());
            assert_eq!(
                result.frontiers,
                vec![ExecutionFrontier {
                    pc: 0,
                    reason: if cancelled {
                        FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
                    } else {
                        FrontierReason::Work
                    },
                    target: None,
                    range: None,
                    payload: None,
                }]
            );
        }
    }

    #[test]
    fn delegation_without_raw_byte_evidence_explains_the_active_source() {
        let owner = Address::from([0x11; 20]);
        let missing_code = Address::from([0x22; 20]);
        let mut world = World::new(Fork::Osaka, "missing delegation byte evidence");
        world.insert(owner, Account::empty()).unwrap();
        let entry = Entry::new(owner);
        let config = ExecutionConfig::default();
        let domain = config.domain().unwrap();
        let mut payload = initial(&world, &entry, ValueAlgebra::new(&domain)).unwrap();
        // Exercise the defensive evidence boundary directly: ordinary validated
        // delegation facts normally provide the marker bytes at resolution.
        let frame = payload.active_mut();
        frame.code = FrameCode::InvalidDelegation;
        frame.key.mode = FrameCode::InvalidDelegation;
        frame.key.code_address = missing_code;
        let result = execute(
            &world,
            &entry,
            payload,
            &config,
            ValueAlgebra::new(&domain),
            &crate::analysis::RecentDistinctJumps,
            &mut WorkBudget::new(config.max_work),
            StorageReadPolicy::Abstract,
        );
        assert_eq!(
            result.diagnostics,
            vec![(
                0,
                DiagnosticKind::InvalidOpcode(InvalidOpcodeDiagnostic::DelegationEntry {
                    opcode: None,
                    detail: "nested delegation indicator cannot be executed".to_owned(),
                    byte_unavailable: Some(
                        "active code account's raw bytes are unavailable".to_owned()
                    ),
                },)
            )]
        );
        assert!(result.executed_pcs.is_empty());
        assert!(result.instruction_progress.is_empty());
        assert!(result.instruction_values.is_empty());
        assert_eq!(result.outcomes[0].kind, OutcomeKind::Failure);
    }
}

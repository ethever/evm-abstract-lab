//! Independent local fixed point. Identity allocation is outside transfer loops.
//! Stack-height contexts are separate blocks; facts never cross incompatible
//! frames. A memory fact survives a merge only when all incoming snapshots agree.

use crate::{
    Block, Completion, DiagnosticCode, Edge, EdgeKind, EntityId, ExitKind, FactScope, Ir,
    Operation, Phase, Progress, Severity, SourceSpan, StageKind, Terminator, U256, Value,
    ValueFact, ValueKind, artifact::Builder, effects::effects,
};
use analysis_kernel::Worklist;
use evm_bytecode::Program;
use evm_domain::{DomainSpec, EvmNumericAlgebra, FiniteConstantSet, FiniteSetAlgebra};
use std::{collections::BTreeMap, num::NonZeroUsize};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    value: EntityId,
    fact: ValueFact,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Memory {
    zero: bool,
    // Only full exact-offset words; aliasing writes invalidate overlapping cells.
    words: BTreeMap<u64, Cell>,
}
impl Memory {
    fn clear(&mut self) {
        self.zero = false;
        self.words.clear();
    }
    fn write(&mut self, offset: Option<u64>, width: u64, value: Option<Cell>) {
        if width == 0 {
            return;
        }
        self.zero = false;
        let Some(offset) = offset.filter(|o| o.checked_add(width).is_some()) else {
            self.words.clear();
            return;
        };
        self.words.retain(|p, _| {
            p.checked_add(32)
                .is_some_and(|end| end <= offset || offset + width <= *p)
        });
        if let Some(value) = value {
            self.words.insert(offset, value);
        }
    }
    fn merge(&mut self, other: &Self) -> bool {
        let before = self.clone();
        self.zero &= other.zero;
        self.words
            .retain(|offset, cell| other.words.get(offset) == Some(cell));
        *self != before
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    stack: Vec<Cell>,
    memory: Memory,
}

pub(crate) fn run(owner: &mut Builder<impl FnMut(Progress)>) -> bool {
    if !owner.pay(1) {
        return false;
    }
    if owner.artifact.input.runtime_bytecode.len() > owner.limits().max_bytecode_bytes {
        owner.stop(
            Completion::Limited,
            DiagnosticCode::Limit,
            "bytecode length exceeds max_bytecode_bytes",
        );
        return false;
    }
    if owner.limits().max_constant_set == 0 {
        owner.stop(
            Completion::Failed,
            DiagnosticCode::Limit,
            "max_constant_set must be nonzero",
        );
        return false;
    }
    if owner.limits().max_structuring_depth > crate::MAX_STRUCTURING_DEPTH {
        owner.stop(
            Completion::Failed,
            DiagnosticCode::Limit,
            "max_structuring_depth exceeds the supported recursion ceiling (512)",
        );
        return false;
    }
    // Decoding is linear in the supplied bytes, including unreachable tails.
    if !owner.pay(owner.artifact.input.runtime_bytecode.len()) {
        return false;
    }
    let program = match Program::decode_with_fork(
        &owner.artifact.input.runtime_bytecode,
        owner.artifact.input.fork,
    ) {
        Ok(p) => p,
        Err(e) => {
            owner.stop(Completion::Failed, DiagnosticCode::Decode, &e.to_string());
            return false;
        }
    };
    let count: usize = program.blocks().iter().map(|b| b.instructions.len()).sum();
    let value_count = program.blocks().len().saturating_mul(2)
        + program
            .blocks()
            .iter()
            .flat_map(|b| &b.instructions)
            .map(|i| {
                let outputs = if (0x80..=0x9f).contains(&i.opcode) {
                    0
                } else {
                    i.stack_io().1
                };
                outputs
                    + usize::from(effects(i.opcode).memory_write)
                    + usize::from(!effects(i.opcode).is_pure())
            })
            .sum::<usize>();
    if program.blocks().len() > owner.limits().max_blocks
        || count > owner.limits().max_operations
        || value_count > owner.limits().max_values
    {
        owner.stop(
            Completion::Limited,
            DiagnosticCode::Limit,
            "decoded block, operation or value limit exceeded",
        );
        return false;
    }
    if !owner.pay(count.saturating_mul(8).saturating_add(value_count)) {
        return false;
    }
    let source = source_ir(owner, &program);
    if !owner.commit(
        StageKind::Decoded,
        source.clone(),
        ("decode", "fork-aware original bytecode decoding"),
        BTreeMap::new(),
        Vec::new(),
        Vec::new(),
    ) {
        return false;
    }
    owner.notify_phase(Phase::Recovery);
    let cap = NonZeroUsize::new(owner.limits().max_constant_set).expect("checked nonzero");
    let algebra =
        FiniteSetAlgebra::from_spec(DomainSpec::new(cap, NonZeroUsize::MIN, NonZeroUsize::MIN));
    let blocks = source.blocks.len();
    let mut recovery = Recovery {
        owner,
        program: &program,
        templates: source.clone(),
        ir: source,
        states: vec![None; blocks],
        origins: (0..blocks).collect(),
        contexts: BTreeMap::new(),
        queue: Worklist::default(),
        edges: BTreeMap::new(),
        algebra,
        parents: BTreeMap::new(),
        copies: Vec::new(),
        value_index: BTreeMap::new(),
        operation_count: count,
    };
    recovery.reindex();
    if blocks > 0 {
        recovery.states[0] = Some(State {
            stack: Vec::new(),
            memory: Memory {
                zero: true,
                words: BTreeMap::new(),
            },
        });
        recovery.contexts.insert((0, 0), 0);
        if !recovery.enqueue(0) {
            return false;
        }
    }
    while !recovery.queue.is_empty() {
        if recovery.owner.artifact.statistics.evaluations >= recovery.owner.limits().max_iterations
        {
            recovery.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "block transfer iteration limit exhausted",
            );
            break;
        }
        // The shared kernel owns the FIFO selection/removal quote. Each wake
        // separately reserves enqueue_work, including geometric queue growth.
        let index = match recovery.queue.pop_eligible(recovery.owner, |_| true) {
            Ok(Some(index)) => index,
            Ok(None) => break,
            Err(error) => {
                if !recovery.owner.stopped {
                    let (completion, code) = if recovery.owner.cancel.is_cancelled() {
                        (Completion::Cancelled, DiagnosticCode::Cancelled)
                    } else {
                        (Completion::Limited, DiagnosticCode::Limit)
                    };
                    recovery.owner.stop(
                        completion,
                        code,
                        &format!("recovery queue selection refused: {error:?}"),
                    );
                }
                break;
            }
        };
        recovery.owner.artifact.statistics.evaluations += 1;
        if !recovery.transfer(index) {
            break;
        }
    }
    if recovery.owner.stopped {
        return false;
    }
    if !recovery
        .owner
        .pay(crate::transform::ir_size(&recovery.ir).saturating_mul(4))
    {
        return false;
    }
    let reached: std::collections::BTreeSet<_> = recovery
        .states
        .iter()
        .enumerate()
        .filter(|(_, s)| s.is_some())
        .map(|(i, _)| recovery.ir.blocks[i].id)
        .collect();
    recovery.ir.blocks.retain(|b| reached.contains(&b.id));
    let operations: std::collections::BTreeSet<_> = recovery
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .map(|o| o.id)
        .collect();
    let word_outputs: std::collections::BTreeSet<_> = recovery
        .ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .flat_map(|o| o.outputs.iter().copied())
        .collect();
    recovery.ir.values.retain(|v| match v.kind {
        ValueKind::Operation { operation, .. }
        | ValueKind::Effect {
            operation: Some(operation),
        }
        | ValueKind::Memory {
            operation: Some(operation),
        } => operations.contains(&operation),
        ValueKind::Parameter { block, .. }
        | ValueKind::MemoryParameter { block }
        | ValueKind::EffectParameter { block } => reached.contains(&block),
        _ => word_outputs.contains(&v.id),
    });
    for block in &recovery.ir.blocks {
        if block.terminator.is_partial() {
            recovery.owner.partial();
            recovery.owner.diagnostic(
                DiagnosticCode::UnknownJump,
                Severity::Warning,
                Some(block.id),
                block.source,
                if matches!(block.terminator, Terminator::Pending) {
                    "block remains an explicit recovery frontier"
                } else {
                    "indirect jump retains an unresolved target exit"
                }
                .to_owned(),
            );
        }
    }
    recovery.ir.fact_scope =
        if recovery.ir.blocks.iter().any(|b| b.terminator.is_partial()) || recovery.owner.stopped {
            FactScope::RecoveredPaths
        } else {
            FactScope::CompleteLocalCfg
        };
    let adopted = recovery.owner.commit(
        StageKind::Baseline,
        recovery.ir,
        (
            "local_recovery",
            "bounded stack/value/memory fixed point with explicit phi assignments",
        ),
        recovery.parents,
        recovery.copies,
        Vec::new(),
    );
    adopted && !recovery.owner.stopped
}

fn source_ir(owner: &mut Builder<impl FnMut(Progress)>, program: &Program) -> Ir {
    let mut ir = Ir {
        bytecode_len: program.byte_len(),
        ..Ir::default()
    };
    for basic in program.blocks() {
        let id = owner.id();
        let memory_parameter = owner.id();
        let effect_parameter = owner.id();
        ir.values.push(Value {
            id: effect_parameter,
            kind: ValueKind::EffectParameter { block: id },
            fact: ValueFact::Unknown,
        });
        ir.values.push(Value {
            id: memory_parameter,
            kind: ValueKind::MemoryParameter { block: id },
            fact: ValueFact::Unknown,
        });
        let mut operations = Vec::new();
        for instruction in &basic.instructions {
            let op_id = owner.id();
            let fx = effects(instruction.opcode);
            let mut outputs = Vec::new();
            if !(0x80..=0x9f).contains(&instruction.opcode) && instruction.is_valid() {
                for index in 0..instruction.stack_io().1 {
                    let value = owner.id();
                    outputs.push(value);
                    ir.values.push(Value {
                        id: value,
                        kind: instruction.immediate.map(ValueKind::Constant).unwrap_or(
                            ValueKind::Operation {
                                operation: op_id,
                                index,
                            },
                        ),
                        fact: ValueFact::Bottom,
                    });
                }
            }
            let effect_output = (!fx.is_pure()).then(|| owner.id());
            if let Some(id) = effect_output {
                ir.values.push(Value {
                    id,
                    kind: ValueKind::Effect {
                        operation: Some(op_id),
                    },
                    fact: ValueFact::Unknown,
                });
            }
            let memory_output = fx.memory_write.then(|| owner.id());
            if let Some(value) = memory_output {
                ir.values.push(Value {
                    id: value,
                    kind: ValueKind::Memory {
                        operation: Some(op_id),
                    },
                    fact: ValueFact::Unknown,
                });
            }
            operations.push(Operation {
                id: op_id,
                source: Some(SourceSpan {
                    pc: instruction.pc,
                    end_pc: instruction
                        .pc
                        .saturating_add(instruction.size)
                        .min(program.byte_len()),
                }),
                opcode: instruction.opcode,
                name: instruction.name().to_owned(),
                inputs: Vec::new(),
                outputs,
                effects: fx,
                memory_input: None,
                memory_output,
                replacement: None,
                effect_input: None,
                effect_output,
            });
        }
        ir.blocks.push(Block {
            id,
            source: Some(SourceSpan {
                pc: basic.start_pc,
                end_pc: basic
                    .instructions
                    .last()
                    .map(|i| i.pc.saturating_add(i.size).min(program.byte_len()))
                    .unwrap_or(basic.start_pc),
            }),
            parameters: Vec::new(),
            memory_parameter,
            effect_parameter,
            operations,
            terminator: Terminator::Pending,
        });
    }
    ir.entry = ir.blocks.first().map(|b| b.id);
    ir
}

struct Recovery<'a, P: FnMut(Progress)> {
    owner: &'a mut Builder<P>,
    program: &'a Program,
    templates: Ir,
    ir: Ir,
    states: Vec<Option<State>>,
    origins: Vec<usize>,
    contexts: BTreeMap<(usize, usize), usize>,
    queue: Worklist<usize>,
    edges: BTreeMap<(usize, usize, EdgeKind), EntityId>,
    algebra: FiniteSetAlgebra,
    value_index: BTreeMap<EntityId, usize>,
    parents: BTreeMap<EntityId, Vec<EntityId>>,
    copies: Vec<(EntityId, EntityId)>,
    // Includes source templates and real stack-height context copies.
    operation_count: usize,
}
impl<P: FnMut(Progress)> Recovery<'_, P> {
    fn enqueue(&mut self, index: usize) -> bool {
        if !self.owner.pay(self.queue.enqueue_work()) {
            return false;
        }
        self.queue.enqueue(index);
        true
    }
    fn reindex(&mut self) {
        self.value_index = self
            .ir
            .values
            .iter()
            .enumerate()
            .map(|(i, v)| (v.id, i))
            .collect();
    }
    fn add_value(&mut self, value: Value) -> bool {
        if self.ir.values.len() >= self.owner.limits().max_values {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "value identity limit exceeded",
            );
            return false;
        }
        self.value_index.insert(value.id, self.ir.values.len());
        self.ir.values.push(value);
        true
    }
    fn numeric(fact: &ValueFact) -> FiniteConstantSet {
        match fact {
            ValueFact::Constants(c) => {
                FiniteConstantSet::try_from_values(c.iter().copied().collect())
                    .expect("nonempty finite fact")
            }
            _ => FiniteConstantSet::top(),
        }
    }
    fn fact(value: FiniteConstantSet) -> ValueFact {
        value
            .as_values()
            .map(|v| ValueFact::Constants(v.iter().copied().collect()))
            .unwrap_or(ValueFact::Unknown)
    }
    fn join(&self, a: &ValueFact, b: &ValueFact) -> ValueFact {
        if *a == ValueFact::Bottom {
            return b.clone();
        }
        if *b == ValueFact::Bottom {
            return a.clone();
        }
        Self::fact(
            self.algebra
                .join_numeric(&Self::numeric(a), &Self::numeric(b)),
        )
    }
    fn publish(&mut self, id: EntityId, fact: ValueFact) -> Cell {
        let index = self.value_index[&id];
        let fact = self.join(&self.ir.values[index].fact, &fact);
        self.ir.values[index].fact = fact.clone();
        Cell { value: id, fact }
    }
    fn copy_context(&mut self, original: usize) -> Option<usize> {
        if self.ir.blocks.len() >= self.owner.limits().max_blocks {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "stack-height context block limit exceeded",
            );
            return None;
        }
        let operations = self.templates.blocks[original].operations.len();
        if self.operation_count.saturating_add(operations) > self.owner.limits().max_operations {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "stack-height context operation limit exceeded; prior validated stage retained",
            );
            return None;
        }
        if !self.owner.pay(
            self.templates
                .values
                .len()
                .saturating_add(operations.saturating_mul(8)),
        ) {
            return None;
        }
        self.operation_count += operations;
        let mut block = self.templates.blocks[original].clone();
        let mut map = BTreeMap::new();
        let ids: Vec<_> = std::iter::once(block.id)
            .chain(std::iter::once(block.memory_parameter))
            .chain(std::iter::once(block.effect_parameter))
            .chain(block.operations.iter().flat_map(|o| {
                std::iter::once(o.id)
                    .chain(o.outputs.iter().copied())
                    .chain(o.memory_output)
                    .chain(o.effect_output)
            }))
            .collect();
        for id in ids {
            map.insert(id, self.owner.id());
        }
        for (old, new) in &map {
            self.parents.insert(*new, vec![*old]);
            self.copies.push((*old, *new));
        }
        let values: Vec<_> = self
            .templates
            .values
            .iter()
            .filter(|v| map.contains_key(&v.id))
            .cloned()
            .collect();
        for mut v in values {
            v.id = map[&v.id];
            match &mut v.kind {
                ValueKind::Operation { operation, .. } => *operation = map[operation],
                ValueKind::Memory {
                    operation: Some(op),
                }
                | ValueKind::Effect {
                    operation: Some(op),
                } => *op = map[op],
                ValueKind::MemoryParameter { block }
                | ValueKind::Parameter { block, .. }
                | ValueKind::EffectParameter { block } => *block = map[block],
                _ => {}
            }
            if !self.add_value(v) {
                return None;
            }
        }
        block.id = map[&block.id];
        block.memory_parameter = map[&block.memory_parameter];
        block.effect_parameter = map[&block.effect_parameter];
        for op in &mut block.operations {
            op.id = map[&op.id];
            for v in &mut op.outputs {
                *v = map[v];
            }
            if let Some(e) = &mut op.effect_output {
                *e = map[e];
            }
            if let Some(m) = &mut op.memory_output {
                *m = map[m];
            }
        }
        let index = self.ir.blocks.len();
        self.ir.blocks.push(block);
        self.states.push(None);
        self.origins.push(original);
        Some(index)
    }
    fn connect(
        &mut self,
        from: usize,
        original: usize,
        kind: EdgeKind,
        state: &State,
        memory: EntityId,
        effect: EntityId,
    ) -> Option<EntityId> {
        let key = (original, state.stack.len());
        let index = if let Some(i) = self.contexts.get(&key) {
            *i
        } else {
            let i = if self.states[original].is_none() {
                original
            } else {
                self.copy_context(original)?
            };
            self.contexts.insert(key, i);
            i
        };
        if self.states[index].is_none() {
            let block_id = self.ir.blocks[index].id;
            let mut parameters = Vec::new();
            let mut incoming = state.clone();
            for (slot, cell) in incoming.stack.iter_mut().enumerate() {
                let id = self.owner.id();
                parameters.push(id);
                if !self.add_value(Value {
                    id,
                    kind: ValueKind::Parameter {
                        block: block_id,
                        index: slot,
                    },
                    fact: cell.fact.clone(),
                }) {
                    return None;
                }
                cell.value = id;
            }
            self.ir.blocks[index].parameters = parameters;
            self.states[index] = Some(incoming);
            if !self.enqueue(index) {
                return None;
            }
        } else {
            let mut incoming = self.states[index].clone().expect("reached context");
            let mut changed = false;
            for (old, new) in incoming.stack.iter_mut().zip(&state.stack) {
                let next = self.join(&old.fact, &new.fact);
                if old.fact != next {
                    old.fact = next;
                    changed = true;
                }
            }
            changed |= incoming.memory.merge(&state.memory);
            if changed {
                self.states[index] = Some(incoming);
                if !self.enqueue(index) {
                    return None;
                }
            }
        }
        let target = self.ir.blocks[index].id;
        let edge_id = if let Some(id) = self.edges.get(&(from, index, kind)) {
            *id
        } else {
            let id = self.owner.id();
            self.edges.insert((from, index, kind), id);
            self.ir.edges.push(Edge {
                id,
                from: self.ir.blocks[from].id,
                to: target,
                kind,
                arguments: Vec::new(),
                memory,
                effect,
            });
            id
        };
        let edge = self
            .ir
            .edges
            .iter_mut()
            .find(|e| e.id == edge_id)
            .expect("allocated edge");
        edge.arguments = state.stack.iter().map(|c| c.value).collect();
        edge.memory = memory;
        edge.effect = effect;
        Some(edge_id)
    }
    fn jump(
        &mut self,
        from: usize,
        target: &Cell,
        kind: EdgeKind,
        state: &State,
        memory: EntityId,
        effect: EntityId,
    ) -> Option<(Vec<EntityId>, bool, Vec<U256>)> {
        let ValueFact::Constants(targets) = &target.fact else {
            let known: std::collections::BTreeSet<_> = self
                .edges
                .iter()
                .filter(|((source, _, edge_kind), _)| *source == from && *edge_kind == kind)
                .map(|((_, target, _), _)| self.origins[*target])
                .collect();
            // Widening the target does not retract known continuations. Their
            // carried stack/memory must still receive this evaluation's input.
            let mut edges = Vec::new();
            for original in known {
                edges.push(self.connect(from, original, kind, state, memory, effect)?);
            }
            return Some((edges, true, Vec::new()));
        };
        let mut edges = Vec::new();
        let mut invalid = Vec::new();
        for pc in targets {
            let target_block = u64::try_from(*pc)
                .ok()
                .and_then(|pc| usize::try_from(pc).ok())
                .and_then(|pc| self.program.jumpdest_blocks().get(&pc).copied());
            if let Some(block) = target_block {
                edges.push(self.connect(from, block, kind, state, memory, effect)?);
            } else {
                invalid.push(*pc);
            }
        }
        if !invalid.is_empty() {
            self.owner.diagnostic(
                DiagnosticCode::InvalidJump,
                Severity::Info,
                Some(self.ir.blocks[from].id),
                self.ir.blocks[from].source,
                "known non-JUMPDEST target is an explicit exceptional exit".to_owned(),
            );
        }
        Some((edges, false, invalid))
    }
    fn transfer(&mut self, index: usize) -> bool {
        let origin = self.origins[index];
        let instructions = self.program.blocks()[origin].instructions.clone();
        let mut block = self.ir.blocks[index].clone();
        let mut state = self.states[index].clone().expect("queued reachable state");
        for cell in &mut state.stack {
            *cell = self.publish(cell.value, cell.fact.clone());
        }
        let mut memory = block.memory_parameter;
        let mut effect = block.effect_parameter;
        let mut terminator = None;
        for (position, instruction) in instructions.iter().enumerate() {
            if !self.owner.pay(
                state
                    .memory
                    .words
                    .len()
                    .saturating_add(state.stack.len())
                    .saturating_add(8),
            ) {
                return false;
            }
            let op = &mut block.operations[position];
            if !instruction.is_valid() {
                terminator = Some(Terminator::Trap {
                    reason: instruction
                        .invalid_reason()
                        .expect("invalid reason")
                        .to_string(),
                });
                block.operations.truncate(position + 1);
                break;
            }
            if !op.effects.is_pure() {
                op.effect_input = Some(effect);
                effect = op.effect_output.expect("effectful operation token");
            }
            let code = instruction.opcode;
            let required = if (0x80..=0x8f).contains(&code) {
                usize::from(code - 0x7f)
            } else if (0x90..=0x9f).contains(&code) {
                usize::from(code - 0x8f) + 1
            } else {
                instruction.stack_io().0
            };
            if state.stack.len() < required {
                self.owner.diagnostic(
                    DiagnosticCode::StackUnderflow,
                    Severity::Info,
                    Some(op.id),
                    op.source,
                    format!(
                        "{} needs {required} stack items, has {}",
                        op.name,
                        state.stack.len()
                    ),
                );
                terminator = Some(Terminator::Trap {
                    reason: "EVM stack underflow".to_owned(),
                });
                block.operations.truncate(position + 1);
                break;
            }
            let next_height = if (0x80..=0x8f).contains(&code) {
                state.stack.len() + 1
            } else if (0x90..=0x9f).contains(&code) {
                state.stack.len()
            } else {
                state.stack.len() - required + instruction.stack_io().1
            };
            if next_height > 1024 {
                self.owner.diagnostic(
                    DiagnosticCode::StackOverflow,
                    Severity::Info,
                    Some(op.id),
                    op.source,
                    "EVM stack exceeds 1024 items".to_owned(),
                );
                terminator = Some(Terminator::Trap {
                    reason: "EVM stack overflow".to_owned(),
                });
                block.operations.truncate(position + 1);
                break;
            }
            op.inputs.clear();
            if (0x80..=0x8f).contains(&code) {
                let cell = state.stack[state.stack.len() - required].clone();
                op.inputs.push(cell.value);
                state.stack.push(cell);
            } else if (0x90..=0x9f).contains(&code) {
                let top = state.stack.len() - 1;
                let other = state.stack.len() - required;
                op.inputs
                    .extend([state.stack[top].value, state.stack[other].value]);
                state.stack.swap(top, other);
            } else {
                let args: Vec<_> = (0..required)
                    .map(|_| state.stack.pop().expect("checked stack"))
                    .collect();
                op.inputs = args.iter().map(|c| c.value).collect();
                if op.effects.memory_read || op.effects.memory_write {
                    op.memory_input = Some(memory);
                }
                if let Some(output) = op.outputs.first().copied() {
                    let fact = match code {
                        0x5f..=0x7f => {
                            ValueFact::Constants(vec![instruction.immediate.unwrap_or(U256::ZERO)])
                        }
                        0x58 => ValueFact::Constants(vec![U256::from(instruction.pc)]),
                        0x38 => ValueFact::Constants(vec![U256::from(self.program.byte_len())]),
                        0x51 => {
                            let offset =
                                args[0].fact.singleton().and_then(|v| u64::try_from(v).ok());
                            offset
                                .and_then(|o| state.memory.words.get(&o))
                                .map(|c| c.fact.clone())
                                .unwrap_or_else(|| {
                                    if state.memory.zero {
                                        ValueFact::Constants(vec![U256::ZERO])
                                    } else {
                                        ValueFact::Unknown
                                    }
                                })
                        }
                        _ if evm_bytecode::word::arity(code).is_some() => {
                            let numeric: Vec<_> =
                                args.iter().map(|a| Self::numeric(&a.fact)).collect();
                            let work = self.algebra.operation_work(code, numeric.iter());
                            if !self.owner.pay(work) {
                                return false;
                            }
                            Self::fact(self.algebra.apply_numeric(code, numeric.iter()).value)
                        }
                        _ => ValueFact::Unknown,
                    };
                    let result = self.publish(output, fact);
                    state.stack.push(result);
                }
                if op.effects.memory_write {
                    match code {
                        0x52 => state.memory.write(
                            args[0].fact.singleton().and_then(|v| u64::try_from(v).ok()),
                            32,
                            Some(args[1].clone()),
                        ),
                        0x53 => state.memory.write(
                            args[0].fact.singleton().and_then(|v| u64::try_from(v).ok()),
                            1,
                            None,
                        ),
                        0xf1 | 0xf2 | 0xf4 | 0xfa => {
                            let base = if matches!(code, 0xf1 | 0xf2) { 3 } else { 2 };
                            if let Some(width) = args[base + 3]
                                .fact
                                .singleton()
                                .and_then(|v| u64::try_from(v).ok())
                            {
                                state.memory.write(
                                    args[base + 2]
                                        .fact
                                        .singleton()
                                        .and_then(|v| u64::try_from(v).ok()),
                                    width,
                                    None,
                                );
                            } else {
                                state.memory.clear();
                            }
                        }
                        0x37 | 0x39 | 0x3e | 0x5e | 0x3c => {
                            let (offset, width) = if code == 0x3c { (1, 3) } else { (0, 2) };
                            if let Some(width) = args[width]
                                .fact
                                .singleton()
                                .and_then(|v| u64::try_from(v).ok())
                            {
                                state.memory.write(
                                    args[offset]
                                        .fact
                                        .singleton()
                                        .and_then(|v| u64::try_from(v).ok()),
                                    width,
                                    None,
                                );
                            } else {
                                state.memory.clear();
                            }
                        }
                        _ => state.memory.clear(),
                    }
                    memory = op.memory_output.expect("writers own a memory token");
                }
                match code {
                    0x56 => {
                        let Some((edges, unresolved, invalid_targets)) =
                            self.jump(index, &args[0], EdgeKind::Jump, &state, memory, effect)
                        else {
                            return false;
                        };
                        terminator = Some(Terminator::Jump {
                            target: args[0].value,
                            edges,
                            unresolved,
                            invalid_targets,
                        });
                    }
                    0x57 => {
                        let zero = args[1].fact.singleton() == Some(U256::ZERO);
                        let nonzero = match &args[1].fact {
                            ValueFact::Constants(v) => v.iter().all(|v| *v != U256::ZERO),
                            _ => false,
                        };
                        let (then_edges, unresolved, invalid_targets) = if zero {
                            (Vec::new(), false, Vec::new())
                        } else {
                            let Some(result) =
                                self.jump(index, &args[0], EdgeKind::True, &state, memory, effect)
                            else {
                                return false;
                            };
                            result
                        };
                        let else_edge = if !nonzero && origin + 1 < self.program.blocks().len() {
                            self.connect(index, origin + 1, EdgeKind::False, &state, memory, effect)
                        } else {
                            None
                        };
                        if self.owner.stopped {
                            return false;
                        }
                        terminator = Some(Terminator::Branch {
                            condition: args[1].value,
                            target: args[0].value,
                            then_edges,
                            else_edge,
                            unresolved,
                            invalid_targets,
                        });
                    }
                    0x00 | 0xf3 | 0xfd | 0xff => {
                        terminator = Some(Terminator::Exit {
                            kind: match code {
                                0x00 => ExitKind::Stop,
                                0xf3 => ExitKind::Return,
                                0xfd => ExitKind::Revert,
                                _ => ExitKind::SelfDestruct,
                            },
                            values: op.inputs.clone(),
                        })
                    }
                    _ => {}
                }
            }
            if terminator.is_some() {
                break;
            }
        }
        if terminator.is_none() {
            if origin + 1 < self.program.blocks().len() {
                let Some(edge) = self.connect(
                    index,
                    origin + 1,
                    EdgeKind::Fallthrough,
                    &state,
                    memory,
                    effect,
                ) else {
                    return false;
                };
                terminator = Some(Terminator::Fallthrough { edge });
            } else {
                terminator = Some(Terminator::Exit {
                    kind: ExitKind::EndOfCode,
                    values: Vec::new(),
                });
            }
        }
        block.terminator = terminator.expect("all transfers are explicit");
        self.ir.blocks[index] = block;
        true
    }
}

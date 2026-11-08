//! Independent local fixed point. Identity allocation is outside transfer loops.
//! Stack height and literal control provenance select separate blocks. Numeric
//! facts still converge inside each context; its identity never follows widening.
//! Shared product values and common path guarantees remain private executable
//! facts. Wire summaries only describe observations across all evaluations.

mod context;
mod feedback;
mod state;
mod targets;
mod values;

use crate::budget::{constant_capacity, count as wide_count};
use crate::{
    Block, Completion, DiagnosticCode, Edge, EdgeKind, EntityId, ExitKind, FactScope, Ir,
    Operation, Phase, Progress, Severity, SourceSpan, StageKind, Terminator, U256, Value,
    ValueFact, ValueKind, artifact::Builder, effects::effects,
};
use analysis_kernel::{Worklist, cost};
use context::ContextKey;
use evm_bytecode::Program;
use evm_domain::{
    DomainSpec, EvmNumericAlgebra, FiniteConstantSet, FiniteSetAlgebra, ProductAlgebra,
    ValueAlgebra,
};
use feedback::FeedbackGraph;
use state::{AbstractValue, Address, Cell, State};
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
};
use targets::{JumpSite, TargetPools};

pub(crate) fn run(owner: &mut Builder<impl FnMut(Progress)>) -> bool {
    if !owner.pay(1) {
        return false;
    }
    if wide_count(owner.artifact.input.runtime_bytecode.len()) > owner.limits().max_bytecode_bytes {
        owner.stop(
            Completion::Limited,
            DiagnosticCode::Limit,
            "bytecode length exceeds max_bytecode_bytes",
        );
        return false;
    }
    let cap = match constant_capacity(owner.limits().max_constant_set) {
        Ok(capacity) => capacity,
        Err(error) => {
            owner.stop(
                Completion::Failed,
                DiagnosticCode::Limit,
                &error.to_string(),
            );
            return false;
        }
    };
    if owner.limits().max_structuring_depth > wide_count(crate::MAX_STRUCTURING_DEPTH) {
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
    if wide_count(program.blocks().len()) > owner.limits().max_blocks
        || wide_count(count) > owner.limits().max_operations
        || wide_count(value_count) > owner.limits().max_values
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
    // Use the shared default cooperation/retention policy with this run's
    // finite-set capacity. A precision cap never turns an unknown into bottom.
    let spec = DomainSpec::new(
        cap,
        NonZeroUsize::new(4).expect("nonzero reduction rounds"),
        NonZeroUsize::new(256).expect("nonzero scalar fact bound"),
    );
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(spec));
    let blocks = source.blocks.len();
    let mut recovery = Recovery {
        owner,
        program: &program,
        templates: source.clone(),
        ir: source,
        states: vec![None; blocks],
        origins: (0..blocks).collect(),
        contexts: BTreeMap::new(),
        jump_targets: TargetPools::default(),
        feedback: FeedbackGraph::default(),
        queue: Worklist::default(),
        edges: BTreeMap::new(),
        algebra,
        parents: BTreeMap::new(),
        copies: Vec::new(),
        value_index: BTreeMap::new(),
        target_candidates: BTreeMap::new(),
        operation_count: count,
        updates: vec![0; blocks],
        wire_algebra: FiniteSetAlgebra::from_spec(spec),
    };
    recovery.reindex();
    if blocks > 0 {
        recovery.states[0] = Some(State::new());
        recovery.contexts.insert(ContextKey::new(0, &[]), 0);
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
        let Some(evaluations) = recovery
            .owner
            .artifact
            .statistics
            .evaluations
            .checked_add(1)
        else {
            recovery.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "block transfer evaluation counter exhausted",
            );
            break;
        };
        recovery.owner.artifact.statistics.evaluations = evaluations;
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
    contexts: BTreeMap<ContextKey, usize>,
    jump_targets: TargetPools,
    feedback: FeedbackGraph,
    queue: Worklist<usize>,
    edges: BTreeMap<(usize, usize, EdgeKind), EntityId>,
    algebra: ValueAlgebra<ProductAlgebra>,
    // Coarse publication joins are separate from the executable product state.
    wire_algebra: FiniteSetAlgebra,
    updates: Vec<usize>,
    value_index: BTreeMap<EntityId, usize>,
    // Retain discovered legal targets even if a value reaches top before its
    // first JUMP evaluation. Candidates never close an unresolved jump.
    target_candidates: BTreeMap<EntityId, BTreeSet<usize>>,
    parents: BTreeMap<EntityId, Vec<EntityId>>,
    copies: Vec<(EntityId, EntityId)>,
    // Includes source templates and real recovery context copies.
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
        if wide_count(self.ir.values.len()) >= self.owner.limits().max_values {
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
            self.wire_algebra
                .join_numeric(&Self::numeric(a), &Self::numeric(b)),
        )
    }
    // Publication never returns an executable value: this summary can include
    // other paths and earlier iterations of the same static SSA operation.
    fn publish(&mut self, id: EntityId, fact: ValueFact) -> Option<()> {
        if let ValueFact::Constants(values) = &fact {
            if !self
                .owner
                .pay(values.len().saturating_mul(cost::key_lookup_work(
                    self.program.jumpdest_blocks().len(),
                    1,
                )))
            {
                return None;
            }
            let targets: BTreeSet<_> = values
                .iter()
                .filter_map(|v| u64::try_from(*v).ok())
                .filter_map(|v| usize::try_from(v).ok())
                .filter(|pc| self.program.jumpdest_blocks().contains_key(pc))
                .collect();
            self.remember_targets(id, targets)?;
        }
        let index = self.value_index[&id];
        let candidates =
            [&self.ir.values[index].fact, &fact]
                .into_iter()
                .fold(0usize, |count, fact| {
                    count.saturating_add(match fact {
                        ValueFact::Constants(values) => values.len(),
                        _ => 0,
                    })
                });
        // The coarse wire carrier uses owned trees/vectors, including operand
        // import and result export, independently of private product joins.
        if !self.owner.pay(
            candidates
                .saturating_mul(cost::tree_work(candidates).saturating_add(1))
                .saturating_mul(16)
                .saturating_add(32),
        ) {
            return None;
        }
        let fact = self.join(&self.ir.values[index].fact, &fact);
        self.ir.values[index].fact = fact;
        Some(())
    }
    fn remember_targets(&mut self, id: EntityId, targets: BTreeSet<usize>) -> Option<bool> {
        if targets.is_empty() {
            return Some(false);
        }
        if !self
            .owner
            .pay(cost::key_lookup_work(self.target_candidates.len(), 1))
        {
            return None;
        }
        let old_size = self.target_candidates.get(&id).map_or(0, BTreeSet::len);
        if !self.owner.pay(
            targets
                .len()
                .saturating_mul(cost::tree_work(old_size.saturating_add(targets.len()))),
        ) {
            return None;
        }
        let known = self.target_candidates.entry(id).or_default();
        known.extend(targets);
        Some(known.len() != old_size)
    }
    fn inherit_targets(&mut self, id: EntityId, source: EntityId) -> Option<bool> {
        if id == source {
            return Some(false);
        }
        if !self
            .owner
            .pay(cost::key_lookup_work(self.target_candidates.len(), 1))
        {
            return None;
        }
        let Some(targets) = self.target_candidates.get(&source) else {
            return Some(false);
        };
        if !self.owner.pay(targets.len()) {
            return None;
        }
        let targets = targets.clone();
        self.remember_targets(id, targets)
    }
    fn context_key(&mut self, original: usize, state: &State) -> Option<ContextKey> {
        let height = state.stack.len();
        // Include key construction, comparison and a possible owned insertion.
        if !self.owner.pay(cost::key_lookup_work(
            self.contexts.len(),
            height.saturating_mul(2).saturating_add(2),
        )) {
            return None;
        }
        Some(ContextKey::new(original, &state.stack))
    }
    fn copy_context(&mut self, original: usize) -> Option<usize> {
        if wide_count(self.ir.blocks.len()) >= self.owner.limits().max_blocks {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "recovery context block limit exceeded",
            );
            return None;
        }
        let operations = self.templates.blocks[original].operations.len();
        if wide_count(self.operation_count)
            .checked_add(wide_count(operations))
            .is_none_or(|total| total > self.owner.limits().max_operations)
        {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "recovery context operation limit exceeded; prior validated stage retained",
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
        self.updates.push(0);
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
        if !self.owner.pay(state.work_size().saturating_mul(4)) {
            return None;
        }
        let mut edge_state = state.clone();
        edge_state.forget_runtime();
        let state = &edge_state;
        let key = self.context_key(original, state)?;
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
        // Topology uses the actual continuation/height context, never source
        // PC ordering. Admit cycle-closing edges before choosing acceleration.
        self.feedback.observe(from, index, self.owner)?;
        if self.states[index].is_none() {
            let block_id = self.ir.blocks[index].id;
            let mut parameters = Vec::new();
            let mut incoming = state.clone();
            for (slot, cell) in incoming.stack.iter_mut().enumerate() {
                let id = self.owner.id();
                parameters.push(id);
                let fact = self.value_fact(&cell.abstract_value)?;
                if !self.add_value(Value {
                    id,
                    kind: ValueKind::Parameter {
                        block: block_id,
                        index: slot,
                    },
                    fact,
                }) {
                    return None;
                }
                self.inherit_targets(id, cell.value)?;
                cell.value = id;
            }
            self.ir.blocks[index].parameters = parameters;
            self.states[index] = Some(incoming);
            if !self.enqueue(index) {
                return None;
            }
            return Some(self.connect_edge(from, index, kind, state, memory, effect));
        }
        self.connect_context(from, index, kind, state, memory, effect)
    }
    fn connect_context(
        &mut self,
        from: usize,
        index: usize,
        kind: EdgeKind,
        state: &State,
        memory: EntityId,
        effect: EntityId,
    ) -> Option<EntityId> {
        let feedback = self.feedback.is_point(index, self.owner)?;
        let previous = self.states[index].as_ref().expect("reached context");
        // A missing memory key may synthesize its backing's known zero.
        // Reserve both sides for every possible union key before merging.
        let zero_factories = previous
            .memory
            .words
            .len()
            .saturating_add(state.memory.words.len())
            .saturating_mul(2)
            .saturating_mul(self.algebra.literal_quote_work(U256::ZERO));
        if !self
            .owner
            .pay(previous.merge_work(state).saturating_add(zero_factories))
        {
            return None;
        }
        let mut incoming = previous.clone();
        debug_assert_eq!(incoming.stack.len(), state.stack.len());
        let mut changed = false;
        let widen = feedback && self.updates[index] >= self.algebra.spec().widening_after_updates();
        for (old, new) in incoming.stack.iter_mut().zip(&state.stack) {
            // Control tags are immutable context identity, including the active
            // target. Numeric/candidate growth never changes this projection.
            debug_assert_eq!(old.control, new.control);
            changed |= self.inherit_targets(old.value, new.value)?;
            let next = self.join_value(&old.abstract_value, &new.abstract_value, widen)?;
            if old.abstract_value != next {
                old.abstract_value = next;
                changed = true;
            }
        }
        incoming.memory.merge(&state.memory, |left, right| {
            self.join_value(left, right, widen)
        })?;
        incoming.storage.merge(&state.storage, |left, right| {
            self.join_value(left, right, widen)
        })?;
        incoming.transient.merge(&state.transient, |left, right| {
            self.join_value(left, right, widen)
        })?;
        incoming.relations = incoming.relations.join(&state.relations);
        changed |= self.states[index].as_ref() != Some(&incoming);
        if changed {
            self.updates[index] = self.updates[index].saturating_add(1);
            self.states[index] = Some(incoming);
            if !self.enqueue(index) {
                return None;
            }
        }
        Some(self.connect_edge(from, index, kind, state, memory, effect))
    }
    fn connect_edge(
        &mut self,
        from: usize,
        index: usize,
        kind: EdgeKind,
        state: &State,
        memory: EntityId,
        effect: EntityId,
    ) -> EntityId {
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
        edge_id
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
        let site = JumpSite {
            original: self.origins[from],
            height: self.states[from]
                .as_ref()
                .expect("reached source")
                .stack
                .len(),
            kind,
        };
        // JUMPI already popped its target. Refine this operand explicitly with
        // the selected edge's guarantees, rather than only its surviving stack.
        let mut target_value = target.abstract_value.clone();
        if !self.refine_value(&mut target_value, &state.relations) {
            return None;
        }
        let mut invalid = Vec::new();
        let target_fact = self.value_fact(&target_value)?;
        let (discovered, unresolved) = if let ValueFact::Constants(targets) = &target_fact {
            if !self.owner.pay(
                targets.len().saturating_mul(
                    cost::key_lookup_work(self.program.jumpdest_blocks().len(), 1)
                        .saturating_add(cost::tree_work(targets.len()))
                        .saturating_add(2),
                ),
            ) {
                return None;
            }
            let mut discovered = Vec::new();
            for pc in targets {
                let legal = u64::try_from(*pc)
                    .ok()
                    .and_then(|pc| usize::try_from(pc).ok())
                    .filter(|pc| self.program.jumpdest_blocks().contains_key(pc));
                if let Some(pc) = legal {
                    discovered.push(pc);
                } else {
                    invalid.push(*pc);
                }
            }
            discovered.sort_unstable();
            discovered.dedup();
            (discovered, false)
        } else {
            // Preserve finite contributions that joined top before the first
            // evaluation, including untagged computed control addresses.
            if !self
                .owner
                .pay(cost::key_lookup_work(self.target_candidates.len(), 1))
            {
                return None;
            }
            let known = self.target_candidates.get(&target.value);
            if !self.owner.pay(known.map_or(0, BTreeSet::len)) {
                return None;
            }
            (
                known.map_or_else(Vec::new, |targets| targets.iter().copied().collect()),
                true,
            )
        };
        let wake = self.jump_targets.observe(site, &discovered, self.owner)?;
        for context in wake {
            if !self.enqueue(context) {
                return None;
            }
        }
        let targets = if unresolved {
            self.jump_targets.subscribe(site, from, self.owner)?
        } else {
            // A finite literal context follows only its own targets. Sharing
            // discoveries must not reintroduce sibling return correlations.
            discovered
        };
        if !self
            .owner
            .pay(targets.len().saturating_mul(cost::key_lookup_work(
                self.program.jumpdest_blocks().len(),
                1,
            )))
        {
            return None;
        }
        let mut edges = Vec::new();
        for pc in targets {
            let original = self.program.jumpdest_blocks()[&pc];
            // Reconnect each discovered PC with this context's own frame. Its
            // stable full tag vector selects the same physical successor on
            // every evaluation, including target/pool widening.
            edges.push(self.connect(from, original, kind, state, memory, effect)?);
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
        Some((edges, unresolved, invalid))
    }
    fn transfer(&mut self, index: usize) -> bool {
        let origin = self.origins[index];
        let instructions = self.program.blocks()[origin].instructions.clone();
        let mut block = self.ir.blocks[index].clone();
        let entry = self.states[index].as_ref().expect("queued reachable state");
        if !self.owner.pay(entry.work_size().saturating_mul(4)) {
            return false;
        }
        let mut state = entry.clone();
        for cell in &mut state.stack {
            let Some(fact) = self.value_fact(&cell.abstract_value) else {
                return false;
            };
            if self.publish(cell.value, fact).is_none() {
                return false;
            }
            // Only live block parameters receive temporary equality identities.
            // Stored values from previous iterations never inherit these ids.
            if let Ok(definition) = u32::try_from(cell.value.0) {
                cell.abstract_value = cell
                    .abstract_value
                    .clone()
                    .with_identity(self.owner.artifact.statistics.evaluations, definition);
            }
        }
        let mut memory = block.memory_parameter;
        let mut effect = block.effect_parameter;
        let mut terminator = None;
        for (position, instruction) in instructions.iter().enumerate() {
            if !self.owner.pay(state.instruction_work().saturating_add(8)) {
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
                    let Some(mut abstract_value) = self.operation_value(
                        code,
                        instruction.immediate,
                        instruction.pc,
                        &args,
                        &state,
                    ) else {
                        return false;
                    };
                    if !self.refine_value(&mut abstract_value, &state.relations) {
                        return false;
                    }
                    // Forwarded observations keep their current execution's
                    // equality identity; newly computed/unknown values get a
                    // fresh temporary definition scoped to this transfer.
                    if abstract_value
                        .identity()
                        .runtime_eq(AbstractValue::top().identity())
                        && let Ok(definition) = u32::try_from(output.0)
                    {
                        abstract_value = abstract_value
                            .with_identity(self.owner.artifact.statistics.evaluations, definition);
                    }
                    let Some(fact) = self.value_fact(&abstract_value) else {
                        return false;
                    };
                    let control = if (0x5f..=0x7f).contains(&code) {
                        u64::try_from(instruction.immediate.unwrap_or(U256::ZERO))
                            .ok()
                            .and_then(|v| usize::try_from(v).ok())
                            .filter(|pc| self.program.jumpdest_blocks().contains_key(pc))
                    } else {
                        None
                    };
                    if self.publish(output, fact).is_none() {
                        return false;
                    }
                    state.stack.push(Cell {
                        value: output,
                        abstract_value,
                        control,
                    });
                }
                if op.effects.memory_write {
                    let query_operands: &[usize] = match code {
                        0x52 | 0x53 => &[0],
                        0xf1 | 0xf2 => &[5, 6],
                        0xf4 | 0xfa => &[4, 5],
                        0x37 | 0x39 | 0x3e | 0x5e => &[0, 2],
                        0x3c => &[1, 3],
                        _ => &[],
                    };
                    let query_work = query_operands.iter().fold(0usize, |work, index| {
                        work.saturating_add(args[*index].abstract_value.singleton_work())
                    });
                    if !self.owner.pay(query_work) {
                        return false;
                    }
                    match code {
                        0x52 => state.memory.write(
                            Address::of(&args[0].abstract_value),
                            32,
                            Some(args[1].abstract_value.clone()),
                        ),
                        0x53 => state
                            .memory
                            .write(Address::of(&args[0].abstract_value), 1, None),
                        0xf1 | 0xf2 | 0xf4 | 0xfa => {
                            let base = if matches!(code, 0xf1 | 0xf2) { 3 } else { 2 };
                            if let Some(width) = args[base + 3]
                                .abstract_value
                                .singleton()
                                .and_then(|v| u64::try_from(v).ok())
                            {
                                state.memory.write(
                                    Address::of(&args[base + 2].abstract_value),
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
                                .abstract_value
                                .singleton()
                                .and_then(|v| u64::try_from(v).ok())
                            {
                                state.memory.write(
                                    Address::of(&args[offset].abstract_value),
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
                if matches!(code, 0x55 | 0x5d)
                    && !self.owner.pay(args[0].abstract_value.singleton_work())
                {
                    return false;
                }
                match code {
                    0x55 => state.storage.write(
                        Address::of(&args[0].abstract_value),
                        args[1].abstract_value.clone(),
                    ),
                    0x5d => state.transient.write(
                        Address::of(&args[0].abstract_value),
                        args[1].abstract_value.clone(),
                    ),
                    _ => {
                        if op.effects.storage_write {
                            state.storage.slots.clear();
                        }
                        if op.effects.transient_write {
                            state.transient.slots.clear();
                        }
                    }
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
                        let Some(then_state) =
                            self.assume_state(&state, &args[1].abstract_value, true)
                        else {
                            return false;
                        };
                        let Some(else_state) =
                            self.assume_state(&state, &args[1].abstract_value, false)
                        else {
                            return false;
                        };
                        let then_possible = then_state.is_some();
                        let else_possible = else_state.is_some();
                        let (then_edges, unresolved, invalid_targets) =
                            if let Some(then_state) = then_state {
                                let Some(result) = self.jump(
                                    index,
                                    &args[0],
                                    EdgeKind::True,
                                    &then_state,
                                    memory,
                                    effect,
                                ) else {
                                    return false;
                                };
                                result
                            } else {
                                (Vec::new(), false, Vec::new())
                            };
                        let else_edge = if let Some(else_state) = else_state
                            && origin + 1 < self.program.blocks().len()
                        {
                            self.connect(
                                index,
                                origin + 1,
                                EdgeKind::False,
                                &else_state,
                                memory,
                                effect,
                            )
                        } else {
                            None
                        };
                        if self.owner.stopped {
                            return false;
                        }
                        // A private path proof need not make the aggregate
                        // wire condition constant. Represent its actual CFG
                        // consequence directly so later graph consumers see a
                        // closed successor, while the original operations keep
                        // every condition evaluation and ordered effect.
                        // Missing EOF fallthrough alone is never such a proof.
                        terminator = Some(if !then_possible {
                            else_edge.map_or(
                                Terminator::Exit {
                                    kind: ExitKind::EndOfCode,
                                    values: Vec::new(),
                                },
                                |edge| Terminator::Fallthrough { edge },
                            )
                        } else if !else_possible {
                            Terminator::Jump {
                                target: args[0].value,
                                edges: then_edges,
                                unresolved,
                                invalid_targets,
                            }
                        } else {
                            Terminator::Branch {
                                condition: args[1].value,
                                target: args[0].value,
                                then_edges,
                                else_edge,
                                unresolved,
                                invalid_targets,
                            }
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

//! Session-owned reuse of deterministic stack blocks across initial revisions.
//!
//! A memo never owns a Store, rollback snapshot, call frame or outcome. Its
//! positive opcode list excludes initial reads, memory, calls and lifecycle.
//! Constant stacks and an empty relation input avoid allocating or equating
//! independent symbolic choices. Applying a delta clones the *new* payload and
//! changes only its active stack/control key. Stateful dependencies therefore
//! enter through exact input values, rather than by patching historical roots.

use crate::{
    U256,
    analysis::{
        FrameCode, FrameKey, InstructionProgress, InstructionValue, MachineEdgeKind, MachineError,
        MachinePayload,
        layers::{CodeCatalog, CodeMode, FactDependencies},
        transfer::{Execution, Successor},
    },
    bytecode::Program,
    domain::AbstractValue,
    resource::WorkBudget,
};
use std::{collections::BTreeMap, sync::Arc};

/// Cumulative optional computation reuse, separate from logical block transfers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReuseStats {
    /// Pure block computations applied to fresh runtime payloads.
    pub hits: usize,
    /// Eligible input signatures that required ordinary execution.
    pub misses: usize,
    /// Retained distinct constant-input block signatures.
    pub entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    frame: FrameKey,
    stack: Vec<U256>,
}

#[derive(Clone)]
struct StackDelta {
    key: FrameKey,
    stack: Vec<AbstractValue>,
}

impl StackDelta {
    fn extract(input: &MachinePayload, output: &MachinePayload) -> Option<Self> {
        let mut normalized = output.clone();
        normalized.normalize();
        let delta = Self {
            key: normalized.active().key.clone(),
            stack: normalized.active().stack.clone(),
        };
        let mut expected = input.clone();
        expected.active_mut().key = delta.key.clone();
        expected.active_mut().stack = delta.stack.clone();
        expected.normalize();
        // Refuse a future transfer change that mutates memory, relations,
        // suspended frames, runtime Store or any rollback checkpoint.
        (expected == normalized && delta.stack.iter().all(|v| v.singleton().is_some()))
            .then_some(delta)
    }

    fn apply(&self, input: &MachinePayload) -> MachinePayload {
        let mut output = input.clone();
        output.active_mut().key = self.key.clone();
        output.active_mut().stack = self.stack.clone();
        output
    }

    fn work_size(&self) -> usize {
        self.stack.iter().fold(
            self.key.jump_history.len().saturating_add(16),
            |cost, value| cost.saturating_add(value.work_size()),
        )
    }
}

struct Memo {
    program: Program,
    input_stack: Vec<AbstractValue>,
    exit: StackDelta,
    successors: Vec<(StackDelta, MachineEdgeKind)>,
    pcs: Vec<usize>,
    progress: Vec<InstructionProgress>,
    values: Vec<InstructionValue>,
}

impl Memo {
    fn work_size(&self) -> usize {
        self.successors.iter().fold(
            self.exit
                .work_size()
                .saturating_add(self.program.byte_len())
                .saturating_add(self.pcs.len().saturating_mul(4))
                .saturating_add(
                    self.input_stack
                        .iter()
                        .fold(0usize, |cost, value| cost.saturating_add(value.work_size())),
                ),
            |cost, (delta, _)| cost.saturating_add(delta.work_size()),
        )
    }

    fn apply(&self, input: &MachinePayload) -> Execution {
        Execution {
            payload: self.exit.apply(input),
            executed_pcs: self.pcs.clone(),
            instruction_progress: self.progress.clone(),
            instruction_values: self.values.clone(),
            successors: self
                .successors
                .iter()
                .map(|(delta, kind)| Successor {
                    payload: delta.apply(input),
                    kind: *kind,
                })
                .collect(),
            diagnostics: Vec::new(),
            frontiers: Vec::new(),
            outcomes: Vec::new(),
            completed_calls: Vec::new(),
        }
    }
}

pub(in crate::analysis) struct ExecutionReuse {
    catalog: Arc<CodeCatalog>,
    max_entries: usize,
    max_work: usize,
    memo: BTreeMap<Key, Memo>,
    pub dependencies: FactDependencies,
    stats: ReuseStats,
}

impl ExecutionReuse {
    /// Private to a session with fixed fork, entry environment and config.
    pub fn new(catalog: Arc<CodeCatalog>, max_entries: usize, max_work: usize) -> Self {
        Self {
            catalog,
            max_entries,
            max_work,
            memo: BTreeMap::new(),
            dependencies: FactDependencies::default(),
            stats: ReuseStats::default(),
        }
    }

    pub fn set_catalog(&mut self, catalog: Arc<CodeCatalog>) {
        self.catalog = catalog;
    }

    pub fn begin(&mut self) {
        self.dependencies = FactDependencies::default();
    }

    pub fn stats(&self) -> ReuseStats {
        self.stats
    }

    pub fn record_dependencies(&mut self, id: usize, input: &MachinePayload) {
        self.dependencies.record(id, input, &self.catalog);
    }

    fn key(&self, input: &MachinePayload) -> Option<Key> {
        let frame = input.active();
        if frame.code != FrameCode::Runtime || !input.relations.is_empty() {
            return None;
        }
        let program = frame.program.as_ref()?;
        let compiled = self
            .catalog
            .source(frame.key.code_hash, CodeMode::Runtime)?;
        if compiled.program() != program
            || !compiled
                .blocks()
                .get(frame.key.basic_block_index)?
                .stack_only
        {
            return None;
        }
        Some(Key {
            frame: frame.key.clone(),
            stack: frame
                .stack
                .iter()
                .map(AbstractValue::singleton)
                .collect::<Option<_>>()?,
        })
    }

    pub fn lookup(
        &mut self,
        input: &MachinePayload,
        budget: &mut WorkBudget,
    ) -> Result<Option<Execution>, MachineError> {
        MachineError::check_cancelled()?;
        let Some(key) = self.key(input) else {
            return Ok(None);
        };
        let comparison = input
            .active()
            .work_size()
            .saturating_add(self.memo.len().checked_ilog2().unwrap_or(0) as usize);
        if !budget.charge(comparison) {
            return Err(MachineError::work_failure());
        }
        let Some(memo) = self.memo.get(&key).filter(|memo| {
            memo.input_stack == input.active().stack
                && input.active().program.as_ref() == Some(&memo.program)
        }) else {
            self.stats.misses += 1;
            return Ok(None);
        };
        let copy = input
            .work_size()
            .saturating_mul(memo.successors.len().saturating_add(1))
            .saturating_add(memo.work_size());
        if copy == usize::MAX || !budget.charge(copy) {
            return Err(MachineError::work_failure());
        }
        self.stats.hits += 1;
        Ok(Some(memo.apply(input)))
    }

    pub fn remember(
        &mut self,
        input: &MachinePayload,
        execution: &Execution,
        budget: &mut WorkBudget,
    ) {
        let Some(key) = self.key(input) else {
            return;
        };
        if self.memo.len() >= self.max_entries && !self.memo.contains_key(&key) {
            return;
        }
        if !execution.frontiers.is_empty()
            || !execution.outcomes.is_empty()
            || !execution.completed_calls.is_empty()
            || !execution.diagnostics.is_empty()
        {
            return;
        }
        // Admission compares whole payloads; charge their traversal and copies.
        // Declining an optional memo never changes the executed result.
        let cost = execution
            .successors
            .iter()
            .fold(
                input
                    .work_size()
                    .saturating_add(execution.payload.work_size()),
                |cost, successor| {
                    cost.saturating_add(input.work_size())
                        .saturating_add(successor.payload.work_size())
                },
            )
            .saturating_mul(4);
        if cost == usize::MAX
            || cost > self.max_work.saturating_sub(budget.used())
            || !budget.charge(cost)
        {
            return;
        }
        let Some(exit) = StackDelta::extract(input, &execution.payload) else {
            return;
        };
        let Some(successors) = execution
            .successors
            .iter()
            .map(|s| StackDelta::extract(input, &s.payload).map(|delta| (delta, s.kind)))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        let memo = Memo {
            program: input
                .active()
                .program
                .as_ref()
                .expect("eligible runtime")
                .clone(),
            input_stack: input.active().stack.clone(),
            exit,
            successors,
            pcs: execution.executed_pcs.clone(),
            progress: execution.instruction_progress.clone(),
            values: execution.instruction_values.clone(),
        };
        if memo.work_size() > self.max_work.saturating_sub(budget.used())
            || !budget.charge(memo.work_size())
        {
            return;
        }
        self.memo.insert(key, memo);
        self.stats.entries = self.memo.len();
    }
}

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
    retained_bytes: usize,
}

impl Memo {
    fn work_size(&self) -> usize {
        self.successors.iter().fold(
            self.exit
                .work_size()
                .saturating_add(self.program.byte_len())
                .saturating_add(self.pcs.len().saturating_mul(4))
                .saturating_add(self.values.iter().fold(0usize, |cost, value| {
                    cost.saturating_add(value.numeric().work_size())
                        .saturating_add(value.provenance().origins().work_size())
                }))
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
    max_retained_bytes: usize,
    retained_bytes: usize,
    memo: BTreeMap<Key, Memo>,
    pub dependencies: FactDependencies,
    stats: ReuseStats,
}

impl ExecutionReuse {
    /// Private to a session with fixed fork, entry environment and config.
    pub fn new(
        catalog: Arc<CodeCatalog>,
        max_entries: usize,
        max_work: usize,
        max_retained_bytes: usize,
    ) -> Self {
        Self {
            catalog,
            max_entries,
            max_work,
            // Private optional-cache allowance derived from the existing memory
            // bound; no additional public tuning surface or unbounded retention.
            max_retained_bytes: max_retained_bytes.saturating_mul(4),
            retained_bytes: 0,
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

    pub fn disable_memo(&mut self) {
        self.max_entries = 0;
        self.memo.clear();
        self.retained_bytes = 0;
        self.stats.entries = 0;
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

    fn signature_work(&self, input: &MachinePayload) -> usize {
        let frame = input.active();
        // Prepay allocation/equality before key(). BTree nodes may compare
        // multiple full variable-width keys at every level; include all three
        // walks used by admission, rather than charging one scalar per level.
        let comparisons = (self.memo.len().checked_ilog2().unwrap_or(0) as usize)
            .saturating_add(1)
            .saturating_mul(128);
        frame.work_size().saturating_mul(4).saturating_add(
            frame
                .key
                .jump_history
                .len()
                .saturating_add(frame.stack.len().saturating_mul(4))
                .saturating_add(32)
                .saturating_mul(comparisons),
        )
    }

    fn optional_charge(&self, budget: &mut WorkBudget, work: usize) -> bool {
        work != usize::MAX
            && work <= self.max_work.saturating_sub(budget.used())
            && budget.charge(work)
    }

    fn retained_bound(input: &MachinePayload, execution: &Execution) -> usize {
        let scalar_unit_bytes = std::mem::size_of::<AbstractValue>().max(256);
        let stack_bytes = |payload: &MachinePayload| {
            payload.active().stack.iter().fold(
                std::mem::size_of::<FrameKey>().saturating_add(
                    payload
                        .active()
                        .key
                        .jump_history
                        .len()
                        .saturating_mul(std::mem::size_of::<usize>()),
                ),
                |cost, value| {
                    cost.saturating_add(
                        value
                            .work_size()
                            .saturating_add(1)
                            .saturating_mul(scalar_unit_bytes),
                    )
                },
            )
        };
        let bytes =
            execution.successors.iter().fold(
                stack_bytes(input)
                    .saturating_add(stack_bytes(&execution.payload))
                    .saturating_add(input.active().program.as_ref().map_or(0, Program::byte_len))
                    .saturating_add(
                        input
                            .active()
                            .stack
                            .len()
                            .saturating_mul(std::mem::size_of::<U256>()),
                    )
                    .saturating_add(
                        execution
                            .executed_pcs
                            .len()
                            .saturating_mul(std::mem::size_of::<usize>()),
                    )
                    .saturating_add(
                        execution
                            .instruction_progress
                            .len()
                            .saturating_mul(std::mem::size_of::<InstructionProgress>()),
                    )
                    .saturating_add(execution.instruction_values.iter().fold(
                        0usize,
                        |cost, value| {
                            cost.saturating_add(std::mem::size_of::<InstructionValue>())
                                .saturating_add(
                                    value
                                        .numeric()
                                        .work_size()
                                        .saturating_add(value.provenance().origins().work_size())
                                        .saturating_mul(scalar_unit_bytes),
                                )
                        },
                    )),
                |cost, successor| {
                    cost.saturating_add(stack_bytes(&successor.payload))
                        .saturating_add(std::mem::size_of::<MachineEdgeKind>())
                },
            );
        // Conservative typed-payload accounting includes owned Vec/map headers,
        // constants, source metadata and scalar DAG nodes held alive. Program is
        // shared with the catalog, but counting its bytes here is conservative.
        // This limits memo retention, not total process RSS or executor memory.
        bytes.saturating_add(
            std::mem::size_of::<Memo>()
                .saturating_add(std::mem::size_of::<Key>())
                .saturating_add(2 * std::mem::size_of::<usize>())
                .saturating_mul(16),
        )
    }

    pub fn lookup(
        &mut self,
        input: &MachinePayload,
        budget: &mut WorkBudget,
    ) -> Result<Option<Execution>, MachineError> {
        MachineError::check_cancelled()?;
        if !budget.charge(0) {
            return Err(MachineError::work_failure());
        }
        if self.max_entries == 0 {
            return Ok(None);
        }
        let signature_work = self.signature_work(input);
        // Optional lookup must not consume the remaining execution budget just
        // to discover that constructing/comparing its signature cannot fit.
        if signature_work == usize::MAX
            || signature_work > self.max_work.saturating_sub(budget.used())
        {
            return Ok(None);
        }
        if !budget.charge(signature_work) {
            return Err(MachineError::work_failure());
        }
        let Some(key) = self.key(input) else {
            return Ok(None);
        };
        let Some(memo) = self.memo.get(&key) else {
            self.stats.misses += 1;
            return Ok(None);
        };
        let comparison = memo.work_size().saturating_add(input.active().work_size());
        if comparison == usize::MAX || comparison > self.max_work.saturating_sub(budget.used()) {
            self.stats.misses += 1;
            return Ok(None);
        }
        if !budget.charge(comparison) {
            return Err(MachineError::work_failure());
        }
        if memo.input_stack != input.active().stack
            || input.active().program.as_ref() != Some(&memo.program)
        {
            self.stats.misses += 1;
            return Ok(None);
        }
        let copy = input
            .work_size()
            .saturating_mul(memo.successors.len().saturating_add(1))
            .saturating_add(memo.work_size());
        if copy == usize::MAX || copy > self.max_work.saturating_sub(budget.used()) {
            self.stats.misses += 1;
            return Ok(None);
        }
        if !budget.charge(copy) {
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
        if self.max_entries == 0 {
            return;
        }
        if !self.optional_charge(budget, self.signature_work(input)) {
            return;
        }
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
        let retained_bytes = Self::retained_bound(input, execution);
        let previous = self.memo.get(&key);
        let previous_bytes = previous.map_or(0, |memo| memo.retained_bytes);
        let release_work = previous.map_or(0, Memo::work_size);
        let next_bytes = self
            .retained_bytes
            .saturating_sub(previous_bytes)
            .saturating_add(retained_bytes);
        if next_bytes == usize::MAX || next_bytes > self.max_retained_bytes {
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
            .saturating_mul(4)
            .saturating_add(release_work)
            .saturating_add(execution.executed_pcs.len())
            .saturating_add(execution.instruction_progress.len())
            .saturating_add(
                execution
                    .instruction_values
                    .iter()
                    .fold(0usize, |cost, value| {
                        cost.saturating_add(value.numeric().work_size())
                            .saturating_add(value.provenance().origins().work_size())
                    })
                    .saturating_mul(2),
            );
        if !self.optional_charge(budget, cost) {
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
            retained_bytes,
        };
        self.memo.insert(key, memo);
        self.retained_bytes = next_bytes;
        self.stats.entries = self.memo.len();
    }
}

#[cfg(test)]
mod tests;

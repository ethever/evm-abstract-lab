//! Structural admission precedes graph allocation. Censuses only inspect cached
//! lengths/opcodes and pay each outer census before walking its children. Numeric
//! payload copies, queries and comparisons keep their separate native invoices.

use super::{Instruction, PartialWorldSsa, SsaError, Transition, block::Block};
use crate::{analysis::EvmWorldAnalysis, domain::WordElement, resource::WorkBudget};
use std::mem::size_of;

fn words<T>() -> usize {
    size_of::<T>().div_ceil(size_of::<usize>())
}
fn plus(a: usize, b: usize) -> usize {
    a.saturating_add(b)
}
fn times(a: usize, b: usize) -> usize {
    a.saturating_mul(b)
}

/// Integer BTree keys need at most one search and one repair path per update.
/// A node has at most eleven keys; twelve visits per level includes its branch.
/// Binary height is an upper bound for this multiway tree's height.
fn tree(operations: usize, entries: usize) -> usize {
    let height = usize::BITS as usize - entries.leading_zeros() as usize + 1;
    times(operations, times(2 * 12, height))
}

/// Both cancellation owners are checked even on empty collections and after
/// the last opaque operation. None is only the standalone structural utility.
pub(super) fn checkpoint(budget: Option<&mut WorkBudget>) -> Result<(), SsaError> {
    match budget {
        Some(budget) => super::pay(budget, 0),
        None => super::checkpoint(),
    }
}

#[derive(Default)]
struct Execution {
    states: usize,
    edges: usize,
    frames: usize,
    slots: usize,
    instructions: usize,
    operands: usize,
    results: usize,
    edge_frames: usize,
    edge_slots: usize,
    phi_inputs: usize,
    successor_scans: usize,
}
impl Execution {
    fn definitions(&self) -> usize {
        plus(plus(self.slots, self.results), self.edges)
    }
    fn effects(&self) -> usize {
        plus(plus(self.states, self.instructions), self.edges)
    }
    fn construction(&self) -> usize {
        // Owned block/phi/instruction/transition records and every copied stack,
        // operand, result, incoming argument and effect edge, in host words.
        [
            1,
            times(self.states, words::<super::WorldBlock>()),
            times(self.frames, words::<Vec<usize>>()),
            times(self.slots, plus(words::<super::FramePhi>(), 1)),
            times(
                self.instructions,
                plus(words::<Instruction>(), words::<(usize, usize, usize)>()),
            ),
            self.operands,
            self.results,
            times(self.edges, words::<Transition>()),
            times(self.edge_frames, words::<Vec<usize>>()),
            self.edge_slots,
            times(self.phi_inputs, words::<(usize, usize)>()),
            times(self.edges, words::<super::EffectInput>()),
        ]
        .into_iter()
        .fold(0, plus)
    }
    fn verification(&self) -> usize {
        let names = plus(self.definitions(), self.effects());
        // Each state filters the whole edge vector. Each incoming phi argument
        // and transition stack is reconstructed/compared; names enter both the
        // global and block-local trees. A third tree serves covered edge lookup.
        [
            1,
            self.construction(),
            times(self.states, self.edges),
            self.phi_inputs,
            self.edge_slots,
            times(self.edge_frames, words::<crate::analysis::FrameKey>()),
            self.successor_scans,
            tree(
                plus(times(names, 2), plus(self.operands, self.phi_inputs)),
                names,
            ),
            tree(plus(times(self.edges, 4), self.phi_inputs), self.edges),
        ]
        .into_iter()
        .fold(0, plus)
    }
}

fn execution<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<Execution, SsaError> {
    let mut count = Execution {
        states: analysis.states().len(),
        edges: analysis.edges().len(),
        ..Execution::default()
    };
    super::pay(budget, plus(1, plus(count.states, count.edges)))?;
    for state in analysis.states() {
        let depth = state.entry.call_stack.depth();
        super::pay(budget, plus(depth, state.executed_pcs.len()))?;
        count.frames = plus(count.frames, depth);
        for frame in state.entry.call_stack.iter() {
            count.slots = plus(count.slots, frame.stack.len());
        }
        count.instructions = plus(count.instructions, state.executed_pcs.len());
        if let Some(block) = state
            .program()
            .and_then(|p| p.blocks().get(state.active().basic_block_index))
        {
            for instruction in block.instructions.iter().take(state.executed_pcs.len()) {
                let (inputs, outputs) = instruction.stack_io();
                count.operands = plus(count.operands, inputs);
                count.results = plus(count.results, outputs);
            }
        }
    }
    for edge in analysis.edges() {
        let Some(source) = analysis.states().get(edge.from) else {
            continue;
        };
        let Some(target) = analysis.states().get(edge.to) else {
            continue;
        };
        let entry_depth = source.entry.call_stack.depth();
        let exit_depth = source
            .exit
            .as_ref()
            .map_or(0, |exit| exit.call_stack.depth());
        let target_depth = target.entry.call_stack.depth();
        super::pay(budget, plus(plus(entry_depth, exit_depth), target_depth))?;
        // A pending/stale prefix retains entry stacks, and a current receipt
        // supplies exit stacks. Their union bounds either construction route.
        for frame in source.entry.call_stack.iter() {
            count.edge_slots = plus(count.edge_slots, frame.stack.len());
        }
        if let Some(exit) = &source.exit {
            for frame in exit.call_stack.iter() {
                count.edge_slots = plus(count.edge_slots, frame.stack.len());
            }
        }
        count.edge_frames = plus(count.edge_frames, plus(entry_depth.max(exit_depth), 1));
        for frame in target.entry.call_stack.iter() {
            count.phi_inputs = plus(count.phi_inputs, frame.stack.len());
        }
        count.successor_scans = plus(
            count.successor_scans,
            source
                .execution_evidence()
                .map_or(0, |receipt| receipt.successors().len()),
        );
    }
    super::pay(budget, 0)?;
    Ok(count)
}

pub(super) fn world<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let count = execution(analysis, budget)?;
    // build_world also runs its full verifier. Both passes are reserved before
    // the first block/instruction/stack allocation, not billed after return.
    super::pay(budget, plus(count.construction(), count.verification()))
}

pub(super) fn partial_build<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let count = execution(analysis, budget)?;
    super::pay(
        budget,
        [
            count.construction(),
            count.successor_scans,
            tree(times(count.edges, 2), count.edges),
            count.edges,
        ]
        .into_iter()
        .fold(0, plus),
    )
}

#[derive(Default)]
struct Artifact {
    units: usize,
    definitions: usize,
    operands: usize,
    instructions: usize,
    phi_inputs: usize,
}
fn artifact<I>(
    blocks: &[Block<I>],
    transitions: &[Transition],
    budget: &mut WorkBudget,
    origin: impl Fn(&I) -> &Instruction,
) -> Result<Artifact, SsaError> {
    super::pay(budget, plus(1, plus(blocks.len(), transitions.len())))?;
    let mut count = Artifact::default();
    for block in blocks {
        super::pay(
            budget,
            [
                block.phis.len(),
                block.instructions.len(),
                block.exit_frames.len(),
            ]
            .into_iter()
            .fold(0, plus),
        )?;
        count.units = plus(
            count.units,
            [
                words::<Block<I>>(),
                times(block.phis.len(), words::<super::FramePhi>()),
                times(block.instructions.len(), words::<I>()),
                times(block.exit_frames.len(), words::<Vec<usize>>()),
                times(block.effect.inputs.len(), words::<super::EffectInput>()),
                block.open_incoming.len(),
            ]
            .into_iter()
            .fold(0, plus),
        );
        count.definitions = plus(count.definitions, block.phis.len());
        count.instructions = plus(count.instructions, block.instructions.len());
        for phi in &block.phis {
            count.phi_inputs = plus(count.phi_inputs, phi.inputs.len());
            count.units = plus(
                count.units,
                times(phi.inputs.len(), words::<(usize, usize)>()),
            );
        }
        for row in &block.instructions {
            let row = origin(row);
            count.units = plus(count.units, plus(row.operands.len(), row.results.len()));
            count.definitions = plus(count.definitions, row.results.len());
            count.operands = plus(count.operands, row.operands.len());
        }
        for frame in &block.exit_frames {
            count.units = plus(count.units, frame.len());
        }
    }
    for transition in transitions {
        super::pay(budget, transition.stacks.len())?;
        count.units = plus(
            count.units,
            [
                words::<Transition>(),
                transition.operands.len(),
                times(transition.stacks.len(), words::<Vec<usize>>()),
            ]
            .into_iter()
            .fold(0, plus),
        );
        count.operands = plus(count.operands, transition.operands.len());
        count.definitions = plus(count.definitions, usize::from(transition.result.is_some()));
        for frame in &transition.stacks {
            count.units = plus(count.units, frame.len());
        }
    }
    super::pay(budget, 0)?;
    Ok(count)
}

pub(super) fn partial_verify<N: WordElement>(
    ir: &PartialWorldSsa<N>,
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let native = execution(analysis, budget)?;
    let actual = artifact(ir.blocks(), ir.transitions(), budget, |row| {
        &row.instruction
    })?;
    super::pay(
        budget,
        [
            native.verification(),
            actual.units,
            tree(
                plus(actual.definitions, actual.operands),
                plus(ir.value_count(), ir.effect_count()),
            ),
            times(ir.deferred_edges().len(), words::<super::DeferredSsaEdge>()),
        ]
        .into_iter()
        .fold(0, plus),
    )
}

pub(super) fn normalize<N: WordElement>(
    raw: &PartialWorldSsa<N>,
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let count = artifact(raw.blocks(), raw.transitions(), budget, |row| {
        &row.instruction
    })?;
    // Copy/map the envelope, allocate the three indexed name/mapping/known
    // tables, register origins, populate dependencies, and scan observed facts.
    // Copy-chain walks and fixed-word evaluation are paid where executed.
    let work = [
        1,
        count.units,
        count.units,
        times(raw.value_count(), plus(2, words::<super::ValueMapping>())),
        count.definitions,
        // At most one observation and one independent proof per definition;
        // reserve the actual generic record layout before its Vec allocation.
        times(times(raw.value_count(), 2), words::<super::ValueFact<N>>()),
        count.phi_inputs,
        times(
            count.operands,
            plus(2, times(2, words::<super::ValueOperand>())),
        ),
        times(count.instructions, words::<super::NormalizedOperation>()),
        times(
            raw.deferred_edges().len(),
            words::<super::DeferredSsaEdge>(),
        ),
    ]
    .into_iter()
    .fold(0, plus);
    super::pay(budget, work)?;
    super::pay(budget, plus(raw.blocks().len(), raw.transitions().len()))?;
    for block in raw.blocks() {
        let state = &analysis.states()[block.state];
        super::pay(
            budget,
            plus(state.entry.call_stack.depth(), block.instructions.len()),
        )?;
        let captured = state
            .execution_evidence()
            .and_then(|receipt| receipt.instruction_values())
            .map_or(0, <[_]>::len);
        let probes = usize::BITS as usize - captured.leading_zeros() as usize + 1;
        for row in &block.instructions {
            super::pay(budget, times(row.instruction.results.len(), probes))?;
        }
    }
    super::pay(budget, 0)
}

pub(super) fn normalized_verify<N: WordElement>(
    ir: &super::NormalizedSsa<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let count = artifact(ir.blocks(), ir.transitions(), budget, |row| &row.origin)?;
    super::pay(budget, plus(ir.facts().len(), ir.mappings().len()))?;
    super::pay(budget, count.instructions)?;
    let mut operation_words = 0;
    for block in ir.blocks() {
        for row in &block.instructions {
            if let super::NormalizedOperation::Opcode { operands, .. } = &row.operation {
                operation_words = plus(
                    operation_words,
                    times(operands.len(), words::<super::ValueOperand>()),
                );
            }
        }
    }
    let mut dependencies = 0;
    for mapping in ir.mappings() {
        dependencies = plus(dependencies, mapping.dependencies.len());
    }
    // Definition vector + set, then comparisons of every structural field,
    // fact header and mapping dependency. Native payload equality is separate.
    super::pay(
        budget,
        [
            1,
            count.units,
            operation_words,
            count.definitions,
            tree(count.definitions, ir.value_count()),
            times(ir.facts().len(), words::<super::ValueFact<N>>()),
            times(ir.mappings().len(), words::<super::ValueMapping>()),
            dependencies,
            times(ir.deferred_edges().len(), words::<super::DeferredSsaEdge>()),
        ]
        .into_iter()
        .fold(0, plus),
    )
}

/// Concrete folding has fixed word width, independent of any opaque carrier.
pub(super) fn evaluate(op: u8, budget: &mut WorkBudget) -> Result<(), SsaError> {
    super::pay(budget, crate::domain::concrete::evaluation_work(op))
}

/// Admit report reference/catalog construction before formatting allocates.
/// Opaque display/serialization is still a caller-funded export boundary.
/// The Store owner supplies its cached map census; no numeric visitor is used.
pub(crate) fn explanation<N: WordElement>(
    analysis: &EvmWorldAnalysis<N>,
    budget: &mut WorkBudget,
) -> Result<(), SsaError> {
    let count = execution(analysis, budget)?;
    let accounts = analysis.world().accounts().len();
    let outcomes = analysis.outcomes().len();
    let summaries = analysis.summaries().len();
    super::pay(
        budget,
        [
            1,
            accounts,
            outcomes,
            summaries,
            analysis.frontiers().len(),
            analysis.procedure_summaries().len(),
        ]
        .into_iter()
        .fold(0, plus),
    )?;
    let mut entries = plus(accounts, plus(outcomes, plus(count.frames, count.edges)));
    let mut code_bytes = 0;
    for account in analysis.world().accounts().values() {
        if let crate::world::Code::Runtime(program) = &account.code {
            code_bytes = plus(code_bytes, program.byte_len());
        }
    }
    for state in analysis.states() {
        for payload in std::iter::once(&state.entry).chain(state.exit.iter()) {
            super::pay(budget, payload.call_stack.depth())?;
            entries = plus(entries, payload.call_stack.depth());
            for frame in payload.call_stack.iter() {
                if let Some(program) = &frame.program {
                    code_bytes = plus(code_bytes, program.byte_len());
                }
            }
        }
    }
    for frontier in analysis.frontiers() {
        if let Some(target) = &frontier.target {
            entries = plus(entries, target.frames.len());
        }
    }
    for outcome in analysis.outcomes() {
        let census = outcome.store.finalization_entries();
        super::pay(budget, census)?;
        entries = plus(entries, census);
        code_bytes = plus(code_bytes, outcome.store.code_identity_bytes());
    }
    for summary in analysis.summaries() {
        super::pay(budget, summary.outputs.len())?;
        entries = plus(entries, summary.outputs.len());
        for output in &summary.outputs {
            let census = output.store.finalization_entries();
            super::pay(budget, census)?;
            entries = plus(entries, census);
            code_bytes = plus(code_bytes, output.store.code_identity_bytes());
        }
    }
    for summary in analysis.procedure_summaries() {
        super::pay(budget, summary.procedures.len())?;
        entries = plus(entries, summary.procedures.len());
        for procedure in &summary.procedures {
            entries = plus(
                entries,
                [
                    procedure.dependencies.len(),
                    procedure.covered_blocks.len(),
                    procedure.outputs.len(),
                ]
                .into_iter()
                .fold(0, plus),
            );
        }
    }
    // Both capture and legend maps insert/search these identities. Raw code
    // bytes bound instruction/PC/immediate rows even for declared-only code.
    super::pay(
        budget,
        [
            1,
            count.construction(),
            tree(times(entries, 2), entries),
            times(code_bytes, words::<Instruction>()),
        ]
        .into_iter()
        .fold(0, plus),
    )
}

/// Opaque range operands have separate copy/query invoices; the surrounding
/// machine key owns ordinary frame records and jump-history vectors.
pub(super) fn frontier<N: WordElement>(
    frontier: &crate::analysis::EvmMachineFrontier<N>,
    budget: &mut WorkBudget,
) -> Result<usize, SsaError> {
    let mut work = words::<crate::analysis::EvmMachineFrontier<N>>();
    if let crate::analysis::FrontierReason::RpcAcquisition { failure, .. } = &frontier.reason {
        work = plus(
            work,
            plus(
                words::<crate::world::rpc::Failure>(),
                plus(failure.message.len(), failure.context.method.len()),
            ),
        );
        if let Some(crate::world::rpc::FailureCause::Transport(cause)) = &failure.cause {
            work = plus(work, cause.native_diagnostic.len());
        }
    }
    if let Some(key) = &frontier.target {
        super::pay(budget, key.frames.len())?;
        work = plus(work, words::<crate::analysis::MachineKey>());
        for frame in &key.frames {
            work = plus(
                work,
                plus(
                    words::<crate::analysis::FrameKey>(),
                    frame.jump_history.len(),
                ),
            );
        }
    }
    Ok(work)
}

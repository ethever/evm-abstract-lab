//! Borrowed rendering views keep the legacy SSA APIs and the final normalized
//! artifact on the same frame, effect, transition and instruction layout path.

use crate::{
    analysis::{InstructionProgress, MachineFrontier, Status},
    ssa::{
        DeferredSsaEdge, EffectId, EffectPhi, FramePhi, Instruction, NormalizedBlock,
        NormalizedInstruction, NormalizedSsa, PartialBlockCoverage, PartialInstruction,
        PartialWorldBlock, PartialWorldSsa, Transition, ValueId, WorldBlock, WorldSsa,
    },
};

#[derive(Clone, Copy)]
pub(super) enum Artifact<'a> {
    World(&'a WorldSsa),
    Partial(&'a PartialWorldSsa),
    Normalized(&'a NormalizedSsa),
}

impl<'a> Artifact<'a> {
    pub(super) fn status(self) -> Status {
        match self {
            Self::World(_) => Status::Converged,
            Self::Partial(ir) => ir.status(),
            Self::Normalized(ir) => ir.status(),
        }
    }

    pub(super) fn block_count(self) -> usize {
        match self {
            Self::World(ir) => ir.blocks().len(),
            Self::Partial(ir) => ir.blocks().len(),
            Self::Normalized(ir) => ir.blocks().len(),
        }
    }

    pub(super) fn value_count(self) -> usize {
        match self {
            Self::World(ir) => ir.value_count(),
            Self::Partial(ir) => ir.value_count(),
            Self::Normalized(ir) => ir.value_count(),
        }
    }

    pub(super) fn effect_count(self) -> usize {
        match self {
            Self::World(ir) => ir.effect_count(),
            Self::Partial(ir) => ir.effect_count(),
            Self::Normalized(ir) => ir.effect_count(),
        }
    }

    pub(super) fn transitions(self) -> &'a [Transition] {
        match self {
            Self::World(ir) => ir.transitions(),
            Self::Partial(ir) => ir.transitions(),
            Self::Normalized(ir) => ir.transitions(),
        }
    }

    pub(super) fn deferred_edges(self) -> &'a [DeferredSsaEdge] {
        match self {
            Self::World(_) => &[],
            Self::Partial(ir) => ir.deferred_edges(),
            Self::Normalized(ir) => ir.deferred_edges(),
        }
    }

    pub(super) fn frontiers(self) -> &'a [MachineFrontier] {
        match self {
            Self::World(_) => &[],
            Self::Partial(ir) => ir.frontiers(),
            Self::Normalized(ir) => ir.frontiers(),
        }
    }

    pub(super) fn blocks(self) -> impl Iterator<Item = Block<'a>> {
        let (world, partial, normalized) = match self {
            Self::World(ir) => (Some(ir), None, None),
            Self::Partial(ir) => (None, Some(ir), None),
            Self::Normalized(ir) => (None, None, Some(ir)),
        };
        world
            .into_iter()
            .flat_map(|ir| ir.blocks().iter().map(Block::world))
            .chain(
                partial
                    .into_iter()
                    .flat_map(|ir| ir.blocks().iter().map(Block::partial)),
            )
            .chain(
                normalized
                    .into_iter()
                    .flat_map(|ir| ir.blocks().iter().map(Block::normalized)),
            )
    }
}

#[derive(Clone, Copy)]
pub(super) struct Block<'a> {
    pub(super) state: usize,
    pub(super) coverage: PartialBlockCoverage,
    pub(super) phis: &'a [FramePhi],
    pub(super) instructions: Instructions<'a>,
    pub(super) exit_frames: &'a [Vec<ValueId>],
    pub(super) effect: &'a EffectPhi,
    pub(super) exit_effect: EffectId,
    pub(super) open_incoming: &'a [usize],
    pub(super) incoming_complete: bool,
    effects: &'a [(usize, EffectId, EffectId)],
}

impl<'a> Block<'a> {
    fn world(block: &'a WorldBlock) -> Self {
        Self {
            state: block.state,
            coverage: PartialBlockCoverage::Current,
            phis: &block.phis,
            instructions: Instructions::World(&block.instructions),
            exit_frames: &block.exit_frames,
            effect: &block.effect,
            exit_effect: block.exit_effect,
            open_incoming: &[],
            incoming_complete: true,
            effects: &block.effects,
        }
    }

    fn partial(block: &'a PartialWorldBlock) -> Self {
        Self {
            state: block.state,
            coverage: block.coverage,
            phis: &block.phis,
            instructions: Instructions::Partial(&block.instructions),
            exit_frames: &block.exit_frames,
            effect: &block.effect,
            exit_effect: block.exit_effect,
            open_incoming: &block.open_incoming,
            incoming_complete: block.incoming_complete,
            effects: &[],
        }
    }

    fn normalized(block: &'a NormalizedBlock) -> Self {
        Self {
            state: block.state,
            coverage: block.coverage,
            phis: &block.phis,
            instructions: Instructions::Normalized(&block.instructions),
            exit_frames: &block.exit_frames,
            effect: &block.effect,
            exit_effect: block.exit_effect,
            open_incoming: &block.open_incoming,
            incoming_complete: block.incoming_complete,
            effects: &[],
        }
    }

    pub(super) fn effects(self) -> impl Iterator<Item = (usize, EffectId, EffectId)> {
        self.effects
            .iter()
            .copied()
            .chain(self.instructions.iter().filter_map(|row| {
                row.effect_result
                    .map(|result| (row.origin.pc, row.effect_input, result))
            }))
    }
}

#[derive(Clone, Copy)]
pub(super) enum Instructions<'a> {
    World(&'a [Instruction]),
    Partial(&'a [PartialInstruction]),
    Normalized(&'a [NormalizedInstruction]),
}

impl<'a> Instructions<'a> {
    pub(super) fn is_empty(self) -> bool {
        match self {
            Self::World(rows) => rows.is_empty(),
            Self::Partial(rows) => rows.is_empty(),
            Self::Normalized(rows) => rows.is_empty(),
        }
    }

    pub(super) fn iter(self) -> impl Iterator<Item = Row<'a>> {
        let (world, partial, normalized) = match self {
            Self::World(rows) => (Some(rows), None, None),
            Self::Partial(rows) => (None, Some(rows), None),
            Self::Normalized(rows) => (None, None, Some(rows)),
        };
        world
            .into_iter()
            .flatten()
            .map(|origin| Row {
                origin,
                normalized: None,
                progress: InstructionProgress::Completed,
                effect_input: 0,
                effect_result: None,
            })
            .chain(partial.into_iter().flatten().map(|row| Row {
                origin: &row.instruction,
                normalized: None,
                progress: row.progress,
                effect_input: row.effect_input,
                effect_result: row.effect_result,
            }))
            .chain(normalized.into_iter().flatten().map(|row| Row {
                origin: &row.origin,
                normalized: Some(row),
                progress: row.progress,
                effect_input: row.effect_input,
                effect_result: row.effect_result,
            }))
    }
}

#[derive(Clone, Copy)]
pub(super) struct Row<'a> {
    pub(super) origin: &'a Instruction,
    pub(super) normalized: Option<&'a NormalizedInstruction>,
    pub(super) progress: InstructionProgress,
    pub(super) effect_input: EffectId,
    pub(super) effect_result: Option<EffectId>,
}

impl Row<'_> {
    pub(super) fn write_body(self, output: &mut String) {
        match self.normalized {
            Some(row) => super::instruction::write_normalized_body(output, row),
            None => super::instruction::write_ssa_body(output, self.origin),
        }
    }
}

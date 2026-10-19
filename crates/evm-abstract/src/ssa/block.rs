//! The shared world-block envelope. Instructions retain their own execution or
//! normalization semantics; mapping the envelope never renames effects or edges.

use super::{
    EffectId, EffectPhi, FramePhi, Instruction, PartialBlockCoverage, ValueId, ValueOrigin,
};
use serde::Serialize;

/// A native machine state with typed instruction attempts and explicit coverage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Block<I> {
    /// Original native machine-state ID.
    pub state: usize,
    /// Current, stale, or absent transfer evidence.
    pub coverage: PartialBlockCoverage,
    /// Entry parameters whose inputs cover only supported transitions.
    pub phis: Vec<FramePhi>,
    /// Retained attempts in original program order.
    pub instructions: Vec<I>,
    /// Observed stacks, including consumed operands of pending instructions.
    /// Stale/unexecuted blocks retain entry names, not an observed exit.
    pub exit_frames: Vec<Vec<ValueId>>,
    /// Entry effect bundle with supported incoming transitions.
    pub effect: EffectPhi,
    /// Last observed effect, or only the entry effect without current coverage.
    pub exit_effect: EffectId,
    /// Native incoming edge IDs without a supported SSA transition.
    pub open_incoming: Vec<usize>,
    /// Whether the analysis and this state's incoming coverage are closed.
    pub incoming_complete: bool,
}

impl<I> Block<I> {
    /// Map ordered instruction rows while leaving metadata borrowed. Exporters
    /// can enrich rows without copying native phi or effect containers.
    pub fn try_map_instruction_rows<J, E>(
        &self,
        mut map: impl FnMut(usize, &I) -> Result<J, E>,
    ) -> Result<Vec<J>, E> {
        let rows = self.instructions.iter().enumerate();
        rows.map(|(index, item)| map(index, item)).collect()
    }

    /// Copy the envelope while mapping instructions in order. A failure stops
    /// before later instructions; the source block remains available unchanged.
    pub fn try_map_instructions<J, E>(
        &self,
        map: impl FnMut(usize, &I) -> Result<J, E>,
    ) -> Result<Block<J>, E> {
        Ok(Block {
            state: self.state,
            coverage: self.coverage,
            phis: self.phis.clone(),
            instructions: self.try_map_instruction_rows(map)?,
            exit_frames: self.exit_frames.clone(),
            effect: self.effect.clone(),
            exit_effect: self.exit_effect,
            open_incoming: self.open_incoming.clone(),
            incoming_complete: self.incoming_complete,
        })
    }

    /// Frame definitions and uses, including parallel phi inputs and every
    /// suspended stack. Instruction values and effect/edge IDs are separate.
    pub fn frame_values_mut(&mut self) -> impl Iterator<Item = &mut ValueId> {
        self.phis
            .iter_mut()
            .flat_map(|phi| {
                std::iter::once(&mut phi.result)
                    .chain(phi.inputs.iter_mut().map(|(_, value)| value))
            })
            .chain(self.exit_frames.iter_mut().flatten())
    }

    /// Retained definitions and native origins in naming order. The projection
    /// selects bytecode evidence, never a computation's rewritten operands.
    pub fn definitions<'a>(
        &'a self,
        origin: impl Fn(&'a I) -> &'a Instruction + 'a,
    ) -> impl Iterator<Item = (ValueId, ValueOrigin)> + 'a {
        let phis = self.phis.iter().map(|phi| {
            let origin = ValueOrigin::Phi {
                state: self.state,
                frame: phi.frame,
                slot: phi.slot,
            };
            (phi.result, origin)
        });
        let rows = self.instructions.iter().enumerate();
        let instructions = rows.flat_map(move |(index, step)| {
            let source = origin(step);
            let site = ValueOrigin::Instruction {
                state: self.state,
                index,
                pc: source.pc,
                opcode: source.opcode,
            };
            source
                .results
                .iter()
                .map(move |value| (*value, site.clone()))
        });
        phis.chain(instructions)
    }
}

#[cfg(test)]
mod tests;

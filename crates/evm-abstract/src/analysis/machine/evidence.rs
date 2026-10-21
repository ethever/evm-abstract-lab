//! Receipt of the latest block transfer, separate from accumulated graph edges.
//! A changed entry invalidates the receipt until that entry is executed again.

use crate::domain::WordElement;

use super::MachineEdgeKind;
use crate::domain::{EvmValue, provenance::Provenance};
use serde::Serialize;
use std::sync::Arc;

/// Progress actually reached by one visited bytecode instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum InstructionProgress {
    /// Reached the opcode, without consuming its stack operands.
    Started,
    /// Operands were consumed, but no normal result was produced. Memory or
    /// other preparation may already have changed; the opcode is not complete.
    OperandsConsumed,
    /// The local stack computation and ordinary instruction effects completed.
    Completed,
    /// Entered terminal or call dispatch. Only observed transitions carry its
    /// results; this does not assert that every possible target was explored.
    Dispatched,
    /// Invalid opcode or stack fault, without a normal stack transformation.
    Faulted,
}

/// Numeric and source facts for one normal instruction definition in the
/// current receipt. Stack aliases and deferred dispatch results have no record.
/// Keeping only these layers avoids retaining symbolic terms or machine states.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EvmInstructionValue<N: WordElement> {
    instruction_index: usize,
    numeric: N,
    provenance: Provenance,
}

impl<N: WordElement> EvmInstructionValue<N> {
    pub(crate) fn new(instruction_index: usize, value: &EvmValue<N>) -> Self {
        Self {
            instruction_index,
            numeric: value.numeric().clone(),
            provenance: value.provenance().clone(),
        }
    }

    /// Index into the receipt's instruction phases and the state's visited PCs.
    pub fn instruction_index(&self) -> usize {
        self.instruction_index
    }

    /// Constraints observed for the normal result, independently of its source.
    pub fn numeric(&self) -> &N {
        &self.numeric
    }

    /// Possible source categories and the code-address usage role.
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

/// Current-input execution evidence used by partial SSA. This internal receipt
/// does not change the existing analysis JSON's numeric or graph contracts.
#[derive(Clone, Debug)]
pub struct EvmExecutionEvidence<N: WordElement> {
    pub(crate) current: bool,
    pub(crate) instructions: Vec<InstructionProgress>,
    /// Sparse, ordered normal definitions. Summary certificates share these
    /// immutable projections; no fresh-expression renaming is required.
    pub(crate) instruction_values: Arc<[EvmInstructionValue<N>]>,
    pub(crate) successors: Vec<(usize, MachineEdgeKind)>,
}

impl<N: WordElement> EvmExecutionEvidence<N> {
    pub(crate) fn new(
        instructions: Vec<InstructionProgress>,
        instruction_values: impl Into<Arc<[EvmInstructionValue<N>]>>,
    ) -> Self {
        Self {
            current: true,
            instructions,
            instruction_values: instruction_values.into(),
            successors: Vec::new(),
        }
    }

    /// Whether the receipt describes this state's current joined entry.
    pub fn is_current(&self) -> bool {
        self.current
    }

    /// Exactly one phase per visited offset in `MachineState::executed_pcs`.
    pub fn instructions(&self) -> &[InstructionProgress] {
        &self.instructions
    }

    /// Sparse normal-result observations in instruction order. A changed entry
    /// makes every old observation unavailable until the state executes again.
    pub fn instruction_values(&self) -> Option<&[EvmInstructionValue<N>]> {
        self.current.then_some(self.instruction_values.as_ref())
    }

    /// Graph successors installed by this receipt, with native state IDs.
    /// Older accumulated edges are not evidence for the latest prefix.
    pub fn successors(&self) -> &[(usize, MachineEdgeKind)] {
        &self.successors
    }
}

#[cfg(test)]
mod tests;

// Local regression fixtures use explicit built-in carriers.
#[cfg(test)]
pub(in crate::analysis) type InstructionValue = EvmInstructionValue<crate::domain::NumericValue>;
#[cfg(test)]
pub(in crate::analysis) type ExecutionEvidence = EvmExecutionEvidence<crate::domain::NumericValue>;

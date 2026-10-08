//! Receipt of the latest block transfer, separate from accumulated graph edges.
//! A changed entry invalidates the receipt until that entry is executed again.

use super::MachineEdgeKind;
use serde::Serialize;

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

/// Current-input execution evidence used by partial SSA. This internal receipt
/// does not change the existing analysis JSON's numeric or graph contracts.
#[derive(Clone, Debug)]
pub struct ExecutionEvidence {
    pub(crate) current: bool,
    pub(crate) instructions: Vec<InstructionProgress>,
    pub(crate) successors: Vec<(usize, MachineEdgeKind)>,
}

impl ExecutionEvidence {
    pub(crate) fn new(instructions: Vec<InstructionProgress>) -> Self {
        Self {
            current: true,
            instructions,
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

    /// Graph successors installed by this receipt, with native state IDs.
    /// Older accumulated edges are not evidence for the latest prefix.
    pub fn successors(&self) -> &[(usize, MachineEdgeKind)] {
        &self.successors
    }
}

//! Shared presentation rules for SSA tables and native CFG previews.
//! The immutable instruction receipts and their value/effect identities remain
//! intact; only completed stack bookkeeping is optional in the reading view.

use evm_abstract_protocol::{BlockCoverage, InstructionProgress, SsaInstruction};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SsaDisplay {
    #[default]
    Compact,
    Full,
}

impl SsaDisplay {
    /// A selected original PC remains reachable even in the compact view.
    pub(crate) fn instruction_visible(
        self,
        coverage: BlockCoverage,
        instruction: &SsaInstruction,
        selected: bool,
    ) -> bool {
        self == Self::Full
            || selected
            || coverage != BlockCoverage::Current
            || instruction.fault
            || instruction.progress != InstructionProgress::Completed
            // POP, DUP1..16 and SWAP1..16 only rearrange existing value names.
            // JUMPDEST, value definitions, effects and control flow stay visible.
            || !matches!(instruction.opcode, 0x50 | 0x80..=0x8f | 0x90..=0x9f)
    }

    pub(crate) fn show_effect(self, instruction: &SsaInstruction) -> bool {
        self == Self::Full
            || instruction.fault
            || instruction.progress != InstructionProgress::Completed
    }

    pub(crate) fn show_completed(self) -> bool {
        self == Self::Full
    }
}

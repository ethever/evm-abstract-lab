//! Shared presentation rules for SSA tables and native CFG previews.
//! The immutable instruction receipts and their value/effect identities remain
//! intact; only completed stack bookkeeping is optional in the reading view.

use evm_abstract_protocol::{
    BlockCoverage, InstructionProgress, SsaInstruction, SsaOperand, SsaOperation,
};

#[derive(Clone, Copy)]
pub(crate) enum DisplayOperand<'a> {
    Value(usize),
    Constant(&'a str),
}

impl DisplayOperand<'_> {
    pub(crate) fn text(self) -> String {
        match self {
            Self::Value(value) => format!("%{value}"),
            Self::Constant(word) => word.into(),
        }
    }
}

fn operand(operand: &SsaOperand) -> DisplayOperand<'_> {
    match operand {
        SsaOperand::Value(value) => DisplayOperand::Value(*value),
        SsaOperand::Constant(word) => DisplayOperand::Constant(word),
    }
}

/// Both SSA views consume the operation already normalized by the core.
pub(crate) fn operation_parts(operation: &SsaOperation) -> (&str, Vec<DisplayOperand<'_>>) {
    match operation {
        SsaOperation::Opcode(operation) => (
            &operation.name,
            operation.operands.iter().map(operand).collect(),
        ),
        SsaOperation::Constant(word) => ("CONST", vec![DisplayOperand::Constant(word)]),
        SsaOperation::Copy(source) => ("COPY", vec![operand(source)]),
    }
}

pub(crate) fn operation_text(operation: &SsaOperation) -> String {
    let (name, operands) = operation_parts(operation);
    let arguments = operands
        .into_iter()
        .map(|operand| format!(" {}", operand.text()))
        .collect::<String>();
    format!("{name}{arguments}")
}

pub(crate) fn source_text(instruction: &SsaInstruction) -> String {
    format!(
        "Original opcode: 0x{:02x} {}{}{}",
        instruction.opcode,
        instruction.name,
        instruction
            .immediate
            .as_ref()
            .map_or(String::new(), |word| format!(" {word}")),
        instruction
            .operands
            .iter()
            .map(|value| format!(" %{value}"))
            .collect::<String>(),
    )
}

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

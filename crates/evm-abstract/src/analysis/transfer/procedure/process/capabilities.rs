//! Paid source inventory, not an execution or reachability certificate.

use crate::{
    analysis::{MachineError, ProcessAction},
    bytecode::Program,
    resource::{WorkBudget, clone_payment::pay},
};
use revm_bytecode::opcode;

#[derive(Clone, Copy)]
pub(in crate::analysis) struct Capabilities {
    mask: u8,
    pub effects: bool,
    pub returns: bool,
    pub reverts: bool,
}

pub(super) const ACTIONS: [ProcessAction; 6] = [
    ProcessAction::Call,
    ProcessAction::StaticCall,
    ProcessAction::DelegateCall,
    ProcessAction::CallCode,
    ProcessAction::Create,
    ProcessAction::Create2,
];

impl Capabilities {
    pub(in crate::analysis) const fn unknown() -> Self {
        Self {
            mask: 0x3f,
            effects: true,
            returns: true,
            reverts: true,
        }
    }

    pub(super) fn actions(self) -> impl Iterator<Item = ProcessAction> {
        ACTIONS
            .into_iter()
            .enumerate()
            .filter_map(move |(index, action)| (self.mask & (1 << index) != 0).then_some(action))
    }

    pub(in crate::analysis) fn scan(
        program: &Program,
        budget: &mut WorkBudget,
    ) -> Result<Self, MachineError> {
        // Byte length covers decoding inventory, opcode tests and retained mask.
        // PUSH immediate bytes are absent from this decoded instruction stream.
        pay(
            budget,
            program.byte_len().saturating_mul(4).saturating_add(8),
        )?;
        let mut out = Self {
            mask: 0,
            effects: false,
            returns: false,
            reverts: false,
        };
        let mut last = None;
        for instruction in program
            .blocks()
            .iter()
            .flat_map(|block| &block.instructions)
        {
            MachineError::check_cancelled()?;
            last = Some(instruction);
            if !instruction.is_valid() {
                continue;
            }
            let op = instruction.opcode;
            for (index, expected) in [
                opcode::CALL,
                opcode::STATICCALL,
                opcode::DELEGATECALL,
                opcode::CALLCODE,
                opcode::CREATE,
                opcode::CREATE2,
            ]
            .into_iter()
            .enumerate()
            {
                if op == expected {
                    out.mask |= 1 << index;
                }
            }
            out.effects |= matches!(
                op,
                opcode::SSTORE | opcode::TSTORE | opcode::SELFDESTRUCT | opcode::LOG0
                    ..=opcode::LOG4
            );
            out.returns |= matches!(op, opcode::STOP | opcode::RETURN | opcode::SELFDESTRUCT);
            out.reverts |= op == opcode::REVERT;
        }
        // End-of-code is a successful empty return. A final unconditional
        // transfer/exit cannot fall through; earlier explicit exits remain may.
        out.returns |= last.is_none_or(|instruction| {
            instruction.is_valid()
                && !matches!(
                    instruction.opcode,
                    opcode::JUMP
                        | opcode::STOP
                        | opcode::RETURN
                        | opcode::REVERT
                        | opcode::SELFDESTRUCT
                        | opcode::INVALID
                )
        });
        out.effects |= out.mask != 0;
        MachineError::check_cancelled()?;
        Ok(out)
    }
}

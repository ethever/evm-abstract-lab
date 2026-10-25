//! Primitive rules for an admitted MAY control step, without Native PC evidence.

use crate::analysis::ProcessAction;
use revm_bytecode::opcode;

#[derive(Clone, Copy)]
pub(in crate::analysis) struct Capabilities {
    mask: u8,
    pub effects: bool,
    /// Every state-writing primitive is absent; STATICCALL may still touch.
    pub readonly: bool,
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
    pub(in crate::analysis) fn boundary(code: crate::analysis::FrameCode) -> Self {
        Self {
            mask: 0,
            effects: false,
            readonly: true,
            returns: matches!(
                code,
                crate::analysis::FrameCode::Empty | crate::analysis::FrameCode::Precompile(_)
            ),
            reverts: false,
        }
    }
    pub(in crate::analysis) const fn unknown() -> Self {
        Self {
            mask: 0x3f,
            effects: true,
            readonly: false,
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

    pub(in crate::analysis) const fn empty() -> Self {
        Self {
            mask: 0,
            effects: false,
            readonly: true,
            returns: false,
            reverts: false,
        }
    }

    pub(super) fn observe(&mut self, op: u8) {
        self.readonly &= !matches!(
            op,
            opcode::SSTORE | opcode::TSTORE | opcode::SELFDESTRUCT | opcode::LOG0
                ..=opcode::LOG4
                    | opcode::CALL
                    | opcode::CALLCODE
                    | opcode::DELEGATECALL
                    | opcode::CREATE
                    | opcode::CREATE2
        );
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
                self.mask |= 1 << index;
            }
        }
        self.effects |= matches!(
            op,
            opcode::SSTORE | opcode::TSTORE | opcode::SELFDESTRUCT | opcode::LOG0..=opcode::LOG4
        );
        self.returns |= matches!(op, opcode::STOP | opcode::RETURN | opcode::SELFDESTRUCT);
        self.reverts |= op == opcode::REVERT;
        self.effects |= self.mask != 0;
    }
}

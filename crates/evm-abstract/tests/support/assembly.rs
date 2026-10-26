//! Mechanical bytecode assembly only. Fixtures retain their own call, memory,
//! and exit conventions; neither executor's EVM semantics belong here.
#![allow(dead_code)]

use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Assembly {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assembled {
    pub bytes: Vec<u8>,
    pub labels: BTreeMap<&'static str, usize>,
}

impl Assembly {
    pub fn op(&mut self, op: u8) -> &mut Self {
        self.bytes.push(op);
        self
    }

    // Always PUSH2: changing to a minimal PUSH would move fixture source PCs.
    pub fn push(&mut self, value: u16) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.bytes.extend(value.to_be_bytes());
        self
    }

    pub fn push_word(&mut self, value: U256) -> &mut Self {
        self.bytes.push(opcode::PUSH32);
        self.bytes.extend(value.to_be_bytes::<32>());
        self
    }

    pub fn label(&mut self, name: &'static str) -> &mut Self {
        self.data(name, &[opcode::JUMPDEST])
    }

    pub fn data(&mut self, name: &'static str, bytes: &[u8]) -> &mut Self {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.bytes.extend(bytes);
        self
    }

    pub fn push_label(&mut self, name: &'static str) -> &mut Self {
        self.bytes.push(opcode::PUSH2);
        self.fixups.push((self.bytes.len(), name));
        self.bytes.extend([0, 0]);
        self
    }

    pub fn jump(&mut self, name: &'static str, conditional: bool) -> &mut Self {
        self.push_label(name).op(if conditional {
            opcode::JUMPI
        } else {
            opcode::JUMP
        })
    }

    pub fn finish(mut self) -> Assembled {
        for (offset, label) in self.fixups {
            let pc = u16::try_from(self.labels[label]).expect("fixture label exceeds PUSH2");
            self.bytes[offset..offset + 2].copy_from_slice(&pc.to_be_bytes());
        }
        Assembled {
            bytes: self.bytes,
            labels: self.labels,
        }
    }
}

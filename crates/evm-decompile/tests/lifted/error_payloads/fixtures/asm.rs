//! Small relocatable bytecode assembler shared by independent test fixtures.
use super::{Fork, Input, U256};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    pub(crate) fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
        self
    }

    pub(crate) fn number(&mut self, value: u64) -> &mut Self {
        self.word(U256::from(value))
    }

    pub(crate) fn word(&mut self, value: U256) -> &mut Self {
        let bytes = value.to_be_bytes::<32>();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(31);
        self.bytes.push(0x5f + u8::try_from(32 - first).unwrap());
        self.bytes.extend_from_slice(&bytes[first..]);
        self
    }

    pub(crate) fn target(&mut self, label: &'static str) -> &mut Self {
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self
    }

    pub(crate) fn dest(&mut self, label: &'static str) -> &mut Self {
        self.labels.insert(label, self.bytes.len());
        self.op(0x5b)
    }

    pub(crate) fn data(&mut self, label: &'static str, bytes: &[u8]) {
        self.labels.insert(label, self.bytes.len());
        self.bytes.extend_from_slice(bytes);
    }

    pub(crate) fn append(&mut self, mut fragment: Self) {
        let base = self.bytes.len();
        for (offset, label) in fragment.fixups {
            fragment.bytes[offset..offset + 2].copy_from_slice(
                &u16::try_from(base + fragment.labels[label])
                    .unwrap()
                    .to_be_bytes(),
            );
        }
        self.bytes.extend(fragment.bytes);
    }

    pub(crate) fn finish(mut self) -> Input {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}

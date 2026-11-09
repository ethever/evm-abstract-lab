//! Small bytecode inputs with visible branch, loop and repeated-call behavior.
use evm_decompile::{ExternalResponse, Fork, Input, MemoryHost, U256};
use std::collections::BTreeMap;

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    jumps: Vec<(usize, &'static str)>,
}
impl Asm {
    fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    fn label(&mut self, label: &'static str) {
        self.labels.insert(label, self.bytes.len());
        self.emit(&[0x5b]);
    }
    fn jump(&mut self, label: &'static str, conditional: bool) {
        self.emit(&[0x60, 0, if conditional { 0x57 } else { 0x56 }]);
        self.jumps.push((self.bytes.len() - 2, label));
    }
    fn finish(mut self) -> Input {
        for (offset, label) in self.jumps {
            self.bytes[offset] = u8::try_from(self.labels[label]).unwrap();
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}

pub(super) fn conditional_tail() -> Input {
    let mut a = Asm::default();
    a.emit(&[0x5f, 0x35]);
    a.jump("true", true);
    a.emit(&[0x60, 2]);
    a.jump("tail", false);
    a.label("true");
    a.emit(&[0x60, 0x20, 0x35]);
    a.jump("failure", true);
    a.emit(&[0x60, 1]);
    a.jump("tail", false);
    a.label("failure");
    a.emit(&[0x60, 0x99, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xfd]);
    a.label("tail");
    a.emit(&[0x5f, 0x55, 0x60, 0x2a, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
    a.finish()
}

pub(super) fn countdown() -> Input {
    let mut a = Asm::default();
    a.emit(&[0x5f, 0x35, 0x5f, 0x55]);
    a.label("loop");
    a.emit(&[0x5f, 0x54, 0x15]);
    a.jump("end", true);
    a.emit(&[0x5f, 0x54, 0x60, 1, 0x90, 0x03, 0x5f, 0x55]);
    a.jump("loop", false);
    a.label("end");
    a.emit(&[0x00]);
    a.finish()
}

pub(super) fn host(input: &Input, values: &[u64]) -> MemoryHost {
    MemoryHost::new(
        input,
        values
            .iter()
            .flat_map(|n| U256::from(*n).to_be_bytes::<32>())
            .collect(),
    )
}

pub(super) fn sequential() -> Input {
    Input::new(
        evm_bytecode::parse_hex(include_str!("../fixtures/shared-procedures/sequential.hex"))
            .unwrap(),
        Fork::Osaka,
    )
}

pub(super) fn repeated_squares() -> Input {
    let mut a = Asm::default();
    for offset in [0, 32] {
        a.emit(&[0x60, offset, 0x35]);
        for _ in 0..8 {
            a.emit(&[0x80, 0x02]);
        }
        a.emit(&[0x60, offset, 0x52]);
    }
    a.emit(&[0x60, 64, 0x5f, 0xf3]);
    a.finish()
}

pub(super) fn indirect_shared_suffix() -> Input {
    let mut bytes = vec![0x5f, 0x35, 0x60, 1, 0x16];
    let gap = bytes.len() + 1;
    bytes.extend([0x61, 0, 0, 0x02]);
    let base = bytes.len() + 1;
    bytes.extend([0x61, 0, 0, 0x01, 0x56]);
    let first = bytes.len();
    bytes.extend([0x5b, 0x61, 0, 0, 0x56]);
    let second = bytes.len();
    bytes.extend([0x5b, 0x61, 0, 0, 0x56]);
    let tail = bytes.len();
    bytes.extend(std::iter::repeat_n(0x5b, 64));
    bytes.extend([0x60, 7, 0x5f, 0x55, 0x00]);
    for (offset, value) in [
        (gap, second - first),
        (base, first),
        (first + 2, tail),
        (second + 2, tail),
    ] {
        bytes[offset..offset + 2].copy_from_slice(&u16::try_from(value).unwrap().to_be_bytes());
    }
    Input::new(bytes, Fork::Osaka)
}

pub(super) fn sequential_host(input: &Input, values: &[u64]) -> MemoryHost {
    let mut host = host(input, values);
    for value in [11, 19] {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: U256::from(value).to_be_bytes::<32>().to_vec(),
            ..ExternalResponse::default()
        });
    }
    host
}

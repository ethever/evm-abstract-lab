use evm_decompile::{Fork, Input};
use std::collections::BTreeMap;

pub(super) struct Fixture {
    pub input: Input,
    pub join: usize,
}

pub(super) fn loop_diamond(terminal: u8) -> Fixture {
    let mut bytes = evm_bytecode::parse_hex(include_str!(
        "../fixtures/structure/regions/loop-normal-join.hex"
    ))
    .unwrap();
    assert_eq!(bytes.pop(), Some(0xfd));
    bytes.push(terminal);
    let metadata: serde_json::Value = serde_json::from_str(include_str!(
        "../fixtures/structure/regions/loop-normal-join.json"
    ))
    .unwrap();
    Fixture {
        input: Input::new(bytes, Fork::Osaka),
        join: metadata["labels"]["join"].as_u64().unwrap() as usize,
    }
}

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
    fn label(&mut self, name: &'static str) {
        self.labels.insert(name, self.bytes.len());
        self.emit(&[0x5b]);
    }
    fn jump(&mut self, name: &'static str, conditional: bool) {
        self.emit(&[0x61, 0, 0, if conditional { 0x57 } else { 0x56 }]);
        self.jumps.push((self.bytes.len() - 3, name));
    }
    fn load(&mut self, offset: u8) {
        self.emit(&[0x60, offset, 0x35]);
    }
    fn finish(mut self) -> Fixture {
        for (offset, label) in self.jumps {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        Fixture {
            input: Input::new(self.bytes, Fork::Osaka),
            join: self.labels["join"],
        }
    }
}

pub(super) fn blocked_loop() -> Fixture {
    let mut a = Asm::default();
    a.load(0);
    a.label("header");
    a.emit(&[0x80, 0x15]);
    a.jump("done", true);
    a.load(32);
    a.jump("a", true);
    a.jump("b", false);
    // Both A and B have outside entries, and their true edges form a cycle.
    // The 1/1 input never reaches the otherwise shared normal continuation.
    a.label("a");
    a.emit(&[0x60, 7, 0x60, 9, 0x55]);
    a.load(64);
    a.jump("b", true);
    a.jump("join", false);
    a.label("b");
    a.emit(&[0x60, 8, 0x60, 10, 0x55]);
    a.load(96);
    a.jump("a", true);
    a.jump("join", false);
    a.label("join");
    a.emit(&[0x80, 0x5f, 0x55, 0x5f, 0x5f, 0xa0, 0x60, 1, 0x90, 0x03]);
    a.jump("header", false);
    a.label("done");
    a.emit(&[0x50, 0x00]);
    a.finish()
}

pub(super) fn nested_loops() -> Fixture {
    let mut a = Asm::default();
    a.load(0);
    a.label("outer");
    a.emit(&[0x80, 0x15]);
    a.jump("done", true);
    a.load(32);
    a.label("inner");
    a.emit(&[0x80, 0x15]);
    a.jump("inner_done", true);
    a.load(64);
    a.jump("left", true);
    a.jump("join", false);
    a.label("left");
    a.emit(&[0x80]);
    a.load(96);
    a.emit(&[0x14]);
    a.jump("failure", true);
    a.jump("join", false);
    a.label("join");
    a.emit(&[
        0x80, 0x60, 3, 0x55, 0x80, 0x5f, 0x52, 0x60, 0x20, 0x5f, 0xa0,
    ]);
    a.emit(&[0x60, 1, 0x90, 0x03]);
    a.jump("inner", false);
    a.label("inner_done");
    a.emit(&[0x50, 0x80, 0x60, 4, 0x55, 0x60, 1, 0x90, 0x03]);
    a.jump("outer", false);
    a.label("done");
    a.emit(&[0x50, 0x00]);
    a.label("failure");
    a.emit(&[0x5f, 0x5f, 0xfd]);
    a.finish()
}

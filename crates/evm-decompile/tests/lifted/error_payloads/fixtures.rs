use evm_decompile::{Fork, Input, U256};
use std::collections::BTreeMap;

pub(super) const DS_MATH: &[u8] = b"ds-math-mul-overflow";
pub(super) const TRANSFER: &[u8] = b"TransferHelper: TRANSFER_FAILED";

#[derive(Default)]
pub(super) struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    pub(super) fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
        self
    }

    pub(super) fn number(&mut self, value: u64) -> &mut Self {
        self.word(U256::from(value))
    }

    pub(super) fn word(&mut self, value: U256) -> &mut Self {
        let bytes = value.to_be_bytes::<32>();
        let first = bytes.iter().position(|b| *b != 0).unwrap_or(31);
        self.bytes.push(0x5f + u8::try_from(32 - first).unwrap());
        self.bytes.extend_from_slice(&bytes[first..]);
        self
    }

    pub(super) fn target(&mut self, label: &'static str) -> &mut Self {
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self
    }

    pub(super) fn dest(&mut self, label: &'static str) -> &mut Self {
        self.labels.insert(label, self.bytes.len());
        self.op(0x5b)
    }

    fn data(&mut self, label: &'static str, bytes: &[u8]) {
        self.labels.insert(label, self.bytes.len());
        self.bytes.extend_from_slice(bytes);
    }

    pub(super) fn finish(mut self) -> Input {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        Input::new(self.bytes, Fork::Osaka)
    }
}

pub(super) fn initialize(asm: &mut Asm, pointer: u16) {
    asm.number(u64::from(pointer)).number(64).op(0x52);
}

// The stack retains p through the overlapping selector / offset writes. q is
// deliberately loaded separately so accepting it requires real memory facts.
pub(super) fn begin(asm: &mut Asm, message: &[u8]) {
    asm.number(64).op(0x51);
    relative(asm, 0, U256::from(0x08c379a0_u64) << 224);
    relative(asm, 4, U256::from(32));
    relative(asm, 36, U256::from(message.len()));
    for (i, chunk) in message.chunks(32).enumerate() {
        let mut word = [0; 32];
        word[..chunk.len()].copy_from_slice(chunk);
        relative(asm, 68 + i * 32, U256::from_be_bytes(word));
    }
}

pub(super) fn relative(asm: &mut Asm, offset: usize, word: U256) {
    asm.word(word).op(0x81);
    if offset != 0 {
        asm.number(u64::try_from(offset).unwrap()).op(0x01);
    }
    asm.op(0x52);
}

pub(super) fn end(asm: &mut Asm, message_len: usize) {
    asm.number(u64::try_from(68 + message_len.div_ceil(32) * 32).unwrap())
        .op(0x01)
        .number(64)
        .op(0x51)
        .op(0x80)
        .op(0x91)
        .op(0x03)
        .op(0x90)
        .op(0xfd);
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Mutation {
    None,
    NonzeroPadding,
    Offset,
    Length,
    UnknownWord,
    UnknownStore,
    ByteOverlap,
    Pointer(u16),
    DisjointStore,
    Copy { opcode: u8, destination: u16 },
    UnknownCopy(u8),
    CallOutput { destination: u16, size: u8 },
    UnknownCallOutput,
    CallVariant { opcode: u8, destination: u16 },
}

pub(super) fn mutate(asm: &mut Asm, mutation: Mutation, message_len: usize) {
    match mutation {
        Mutation::None => {}
        Mutation::NonzeroPadding => {
            asm.number(1)
                .op(0x81)
                .number(u64::try_from(68 + message_len).unwrap())
                .op(0x01)
                .op(0x53);
        }
        Mutation::Offset => relative(asm, 4, U256::from(64)),
        Mutation::Length => relative(asm, 36, U256::from(message_len + 32)),
        Mutation::UnknownWord => {
            asm.number(32)
                .op(0x35)
                .op(0x81)
                .number(68)
                .op(0x01)
                .op(0x52);
        }
        Mutation::UnknownStore => {
            asm.number(0).number(32).op(0x35).op(0x52);
        }
        Mutation::ByteOverlap => {
            asm.number(0xff).op(0x81).number(69).op(0x01).op(0x53);
        }
        Mutation::Pointer(pointer) => initialize(asm, pointer),
        Mutation::DisjointStore => {
            asm.number(0x42).number(0x300).op(0x52);
        }
        Mutation::Copy {
            opcode,
            destination,
        } => {
            asm.number(32)
                .number(0)
                .number(u64::from(destination))
                .op(opcode);
        }
        Mutation::UnknownCopy(opcode) => {
            asm.number(32).number(0).number(32).op(0x35).op(opcode);
        }
        Mutation::CallOutput { destination, size } => {
            call(asm, Some(destination), size);
        }
        Mutation::UnknownCallOutput => call(asm, None, 32),
        Mutation::CallVariant {
            opcode,
            destination,
        } => call_variant(asm, opcode, Some(destination), 32),
    }
}

pub(super) fn call(asm: &mut Asm, destination: Option<u16>, size: u8) {
    call_variant(asm, 0xf1, destination, size);
}

fn call_variant(asm: &mut Asm, opcode: u8, destination: Option<u16>, size: u8) {
    asm.number(u64::from(size));
    if let Some(destination) = destination {
        asm.number(u64::from(destination));
    } else {
        asm.number(32).op(0x35);
    }
    for _ in 0..if matches!(opcode, 0xf1 | 0xf2) { 4 } else { 3 } {
        asm.number(0);
    }
    asm.number(0xff).op(opcode).op(0x50);
}

pub(super) fn standalone(message: &[u8], mutation: Mutation) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    begin(&mut asm, message);
    mutate(&mut asm, mutation, message.len());
    end(&mut asm, message.len());
    asm.finish()
}

pub(super) fn success(asm: &mut Asm) {
    asm.number(0x55).number(0).op(0x55).op(0x00);
}

pub(super) fn guarded(message: &[u8], failure_nonzero: bool, observation: Option<u8>) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0).op(0x35).target("taken").op(0x57);
    if failure_nonzero {
        success(&mut asm);
    } else {
        failure(&mut asm, message, observation);
    }
    asm.dest("taken");
    if failure_nonzero {
        failure(&mut asm, message, observation);
    } else {
        success(&mut asm);
    }
    asm.finish()
}

fn failure(asm: &mut Asm, message: &[u8], observation: Option<u8>) {
    if let Some(opcode) = observation {
        match opcode {
            0x55 => {
                asm.number(0x99).number(9).op(0x55);
            }
            0xf1 => call(asm, Some(0x300), 0),
            0x54 => {
                asm.number(9).op(0x54).op(0x50);
            }
            0x51 => {
                asm.word(U256::MAX).op(0x51).op(0x50);
            }
            0x52 => {
                asm.number(0x42).number(0x300).op(0x52);
            }
            0x33 => {
                asm.op(0x33).op(0x50);
            }
            _ => panic!("unsupported fixture observation"),
        }
    }
    begin(asm, message);
    end(asm, message.len());
}

pub(super) fn predecessor_pointer(both_initialize: bool) -> Input {
    let mut asm = Asm::default();
    asm.number(0).op(0x35).target("other").op(0x57);
    initialize(&mut asm, 128);
    asm.target("joined").op(0x56).dest("other");
    if both_initialize {
        initialize(&mut asm, 160);
    }
    asm.target("joined").op(0x56).dest("joined");
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

pub(super) fn equal_ranges_distinct_pointers() -> Input {
    let mut asm = Asm::default();
    asm.number(0).op(0x35).target("first_other").op(0x57);
    initialize(&mut asm, 128);
    asm.target("first_join").op(0x56).dest("first_other");
    initialize(&mut asm, 160);
    asm.target("first_join").op(0x56).dest("first_join");
    begin(&mut asm, DS_MATH);
    asm.number(32).op(0x35).target("second_other").op(0x57);
    initialize(&mut asm, 128);
    asm.target("second_join").op(0x56).dest("second_other");
    initialize(&mut asm, 160);
    asm.target("second_join").op(0x56).dest("second_join");
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

pub(super) fn two_errors() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(32)
        .op(0x35)
        .number(0)
        .op(0x35)
        .op(0x80)
        .target("pass")
        .op(0x57)
        .op(0x81)
        .target("second")
        .op(0x57);
    failure(&mut asm, DS_MATH, None);
    asm.dest("second");
    failure(&mut asm, TRANSFER, None);
    asm.dest("pass");
    success(&mut asm);
    asm.finish()
}

pub(super) fn shared_join(nested: bool) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .op(0x35)
        .target("store_or_inner")
        .op(0x57)
        .target("tail")
        .op(0x56)
        .dest("store_or_inner");
    if nested {
        asm.number(32)
            .op(0x35)
            .target("store")
            .op(0x57)
            .target("tail")
            .op(0x56)
            .dest("store");
    }
    asm.number(0x55)
        .number(0)
        .op(0x55)
        .target("tail")
        .op(0x56)
        .dest("tail");
    failure(&mut asm, DS_MATH, None);
    asm.finish()
}

pub(super) fn forwarded() -> Input {
    let mut asm = Asm::default();
    asm.op(0x3d)
        .number(0)
        .number(0)
        .op(0x3e)
        .op(0x3d)
        .number(0)
        .op(0xfd);
    asm.finish()
}

pub(super) fn codecopy_gap() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    // A preceding nonzero word makes the three uncopied padding bytes known
    // incorrect, independently of any assumption about fresh memory.
    asm.word(U256::MAX).number(196).op(0x52);
    asm.number(64).op(0x51);
    relative(&mut asm, 0, U256::from(0x08c379a0_u64) << 224);
    relative(&mut asm, 4, U256::from(32));
    relative(&mut asm, 36, U256::from(29));
    asm.number(29)
        .target("data")
        .op(0x82)
        .number(68)
        .op(0x01)
        .op(0x39);
    end(&mut asm, 29);
    // Copy exactly 29 printable bytes and leave stale padding untouched.
    asm.data("data", b"12345678901234567890123456789");
    asm.finish()
}

pub(super) fn bounded_pointer_update() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .op(0x35)
        .number(31)
        .op(0x16)
        .number(64)
        .op(0x51)
        .op(0x01)
        .number(64)
        .op(0x52);
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

pub(super) fn loop_pointer(unknown_alias: bool) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .dest("head")
        .op(0x80)
        .number(2)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50);
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.dest("body").number(0x42);
    if unknown_alias {
        asm.number(0).op(0x35);
    } else {
        asm.number(512);
    }
    asm.op(0x52).number(1).op(0x01).target("head").op(0x56);
    asm.finish()
}

pub(super) fn partial_pass() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0).op(0x35).target("pass").op(0x57);
    // Close the payload proof inside the failed block itself. An unresolved
    // success route supplies no unconditional predecessor-memory premises.
    initialize(&mut asm, 128);
    failure(&mut asm, DS_MATH, None);
    asm.dest("pass").number(32).op(0x35).op(0x56);
    asm.finish()
}

pub(super) fn same_error_different_constructors() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(32)
        .op(0x35)
        .number(0)
        .op(0x35)
        .op(0x80)
        .target("pass")
        .op(0x57)
        .op(0x81)
        .target("large")
        .op(0x57);
    failure(&mut asm, DS_MATH, None);
    asm.dest("large");
    initialize(&mut asm, 4096);
    failure(&mut asm, DS_MATH, None);
    asm.dest("pass");
    success(&mut asm);
    asm.finish()
}

pub(super) fn interleaved_unrelated_memory(read: bool) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0).op(0x35).target("pass").op(0x57);
    begin(&mut asm, DS_MATH);
    if read {
        asm.number(512).op(0x51).op(0x50);
    } else {
        mutate(&mut asm, Mutation::DisjointStore, DS_MATH.len());
    }
    end(&mut asm, DS_MATH.len());
    asm.dest("pass");
    success(&mut asm);
    asm.finish()
}

pub(super) fn unknown_width_copy(opcode: u8) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    begin(&mut asm, DS_MATH);
    asm.number(0).op(0x35).number(0).number(0x300).op(opcode);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

pub(super) fn shared_nested_constructor() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    // Both observations precede the outer require. Failed control then uses
    // only captured words and jumps to the one shared constructor block.
    asm.number(32)
        .op(0x35)
        .number(0)
        .op(0x35)
        .op(0x80)
        .target("pass")
        .op(0x57)
        .op(0x81)
        .target("right")
        .op(0x57)
        .target("error")
        .op(0x56)
        .dest("right")
        .target("error")
        .op(0x56)
        .dest("pass");
    success(&mut asm);
    asm.dest("error");
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

pub(super) fn dispatcher() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .op(0x35)
        .target("a")
        .op(0x57)
        .target("b")
        .op(0x56)
        .dest("a")
        .number(32)
        .op(0x35)
        .target("b")
        .op(0x57)
        .target("error")
        .op(0x56)
        .dest("b")
        .number(64)
        .op(0x35)
        .target("a")
        .op(0x57)
        .target("error")
        .op(0x56)
        .dest("error");
    begin(&mut asm, DS_MATH);
    end(&mut asm, DS_MATH.len());
    asm.finish()
}

#[derive(Clone, Copy, Debug)]
pub(super) enum MixedFailure {
    Empty,
    EmptyError,
    Forwarded,
}

pub(super) fn mixed_errors(other: MixedFailure) -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(32)
        .op(0x35)
        .number(0)
        .op(0x35)
        .op(0x80)
        .target("pass")
        .op(0x57)
        .op(0x81)
        .target("other")
        .op(0x57);
    failure(&mut asm, DS_MATH, None);
    asm.dest("other");
    match other {
        MixedFailure::Empty => {
            asm.number(0).number(0).op(0xfd);
        }
        MixedFailure::EmptyError => failure(&mut asm, b"", None),
        MixedFailure::Forwarded => {
            asm.op(0x3d)
                .number(0)
                .number(0)
                .op(0x3e)
                .op(0x3d)
                .number(0)
                .op(0xfd);
        }
    }
    asm.dest("pass");
    success(&mut asm);
    asm.finish()
}

pub(super) fn loop_guard() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(1)
        .number(2)
        .number(0)
        .dest("head")
        .op(0x80)
        .number(3)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .number(0)
        .op(0x55)
        .number(1)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .number(1)
        .op(0x01)
        .op(0x91)
        .op(0x90)
        .op(0x91)
        .number(0)
        .op(0x35)
        .target("head")
        .op(0x57);
    failure(&mut asm, DS_MATH, None);
    asm.finish()
}

pub(super) fn shared_call_join() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .op(0x35)
        .target("call")
        .op(0x57)
        .target("tail")
        .op(0x56)
        .dest("call");
    call(&mut asm, Some(0x300), 32);
    asm.target("tail").op(0x56).dest("tail");
    failure(&mut asm, DS_MATH, None);
    asm.finish()
}

pub(super) fn conditional_pointer_bounds() -> Input {
    let mut asm = Asm::default();
    initialize(&mut asm, 128);
    asm.number(0)
        .op(0x35)
        .op(0x80)
        .number(32)
        .op(0x90)
        .op(0x10)
        .target("bounded")
        .op(0x57);
    // x >= 32 does not establish that 128+x avoids modular wrap.
    asm.number(128).op(0x01).number(64).op(0x52);
    failure(&mut asm, DS_MATH, None);
    asm.dest("bounded").number(128).op(0x01).number(64).op(0x52);
    failure(&mut asm, DS_MATH, None);
    asm.finish()
}

pub(super) fn calldata(words: &[U256]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| word.to_be_bytes::<32>())
        .collect()
}

//! Relocatable programs with live caller values and observable helper effects.
use evm_decompile::{ExternalResponse, Fork, Input, MemoryHost, U256};
use std::collections::BTreeMap;

#[derive(Default)]
struct Asm {
    bytes: Vec<u8>,
    labels: BTreeMap<&'static str, usize>,
    fixups: Vec<(usize, &'static str)>,
}

impl Asm {
    fn op(&mut self, opcode: u8) -> &mut Self {
        self.bytes.push(opcode);
        self
    }

    fn number(&mut self, value: u64) -> &mut Self {
        self.word(U256::from(value))
    }

    fn word(&mut self, value: U256) -> &mut Self {
        let bytes = value.to_be_bytes::<32>();
        let first = bytes.iter().position(|byte| *byte != 0).unwrap_or(31);
        self.bytes.push(0x5f + u8::try_from(32 - first).unwrap());
        self.bytes.extend_from_slice(&bytes[first..]);
        self
    }

    fn target(&mut self, label: &'static str) -> &mut Self {
        self.bytes.push(0x61);
        self.fixups.push((self.bytes.len(), label));
        self.bytes.extend([0, 0]);
        self
    }

    fn dest(&mut self, label: &'static str) -> &mut Self {
        assert!(self.labels.insert(label, self.bytes.len()).is_none());
        self.op(0x5b)
    }

    fn finish(mut self) -> Fixture {
        for (offset, label) in self.fixups {
            self.bytes[offset..offset + 2]
                .copy_from_slice(&u16::try_from(self.labels[label]).unwrap().to_be_bytes());
        }
        Fixture {
            input: Input::new(self.bytes, Fork::Osaka),
            labels: self.labels,
        }
    }
}

pub(super) struct Fixture {
    pub input: Input,
    pub labels: BTreeMap<&'static str, usize>,
}

fn persisted(fixture: Fixture, hex: &str) -> Fixture {
    assert_eq!(
        fixture.input.runtime_bytecode,
        evm_bytecode::parse_hex(hex.trim()).unwrap(),
        "static fixture differs from its relocatable assembler"
    );
    fixture
}

pub(super) fn words(words: &[U256]) -> Vec<u8> {
    words
        .iter()
        .flat_map(|word| word.to_be_bytes::<32>())
        .collect()
}

/// Both output words from the first invocation remain live across the second.
/// The same helper has multiple CFG blocks and two context-specific returns.
pub(super) fn sequential() -> Fixture {
    let mut a = Asm::default();
    a.target("after_first")
        .number(0)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_first")
        .target("after_second")
        .number(32)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_second");
    for offset in [96, 64, 32, 0] {
        a.number(offset).op(0x52);
    }
    a.number(128).number(0).op(0xf3).dest("helper");
    // One counter update per dynamic call, followed by the latest caller word.
    a.number(0)
        .op(0x54)
        .number(1)
        .op(0x01)
        .number(0)
        .op(0x55)
        .op(0x80)
        .number(1)
        .op(0x55)
        .op(0x80)
        .op(0x15)
        .target("failure")
        .op(0x57)
        .op(0x80)
        .number(256)
        .op(0x52)
        .number(32)
        .number(256)
        .op(0xa0)
        .op(0x80)
        .number(1)
        .op(0x16)
        .target("odd")
        .op(0x57)
        .number(3)
        .op(0x01)
        .target("join")
        .op(0x56)
        .dest("odd")
        .number(7)
        .op(0x01)
        .dest("join");
    // Distinct queued return data must feed distinct dynamic output bindings.
    a.number(32)
        .number(320)
        .number(32)
        .number(256)
        .number(0x1234)
        .number(100_000)
        .op(0xfa)
        .op(0x50)
        .number(32)
        .number(0)
        .number(448)
        .op(0x3e)
        .number(320)
        .op(0x51)
        .op(0x01)
        .op(0x80)
        .number(384)
        .op(0x52)
        .number(0)
        .op(0x54)
        .op(0x80)
        .number(2)
        .op(0x55)
        .op(0x91)
        .op(0x56)
        .dest("failure");
    let message = b"shared failure";
    for (offset, value) in [
        (0, U256::from(0x08c379a0_u64) << 224),
        (4, U256::from(32)),
        (36, U256::from(message.len())),
        (68, {
            let mut word = [0; 32];
            word[..message.len()].copy_from_slice(message);
            U256::from_be_bytes(word)
        }),
    ] {
        a.word(value).number(offset).op(0x52);
    }
    a.number(100).number(0).op(0xfd);
    persisted(
        a.finish(),
        include_str!("../fixtures/shared-procedures/sequential.hex"),
    )
}

pub(super) fn sequential_host(input: &Input, first: U256, second: U256) -> MemoryHost {
    let mut host = MemoryHost::new(input, words(&[first, second]));
    for value in [11, 19] {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: words(&[U256::from(value)]),
            ..ExternalResponse::default()
        });
    }
    host
}

pub(super) fn loop_host(input: &Input, counts: [u64; 2]) -> MemoryHost {
    let mut host = MemoryHost::new(input, words(&counts.map(U256::from)));
    for index in 0..counts.into_iter().sum::<u64>() {
        host.responses.push_back(ExternalResponse {
            result: U256::from(1),
            return_data: words(&[U256::from(index + 1)]),
            ..ExternalResponse::default()
        });
    }
    host
}

/// A loop phi is rebound on every iteration and on every helper invocation.
pub(super) fn loop_calls() -> Fixture {
    let mut a = Asm::default();
    a.target("after_first")
        .number(0)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_first")
        .target("after_second")
        .number(32)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_second")
        .number(32)
        .op(0x52)
        .number(0)
        .op(0x52)
        .number(64)
        .number(0)
        .op(0xf3)
        .dest("helper")
        .number(0)
        .dest("loop")
        .op(0x81)
        .op(0x15)
        .target("done")
        .op(0x57)
        .op(0x81)
        .op(0x01)
        .op(0x80)
        .number(64)
        .op(0x52)
        .op(0x81)
        .number(2)
        .op(0x55)
        .number(32)
        .number(64)
        .op(0xa0)
        .number(32)
        .number(320)
        .number(32)
        .number(64)
        .number(0x1234)
        .number(100_000)
        .op(0xfa)
        .op(0x50)
        .op(0x90)
        .number(1)
        .op(0x90)
        .op(0x03)
        .op(0x90)
        .target("loop")
        .op(0x56)
        .dest("done")
        .op(0x90)
        .op(0x50)
        .op(0x90)
        .op(0x56);
    persisted(
        a.finish(),
        include_str!("../fixtures/shared-procedures/loop-calls.hex"),
    )
}

/// The terminal tail is reached from two arms which also have separate early
/// exits. It is therefore materialized twice in the source Structured tree.
pub(super) fn repeated_tail(terminal: u8) -> Fixture {
    let mut a = Asm::default();
    a.number(0)
        .op(0x35)
        .target("left")
        .op(0x57)
        .number(32)
        .op(0x35)
        .target("tail")
        .op(0x57)
        .number(1)
        .number(7)
        .op(0x55)
        .op(0x00)
        .dest("left")
        .number(64)
        .op(0x35)
        .target("tail")
        .op(0x57)
        .number(2)
        .number(7)
        .op(0x55)
        .op(0x00)
        .dest("tail")
        .number(96)
        .op(0x35)
        .op(0x80)
        .number(64)
        .op(0x52)
        .number(5)
        .op(0x55)
        .number(32)
        .number(64)
        .op(0xa0);
    if matches!(terminal, 0xf3 | 0xfd) {
        a.number(32).number(128).op(0x35);
    } else if terminal == 0xff {
        a.number(17);
    }
    a.op(terminal);
    a.finish()
}

/// Caller arithmetic is normally inlined by Lifted liveness. Outlining must
/// print an evaluable argument or materialize its source binding explicitly.
pub(super) fn pure_call_inputs() -> Fixture {
    persisted(
        pure_input_variant(false),
        include_str!("../fixtures/shared-procedures/pure-call-inputs.hex"),
    )
}

pub(super) fn partial_calls() -> Fixture {
    persisted(
        pure_input_variant(true),
        include_str!("../fixtures/shared-procedures/partial-call-inputs.hex"),
    )
}

fn pure_input_variant(partial: bool) -> Fixture {
    let mut a = Asm::default();
    a.target("after_first")
        .number(0)
        .op(0x35)
        .number(7)
        .op(0x01)
        .target("helper")
        .op(0x56)
        .dest("after_first")
        .number(1)
        .op(0x55)
        .number(2)
        .op(0x55)
        .target("after_second")
        .number(32)
        .op(0x35)
        .number(9)
        .op(0x01)
        .target("helper")
        .op(0x56)
        .dest("after_second")
        .number(3)
        .op(0x55)
        .number(4)
        .op(0x55);
    if partial {
        a.number(64).op(0x35).op(0x56);
    } else {
        a.op(0x00);
    }
    a.dest("helper")
        .op(0x80)
        .number(64)
        .op(0x52)
        .op(0x80)
        .number(0)
        .op(0x55)
        .number(32)
        .number(64)
        .op(0xa0)
        .op(0x80)
        .number(96)
        .op(0x52)
        .op(0x80)
        .number(5)
        .op(0x55)
        .target("tail")
        .op(0x56)
        .dest("tail")
        .op(0x80)
        .number(1)
        .op(0x01)
        .op(0x91)
        .op(0x56);
    a.finish()
}

pub(super) fn two_arguments() -> Fixture {
    let mut a = Asm::default();
    a.target("after_first")
        .number(0)
        .op(0x35)
        .number(32)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_first")
        .number(1)
        .op(0x55)
        .target("after_second")
        .number(64)
        .op(0x35)
        .number(96)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_second")
        .number(2)
        .op(0x55)
        .op(0x00)
        .dest("helper")
        .op(0x01)
        .op(0x80)
        .number(64)
        .op(0x52)
        .op(0x80)
        .number(0)
        .op(0x55)
        .op(0x80)
        .number(96)
        .op(0x52)
        .op(0x80)
        .number(3)
        .op(0x55)
        .number(64)
        .number(64)
        .op(0xa0)
        .op(0x90)
        .op(0x56);
    persisted(
        a.finish(),
        include_str!("../fixtures/shared-procedures/two-arguments.hex"),
    )
}

/// The false arm only transfers state to the join; the true arm stores a word.
/// Both invocations return a live value and execute the same useful body.
pub(super) fn token_only_else() -> Fixture {
    conditional_else(false)
}

pub(super) fn observable_else() -> Fixture {
    conditional_else(true)
}

fn conditional_else(observable_else: bool) -> Fixture {
    let mut a = Asm::default();
    a.target("after_first")
        .number(0)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_first")
        .number(1)
        .op(0x55)
        .target("after_second")
        .number(32)
        .op(0x35)
        .target("helper")
        .op(0x56)
        .dest("after_second")
        .number(2)
        .op(0x55)
        .op(0x00)
        .dest("helper")
        .op(0x80)
        .number(64)
        .op(0x52)
        .op(0x80)
        .number(0)
        .op(0x55)
        .op(0x80)
        .target("then")
        .op(0x57);
    if observable_else {
        a.op(0x80).number(5).op(0x55);
    }
    a.target("join")
        .op(0x56)
        .dest("then")
        .op(0x80)
        .number(96)
        .op(0x52)
        .number(32)
        .number(64)
        .op(0xa0)
        .op(0x80)
        .number(3)
        .op(0x55)
        .op(0x80)
        .number(4)
        .op(0x55)
        .dest("join")
        .number(1)
        .op(0x01)
        .op(0x90)
        .op(0x56);
    a.finish()
}

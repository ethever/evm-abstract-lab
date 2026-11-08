use super::{Asm, ExternalResponse, Input, MemoryHost, StructuredKind, StructuredNode, U256};

#[derive(Clone, Copy, Debug)]
pub(super) enum Failure {
    Empty(U256),
    Chain,
    Diamond,
    Payload,
    Panic,
    Error,
    Forwarded,
    Call,
    Sload,
    Mstore,
    WorldRead,
    Stop,
    Return,
    Invalid,
    Unknown,
    BadJump,
    Underflow,
    FaultingMemory,
}

fn word(asm: &mut Asm, value: U256) {
    asm.op(0x7f);
    for byte in value.to_be_bytes::<32>() {
        asm.op(byte);
    }
}

fn empty(asm: &mut Asm) {
    asm.op(0x5f).op(0x5f).op(0xfd);
}

fn success(asm: &mut Asm) {
    asm.push(0x55).op(0x5f).op(0x55).op(0x00);
}

fn body(asm: &mut Asm, failure: Failure) {
    match failure {
        Failure::Empty(offset) => {
            asm.op(0x5f);
            word(asm, offset);
            asm.op(0xfd);
        }
        Failure::Chain => {
            asm.target("chain_a")
                .op(0x56)
                .dest("chain_a")
                .push(3)
                .push(4)
                .op(0x01)
                .op(0x50)
                .target("tail")
                .op(0x56)
                .dest("tail");
            empty(asm);
        }
        Failure::Diamond => {
            asm.op(0x81)
                .target("diamond_a")
                .op(0x57)
                .target("diamond_b")
                .op(0x56)
                .dest("diamond_a")
                .push(3)
                .op(0x50)
                .target("tail")
                .op(0x56)
                .dest("diamond_b")
                .push(4)
                .op(0x50)
                .target("tail")
                .op(0x56)
                .dest("tail");
            empty(asm);
        }
        Failure::Payload => {
            asm.push(0x42).op(0x5f).op(0x53).push(1).op(0x5f).op(0xfd);
        }
        Failure::Panic | Failure::Error => {
            let selector = if matches!(failure, Failure::Panic) {
                0x4e487b71_u64
            } else {
                0x08c379a0_u64
            };
            word(asm, U256::from(selector) << 224);
            asm.op(0x5f).op(0x52);
            if matches!(failure, Failure::Panic) {
                asm.push(0x11).push(4).op(0x52).push(36);
            } else {
                asm.push(32)
                    .push(4)
                    .op(0x52)
                    .op(0x5f)
                    .push(36)
                    .op(0x52)
                    .push(68);
            }
            asm.op(0x5f).op(0xfd);
        }
        Failure::Forwarded => {
            asm.op(0x3d)
                .op(0x5f)
                .op(0x5f)
                .op(0x3e)
                .op(0x3d)
                .op(0x5f)
                .op(0xfd);
        }
        Failure::Call => {
            for _ in 0..7 {
                asm.op(0x5f);
            }
            asm.op(0xf1).op(0x50);
            empty(asm);
        }
        Failure::Sload => {
            asm.push(9).op(0x54).op(0x50);
            empty(asm);
        }
        Failure::Mstore => {
            asm.push(0x42).op(0x5f).op(0x52);
            empty(asm);
        }
        Failure::WorldRead => {
            asm.op(0x33).op(0x50);
            empty(asm);
        }
        Failure::Stop => {
            asm.op(0x00);
        }
        Failure::Return => {
            asm.op(0x5f).op(0x5f).op(0xf3);
        }
        Failure::Invalid => {
            asm.op(0xfe);
        }
        Failure::Unknown => {
            asm.push(32).op(0x35).op(0x56);
        }
        Failure::BadJump => {
            asm.push(1).op(0x56);
        }
        Failure::Underflow => {
            asm.op(0x50).op(0x50).op(0x50);
            empty(asm);
        }
        Failure::FaultingMemory => {
            asm.op(0x3d);
            word(asm, U256::MAX);
            asm.op(0x5f).op(0x3e);
            empty(asm);
        }
    }
}

pub(super) fn direct(failure_nonzero: bool, failure: Failure) -> Input {
    let mut asm = Asm::default();
    if matches!(failure, Failure::Diamond) {
        // Capture the independent inner choice before entering the transparent
        // failure region. Reusing the outer word makes this diamond redundant.
        asm.push(32).op(0x35);
    }
    asm.op(0x5f).op(0x35).op(0x80);
    if failure_nonzero {
        asm.target("failure").op(0x57);
        success(&mut asm);
        asm.dest("failure");
        body(&mut asm, failure);
    } else {
        asm.target("pass").op(0x57);
        body(&mut asm, failure);
        asm.dest("pass");
        success(&mut asm);
    }
    asm.finish()
}

pub(super) fn decoder(all_revert: bool) -> Input {
    let mut asm = Asm::default();
    // Signed bound on a wrapping subtraction. Sizes 0..3 become negative.
    asm.push(64)
        .push(4)
        .op(0x36)
        .op(0x03)
        .op(0x12)
        .target("fail")
        .op(0x57);
    // Canonical bool admission at selector+0.
    asm.push(4)
        .op(0x35)
        .op(0x80)
        .op(0x15)
        .op(0x15)
        .op(0x14)
        .target("bool_pass")
        .op(0x57)
        .target("fail")
        .op(0x56)
        .dest("bool_pass");
    // Dynamic offset and length, keeping cursor and length live on pass edges.
    asm.push(36)
        .op(0x35)
        .push(4)
        .op(0x01)
        .push(32)
        .op(0x81)
        .op(0x36)
        .op(0x03)
        .op(0x12)
        .target("fail")
        .op(0x57)
        .op(0x80)
        .op(0x35)
        .push(32)
        .op(0x82)
        .op(0x01)
        .op(0x36)
        .op(0x03)
        .op(0x81)
        .op(0x11)
        .target("fail")
        .op(0x57)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55);
    for _ in 0..7 {
        asm.op(0x5f);
    }
    asm.op(0xf1).op(0x50);
    if all_revert {
        asm.target("fail").op(0x56);
    } else {
        asm.op(0x00);
    }
    asm.dest("fail");
    empty(&mut asm);
    asm.finish()
}

pub(super) fn decoder_data(size: usize, boolean: U256, offset: U256, length: U256) -> Vec<u8> {
    let mut data = vec![0_u8; size];
    let mut put = |at: usize, word: U256| {
        for (index, byte) in word.to_be_bytes::<32>().into_iter().enumerate() {
            if let Some(slot) = data.get_mut(at + index) {
                *slot = byte;
            }
        }
    };
    put(4, boolean);
    put(36, offset);
    if let Ok(cursor) = usize::try_from(offset.wrapping_add(U256::from(4)))
        && cursor >= 68
    {
        put(cursor, length);
    }
    data
}

pub(super) fn decoder_host(input: &Input, data: Vec<u8>) -> MemoryHost {
    let mut host = MemoryHost::new(input, data);
    host.responses.push_back(ExternalResponse::default());
    host
}

pub(super) fn join_revert() -> Input {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .target("store")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("store")
        .push(0x55)
        .op(0x5f)
        .op(0x55)
        .target("join")
        .op(0x56)
        .dest("join");
    empty(&mut asm);
    asm.finish()
}

pub(super) fn nested_join_revert() -> Input {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .target("inner")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("inner")
        .push(32)
        .op(0x35)
        .target("store")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("store")
        .push(0x55)
        .op(0x5f)
        .op(0x55)
        .target("join")
        .op(0x56)
        .dest("join");
    empty(&mut asm);
    asm.finish()
}

pub(super) fn partial_success() -> Input {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35).target("pass").op(0x57);
    empty(&mut asm);
    asm.dest("pass").push(32).op(0x35).op(0x56);
    asm.finish()
}

pub(super) fn loop_failure(exit: bool) -> Input {
    let mut asm = Asm::default();
    if exit {
        // An independent captured word keeps both the infinite path and the
        // terminating revert path feasible after the outer condition is zero.
        asm.push(32).op(0x35);
    }
    asm.op(0x5f)
        .op(0x35)
        .op(0x80)
        .target("pass")
        .op(0x57)
        .dest("loop");
    if exit {
        asm.op(0x81).target("exit").op(0x57);
    }
    asm.target("loop").op(0x56);
    if exit {
        asm.dest("exit");
        empty(&mut asm);
    }
    asm.dest("pass");
    success(&mut asm);
    asm.finish()
}

pub(super) fn eof_branch(outer: bool) -> Input {
    let mut asm = Asm::default();
    if outer {
        asm.push(32).op(0x35);
    }
    asm.op(0x5f).op(0x35);
    if outer {
        // The last JUMPI consumes the separately captured second word.
        asm.target("pass").op(0x57);
    }
    asm.target("last").op(0x56).dest("fail");
    empty(&mut asm);
    if outer {
        asm.dest("pass");
        success(&mut asm);
    }
    // The absent false edge is a real EOF halt, not an empty successor list.
    asm.dest("last").target("fail").op(0x57);
    asm.finish()
}

pub(super) fn old_capture() -> Input {
    let mut asm = Asm::default();
    asm.push(9)
        .op(0x54)
        .op(0x5f)
        .op(0x35)
        .target("pass")
        .op(0x57);
    empty(&mut asm);
    asm.dest("pass")
        .push(0x77)
        .push(9)
        .op(0x55)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    asm.finish()
}

pub(super) fn loop_phi() -> Input {
    let mut asm = Asm::default();
    asm.push(1)
        .push(2)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .push(3)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .op(0x91)
        .op(0x90)
        .op(0x91)
        .op(0x5f)
        .op(0x35)
        .target("head")
        .op(0x57);
    empty(&mut asm);
    asm.finish()
}

pub(super) fn has_following_if(node: &StructuredNode) -> bool {
    match &node.kind {
        StructuredKind::Sequence(nodes) => {
            nodes
                .windows(2)
                .any(|nodes| matches!(nodes[0].kind, StructuredKind::If { .. }))
                || nodes.iter().any(has_following_if)
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => has_following_if(then_branch) || has_following_if(else_branch),
        StructuredKind::While { header, body, .. } => {
            has_following_if(header) || has_following_if(body)
        }
        _ => false,
    }
}

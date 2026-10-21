use revm_bytecode::opcode;

pub struct Fixture {
    pub code: Vec<u8>,
    pub impossible: usize,
}

pub fn guarded(recursive: bool) -> Fixture {
    let mut code = Vec::new();
    let mut targets = Vec::new();
    if recursive {
        // A growing callee effect forces late output/input generations. Slot0's
        // independent excluded-word invariant must survive the joint closure.
        code.extend([
            opcode::PUSH1,
            2,
            opcode::SLOAD,
            opcode::PUSH1,
            1,
            opcode::ADD,
            opcode::PUSH1,
            2,
            opcode::SSTORE,
        ]);
    }
    code.extend([opcode::PUSH0, opcode::SLOAD]);
    for phase in 0..2 {
        code.extend([opcode::DUP1, opcode::PUSH1, 42, opcode::EQ, opcode::PUSH2]);
        let continuation = code.len();
        code.extend([0, 0, opcode::JUMP]);
        let guard_block = code.len();
        code[continuation..continuation + 2].copy_from_slice(&(guard_block as u16).to_be_bytes());
        code.extend([opcode::JUMPDEST, opcode::PUSH2]);
        targets.push(code.len());
        code.extend([0, 0, opcode::JUMPI]);
        if recursive && phase == 0 {
            code.extend([
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::PUSH0,
                opcode::ADDRESS,
                opcode::GAS,
                opcode::CALL,
                opcode::POP,
            ]);
        }
    }
    code.extend([
        opcode::POP,
        opcode::PUSH1,
        1,
        opcode::PUSH0,
        opcode::MSTORE,
        opcode::PUSH1,
        32,
        opcode::PUSH0,
        opcode::RETURN,
    ]);
    let impossible = code.len();
    code.extend([opcode::JUMPDEST, opcode::INVALID]);
    for position in targets {
        code[position..position + 2].copy_from_slice(&(impossible as u16).to_be_bytes());
    }
    Fixture { code, impossible }
}

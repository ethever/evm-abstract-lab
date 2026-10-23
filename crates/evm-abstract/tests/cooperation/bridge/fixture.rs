use alloy_primitives::U256;
use evm_abstract::domain::symbolic::{ExprId, ExprKind};
use revm_bytecode::opcode;

pub fn compares_excluded_word(expression: &ExprId, value: &ExprId) -> bool {
    let ExprKind::Operation {
        opcode: opcode::EQ,
        args,
    } = expression.kind()
    else {
        return false;
    };
    args.len() == 2
        && (0..2).any(|constant| {
            let term = &args[1 - constant];
            if args[constant].as_constant() == Some(U256::from(42)) && term == value {
                return true;
            }
            let ExprKind::Operation {
                opcode: opcode::MUL,
                args: factors,
            } = term.kind()
            else {
                return false;
            };
            // Odd multiplication is bijective modulo 2^256: 3*v==126 iff v==42.
            // The native local inverse engine does not supply this MUL proof.
            args[constant].as_constant() == Some(U256::from(126))
                && factors.len() == 2
                && (0..2).any(|factor| {
                    factors[factor].as_constant() == Some(U256::from(3))
                        && factors[1 - factor] == *value
                })
        })
}

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
        code.push(opcode::DUP1);
        if !recursive {
            // Keep the feedback ablation useful alongside current-store branch
            // refinement, which can independently reject direct v==42 guards.
            code.extend([opcode::PUSH1, 3, opcode::MUL]);
        }
        code.extend([
            opcode::PUSH1,
            if recursive { 42 } else { 126 },
            opcode::EQ,
            opcode::PUSH2,
        ]);
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

use super::support::{A, Assembly, Fixture, USER};
use revm_bytecode::opcode;

pub(super) fn counter(target: u16) -> Fixture {
    let mut code = Assembly::default();
    code.input()
        .op(opcode::ISZERO)
        .jump("base")
        .decrement_input()
        .label("call")
        .call(opcode::CALL, target, 0, 32, 0)
        .op(opcode::POP);
    code.label("base").return_word(42);
    code.finish()
}

pub(super) fn context_counter(op: u8, target: u16) -> Fixture {
    let mut code = Assembly::default();
    code.input()
        .op(opcode::ISZERO)
        .jump("base")
        .decrement_input()
        .label("call")
        .call(op, target, 7, 32, 128)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 128);
    code.label("base")
        .op(opcode::ADDRESS)
        .mstore(0)
        .op(opcode::CALLER)
        .mstore(32)
        .op(opcode::CALLVALUE)
        .mstore(64)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(96)
        .exit(opcode::RETURN, 128);
    code.label("failed").exit(opcode::REVERT, 0);
    code.finish()
}

pub(super) fn no_base_success() -> Fixture {
    let mut code = Assembly::default();
    code.label("call")
        .call_address(0, 32)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 32);
    code.label("failed")
        .push(99)
        .mstore(0)
        .exit(opcode::REVERT, 32);
    code.finish()
}

pub(super) fn growth() -> Fixture {
    let mut code = Assembly::default();
    code.input()
        .op(opcode::ISZERO)
        .jump("base")
        .push(0)
        .op(opcode::SLOAD)
        .push(1)
        .op(opcode::ADD)
        .push(0)
        .op(opcode::SSTORE)
        .push(0)
        .op(opcode::TLOAD)
        .push(1)
        .op(opcode::ADD)
        .push(0)
        .op(opcode::TSTORE)
        .decrement_input()
        .label("call")
        .call_address(32, 0)
        .op(opcode::POP);
    code.label("base")
        .push(0)
        .op(opcode::SLOAD)
        .mstore(0)
        .push(0)
        .op(opcode::TLOAD)
        .mstore(32)
        .push(1)
        .op(opcode::SLOAD)
        .mstore(64)
        .exit(opcode::RETURN, 96);
    code.finish()
}

/// The root writes before entering a child; every recursive child eventually
/// reverts or faults, so the parent must retain only its own writes and log.
pub(super) fn rollback(exceptional: bool, outer_revert: bool) -> Fixture {
    let mut code = Assembly::default();
    code.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("child")
        .store(0, 5)
        .tstore(0, 7)
        .push(700)
        .push(0)
        .push(0)
        .op(opcode::LOG1)
        .input()
        .mstore(0)
        .label("outer_call")
        .call_address(32, 0)
        .mstore(0)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .push(0)
        .op(opcode::TLOAD)
        .mstore(64)
        .exit(
            if outer_revert {
                opcode::REVERT
            } else {
                opcode::RETURN
            },
            96,
        );
    code.label("child")
        .store(0, 9)
        .tstore(0, 11)
        .push(900)
        .push(0)
        .push(0)
        .op(opcode::LOG1)
        .input()
        .op(opcode::ISZERO)
        .jump("child_exit")
        .decrement_input()
        .label("recursive_call")
        .call_address(32, 0)
        .op(opcode::POP);
    code.label("child_exit");
    if exceptional {
        code.op(opcode::INVALID);
    } else {
        code.push(99).mstore(0).exit(opcode::REVERT, 32);
    }
    code.finish()
}

/// Identical bytes alternate code accounts while each account reads its own
/// slot. Neither code hash equality nor the same PC permits owner merging.
pub(super) fn same_code_two_owners(second: u16) -> Fixture {
    let mut code = Assembly::default();
    code.input()
        .op(opcode::ISZERO)
        .jump("base")
        .decrement_input()
        .op(opcode::ADDRESS)
        .push(A as u16)
        .op(opcode::EQ)
        .jump("from_a")
        .call(opcode::CALL, A as u16, 0, 32, 32)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 32);
    code.label("from_a")
        .call(opcode::CALL, second, 0, 32, 32)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 32);
    code.label("base")
        .push(0)
        .op(opcode::SLOAD)
        .mstore(0)
        .exit(opcode::RETURN, 32);
    code.label("failed").exit(opcode::REVERT, 0);
    code.finish()
}

pub(super) fn leaf() -> Fixture {
    let mut code = Assembly::default();
    code.push(0)
        .op(opcode::SLOAD)
        .mstore(0)
        .exit(opcode::RETURN, 32);
    code.finish()
}

pub(super) fn chain(target: u16) -> Fixture {
    let mut code = Assembly::default();
    code.call(opcode::CALL, target, 0, 0, 32)
        .op(opcode::ISZERO)
        .jump("failed")
        .exit(opcode::RETURN, 32);
    code.label("failed").exit(opcode::REVERT, 0);
    code.finish()
}

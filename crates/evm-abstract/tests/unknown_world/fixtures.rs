use super::support::{A, Assembly, C, Fixture, U, USER, V};
use revm_bytecode::opcode;

/// run uses busy slot 0 and x slot 1; selectors 1/2 enter the issue's one/two.
/// Root returndata exposes x and busy at the instant unknown U returned.
pub(super) fn busy_x(nested: bool) -> Fixture {
    let mut a = Assembly::default();
    a.input(0);
    for (selector, label) in [(0, "run"), (1, "one"), (2, "two")] {
        a.op(opcode::DUP1)
            .push(selector)
            .op(opcode::EQ)
            .jump(label, true);
    }
    if nested {
        a.op(opcode::DUP1)
            .push(3)
            .op(opcode::EQ)
            .jump("three", true);
    }
    a.op(opcode::POP).op(opcode::STOP);
    a.label("run")
        .op(opcode::POP)
        .push(0)
        .op(opcode::SLOAD)
        .jump("busy_revert", true)
        .store(0, 1)
        .store(1, 0)
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .label("resumed")
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .store(0, 0)
        .push(1)
        .op(opcode::SLOAD)
        .mstore(0)
        .return_range(0, 64);
    a.label("one").op(opcode::POP).store(1, 1).op(opcode::STOP);
    a.label("two")
        .op(opcode::POP)
        .push(1)
        .op(opcode::SLOAD)
        .push(1)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("two_done", true)
        .store(1, 2);
    a.label("two_done").op(opcode::STOP);
    if nested {
        a.label("three")
            .op(opcode::POP)
            .call(opcode::CALL, V as u16, 0, (0, 0), (0, 0))
            .op(opcode::POP)
            .op(opcode::STOP);
    }
    a.label("busy_revert").push(0).push(0).op(opcode::REVERT);
    a.finish()
}

/// Caller guard prevents callbacks from entering root setup again. There is
/// no callback path that writes slot 0, so a CALL has to retain parent 42.
pub(super) fn preserved_parent(op: u8, is_value_call: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .store(0, 42)
        .call(
            op,
            U as u16,
            if is_value_call { 7 } else { 0 },
            (0, 0),
            (0, 0),
        )
        .label("resumed")
        .mstore(0)
        .push(0)
        .op(opcode::SLOAD)
        .mstore(32)
        .return_range(0, 64);
    a.label("callback").op(opcode::STOP);
    a.finish()
}

/// C's own entry executes both reads and guarded writes. The parent A cannot
/// read C storage with SLOAD; it reads the child's returned bytes each time.
pub(super) fn external_reader(outer_revert: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .call(opcode::CALL, C as u16, 0, (0, 0), (0, 32))
        .op(opcode::ISZERO)
        .jump("read_failed", true)
        .push(0)
        .op(opcode::MLOAD)
        .call(opcode::CALL, U as u16, 0, (0, 0), (64, 0))
        .op(opcode::POP)
        .label("resumed")
        .call(opcode::CALL, C as u16, 0, (0, 0), (32, 32))
        .op(opcode::ISZERO)
        .jump("read_failed", true)
        .mstore(0);
    if outer_revert {
        a.push(64).push(0).op(opcode::REVERT);
    } else {
        a.return_range(0, 64);
    }
    a.label("callback").op(opcode::STOP);
    a.label("read_failed").push(0).push(0).op(opcode::REVERT);
    a.finish()
}

pub(super) fn external_cell() -> Fixture {
    let mut c = Assembly::default();
    c.input(0)
        .push(1)
        .op(opcode::EQ)
        .jump("write", true)
        .push(0)
        .op(opcode::SLOAD)
        .return_top();
    c.label("write")
        .push(1)
        .op(opcode::SLOAD)
        .op(opcode::ISZERO)
        .jump("no_write", true)
        .store(0, 9);
    c.label("no_write").push(0).op(opcode::SLOAD).return_top();
    c.finish()
}

/// Finite concrete U witnesses call the actual known entry in sequence.
pub(super) fn callback_sequence(selectors: &[u16], revert: bool) -> Fixture {
    let mut u = Assembly::default();
    for selector in selectors {
        u.push(*selector)
            .mstore(0)
            .call(opcode::CALL, A as u16, 0, (0, 32), (0, 0))
            .op(opcode::POP);
    }
    u.push(0).push(0).op(if revert {
        opcode::REVERT
    } else {
        opcode::RETURN
    });
    u.finish()
}

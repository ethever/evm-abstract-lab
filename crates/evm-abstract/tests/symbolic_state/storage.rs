use super::support::{Assembly, ROOT, no_bad_branch, reachable, return_words, run};
use revm_bytecode::opcode;

fn store_input_key(code: &mut Assembly, input_offset: u16, value: u16) {
    code.push(value).load_input(input_offset).op(opcode::SSTORE);
}

#[test]
fn same_unknown_slot_write_then_read_is_exact() {
    let mut code = Assembly::default();
    store_input_key(&mut code, 0, 7);
    code.load_input(0).op(opcode::SLOAD).return_top();
    return_words(&run(&code.finish().code), &[7]);
}

#[test]
fn guarded_equal_unknown_slots_read_the_last_write() {
    alias_guard(true);
}

#[test]
fn guarded_unequal_unknown_slots_preserve_the_earlier_write() {
    alias_guard(false);
}

fn alias_guard(equal: bool) {
    let expected = if equal { 9 } else { 7 };
    let mut code = Assembly::default();
    code.load_input(0).load_input(32).op(opcode::EQ);
    if !equal {
        code.op(opcode::ISZERO);
    }
    code.jump("checked", true).return_word(0).label("checked");
    store_input_key(&mut code, 0, 7);
    store_input_key(&mut code, 32, 9);
    code.load_input(0)
        .op(opcode::SLOAD)
        .require_top(expected)
        .return_top()
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    assert!(reachable(&analysis, ROOT, fixture.labels["checked"]));
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[0, u64::from(expected)]);
}

#[test]
fn unresolved_alias_read_retains_both_results_and_their_conditions() {
    let mut code = Assembly::default();
    store_input_key(&mut code, 0, 7);
    store_input_key(&mut code, 32, 9);
    // Read before learning whether k and q alias; the later branch must refine it.
    code.load_input(0)
        .op(opcode::SLOAD)
        .load_input(0)
        .load_input(32)
        .op(opcode::EQ)
        .jump("equal", true)
        .label("unequal")
        .require_top(7)
        .return_top()
        .label("equal")
        .require_top(9)
        .return_top()
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    for label in ["equal", "unequal"] {
        assert!(
            reachable(&analysis, ROOT, fixture.labels[label]),
            "{label} has a witness"
        );
    }
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[7, 9]);
}

#[test]
fn smt_proved_distinct_arithmetic_slot_preserves_last_write_order() {
    // k and k+1 differ even at the EVM word wrap boundary.
    let mut code = Assembly::default();
    store_input_key(&mut code, 0, 7);
    code.push(9)
        .load_input(0)
        .push(1)
        .op(opcode::ADD)
        .op(opcode::SSTORE)
        .load_input(0)
        .op(opcode::SLOAD)
        .require_top(7)
        .return_top()
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[7]);
}

#[test]
fn concrete_and_symbolic_writes_obey_order_in_both_directions() {
    for symbolic_last in [false, true] {
        let mut code = Assembly::default();
        if symbolic_last {
            code.push(7).op(opcode::PUSH0).op(opcode::SSTORE);
            store_input_key(&mut code, 0, 9);
            code.op(opcode::PUSH0).op(opcode::SLOAD);
        } else {
            store_input_key(&mut code, 0, 7);
            code.push(9)
                .op(opcode::PUSH0)
                .op(opcode::SSTORE)
                .load_input(0)
                .op(opcode::SLOAD);
        }
        code.load_input(0)
            .op(opcode::ISZERO)
            .jump("equal", true)
            .label("unequal")
            .require_top(7)
            .return_top()
            .label("equal")
            .require_top(9)
            .return_top()
            .label("bad")
            .return_word(0xffff);
        let fixture = code.finish();
        let analysis = run(&fixture.code);
        no_bad_branch(&analysis, &fixture, ROOT);
        return_words(&analysis, &[7, 9]);
    }
}

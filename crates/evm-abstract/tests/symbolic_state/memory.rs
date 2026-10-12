use super::support::{Assembly, ROOT, complete, no_bad_branch, return_words, run, word};
use alloy_primitives::U256;
use evm_abstract::{analysis::OutcomeKind, domain::Domain};
use revm_bytecode::opcode;

// p ranges over 512 consecutive byte offsets, rather than an enumerable set.
fn symbolic_offset(code: &mut Assembly) {
    code.load_input(0).push(0x1ff).op(opcode::AND);
}

#[test]
fn symbolic_unaligned_word_partial_overwrite_preserves_old_read() {
    let original = U256::from_be_bytes([0x55; 32]);
    let mut overwritten = [0x55; 32];
    overwritten[31] = 0xaa;
    let mut code = Assembly::default();
    code.push_word(original);
    symbolic_offset(&mut code);
    code.op(opcode::MSTORE);
    symbolic_offset(&mut code);
    code.op(opcode::MLOAD).push(0xaa);
    symbolic_offset(&mut code);
    code.push(31).op(opcode::ADD).op(opcode::MSTORE8);
    symbolic_offset(&mut code);
    code.op(opcode::MLOAD)
        .push(32)
        .op(opcode::MSTORE)
        .op(opcode::PUSH0)
        .op(opcode::MSTORE)
        .push(64)
        .op(opcode::PUSH0)
        .op(opcode::RETURN);
    let analysis = run(&code.finish().code);
    complete(&analysis);
    for outcome in analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
    {
        assert_eq!(outcome.data.len().singleton(), Some(U256::from(64)));
        assert_eq!(
            outcome
                .data
                .read_word(&word(0), Domain::default())
                .singleton(),
            Some(original)
        );
        assert_eq!(
            outcome
                .data
                .read_word(&word(32), Domain::default())
                .singleton(),
            Some(U256::from_be_bytes(overwritten))
        );
    }
}

#[test]
fn symbolic_word_byte_overlay_and_historical_read_feed_later_guards() {
    let mut code = Assembly::default();
    code.load_input(32);
    symbolic_offset(&mut code);
    code.op(opcode::MSTORE);
    symbolic_offset(&mut code);
    code.op(opcode::MLOAD).push(0xaa);
    symbolic_offset(&mut code);
    code.push(31)
        .op(opcode::ADD)
        .op(opcode::MSTORE8)
        .load_input(32)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true);
    symbolic_offset(&mut code);
    code.op(opcode::MLOAD)
        .load_input(32)
        .push_word(U256::MAX - U256::from(255))
        .op(opcode::AND)
        .push(0xaa)
        .op(opcode::OR)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("bad", true)
        .return_word(7)
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[7]);
}

#[test]
fn joined_branch_address_keeps_its_own_memory() {
    let mut code = Assembly::default();
    code.load_input(0)
        .jump("right", true)
        .label("left")
        .push(7)
        .push(0x80)
        .op(opcode::MSTORE)
        .push(0x80)
        .jump("join", false)
        .label("right")
        .push(7)
        .push(0xc0)
        .op(opcode::MSTORE)
        .push(0xc0)
        .jump("join", false)
        .label("join")
        .op(opcode::MLOAD)
        .return_top();
    // support::config uses context_depth=0: the two payloads must actually join.
    return_words(&run(&code.finish().code), &[7]);
}

#[test]
fn copied_unknown_content_hashes_to_the_same_storage_slot() {
    let mut code = Assembly::default();
    code.op(opcode::CALLDATASIZE)
        .push(32)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("outside", true)
        .push(32)
        .op(opcode::PUSH0)
        .push(0x80)
        .op(opcode::CALLDATACOPY)
        .push(32)
        .push(0x80)
        .op(opcode::KECCAK256)
        .push(7)
        .op(opcode::SWAP1)
        .op(opcode::SSTORE)
        .push(32)
        .push(0x80)
        .push(0xc0)
        .op(opcode::MCOPY)
        .push(32)
        .push(0xc0)
        .op(opcode::KECCAK256)
        .op(opcode::SLOAD)
        .require_top(7)
        .return_top()
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[0, 7]);
}

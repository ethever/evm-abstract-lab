use super::support::{
    Assembly, ROOT, address, complete, config, conservative_guarded_returns_include,
    construct_world, entry, executed_opcode, no_bad_branch, numeric_spec, possible_lengths,
    reachable, return_words, run, run_world, word,
};
use crate::native;
use alloy_primitives::U256;
use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Fork,
    analysis::OutcomeKind,
    domain::{
        EvmValue,
        provenance::{Origin, Provenance},
        relational::{CheckResult, EvmValueQuery},
        symbolic::{ByteStateKind, ExprKind, SlotStateKind},
    },
    world::EvmByteArray,
};
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
    let returned: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|o| o.kind == OutcomeKind::Return)
        .collect();
    assert!(!returned.is_empty(), "the 64-byte return must execute");
    for outcome in returned {
        assert_eq!(outcome.data.len().singleton(), Some(U256::from(64)));
        assert_eq!(
            outcome
                .data
                .read_word(&word(0), native::product().borrowed())
                .singleton(),
            Some(original)
        );
        assert_eq!(
            outcome
                .data
                .read_word(&word(32), native::product().borrowed())
                .singleton(),
            Some(U256::from_be_bytes(overwritten))
        );
    }
}

#[test]
fn symbolic_word_byte_overlay_keeps_history_and_concrete_guard_witnesses() {
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
        .jump("bad_history", true);
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
        .return_word(0xffff)
        .label("bad_history")
        .return_word(0xfffe);
    let fixture = code.finish();
    let analysis = run(&fixture.code);
    // The old whole-word read has direct source identity. Only reconstructing
    // the later symbolic byte overlay may lose its compound correlation.
    assert!(!reachable(&analysis, ROOT, fixture.labels["bad_history"]));
    conservative_guarded_returns_include(&analysis, &[7]);
    let world = construct_world(Fork::Osaka, &[(ROOT, &fixture.code)]);
    for (offset, original) in [
        (0u64, U256::ZERO),
        (31, U256::from(0x1234)),
        (511, U256::MAX),
    ] {
        let mut calldata = U256::from(offset).to_be_bytes::<32>().to_vec();
        calldata.extend(original.to_be_bytes::<32>());
        let input = entry(EvmByteArray::<NumericValue>::exact(&calldata));
        let exact = run_world(world.clone(), input.clone(), config());
        no_bad_branch(&exact, &fixture, ROOT);
        assert!(!reachable(&exact, ROOT, fixture.labels["bad_history"]));
        return_words(&exact, &[7]);
        let observed = super::oracle::compare(&world, &input, &exact);
        assert_eq!(observed.output, U256::from(7).to_be_bytes::<32>());
        super::oracle::compare(&world, &input, &analysis);
    }
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
        .label("hash_path")
        .push(32)
        .op(opcode::PUSH0)
        .push(0x80)
        .op(opcode::CALLDATACOPY)
        .push(32)
        .push(0x80)
        .label("first_hash")
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
    assert!(reachable(&analysis, ROOT, fixture.labels["hash_path"]));
    executed_opcode(&analysis, &fixture, "first_hash", opcode::KECCAK256);
    for expected in [0, 7] {
        assert!(
            analysis.outcomes().iter().any(|outcome| {
                outcome.kind == OutcomeKind::Return
                    && outcome
                        .data
                        .read_word(&word(0), native::product().borrowed())
                        .singleton()
                        == Some(U256::from(expected))
            }),
            "missing exact return {expected}"
        );
    }
}

#[test]
fn symbolic_length_copy_hash_storage_and_disjoint_mcopy_keep_the_source() {
    let mut code = Assembly::default();
    // GT pops L first: the outside branch is L > 512. Both copy windows fit
    // the unchanged 4KiB policy, including the zero-length input.
    code.push(512)
        .op(opcode::CALLDATASIZE)
        .op(opcode::GT)
        .jump("outside", true)
        .label("hash_path")
        .op(opcode::CALLDATASIZE)
        .op(opcode::PUSH0)
        .push(0x80)
        .label("copy")
        .op(opcode::CALLDATACOPY)
        .op(opcode::CALLDATASIZE)
        .push(0x80)
        .label("first_hash")
        .op(opcode::KECCAK256)
        .push(7)
        .op(opcode::SWAP1)
        .label("store")
        .op(opcode::SSTORE)
        .op(opcode::CALLDATASIZE)
        .push(0x80)
        .push(0x400)
        .label("memory_copy")
        .op(opcode::MCOPY)
        .op(opcode::CALLDATASIZE)
        .push(0x400)
        .label("second_hash")
        .op(opcode::KECCAK256)
        .label("load")
        .op(opcode::SLOAD)
        .require_top(7)
        .label("success")
        .return_top()
        .label("outside")
        .return_word(0)
        .label("bad")
        .return_word(0xffff);
    let fixture = code.finish();
    eprintln!(
        "symbolic hash fixture={} labels={:?}",
        fixture.code, fixture.labels
    );
    let analysis = run(&fixture.code);
    let domain = native::product_with(analysis.domain_spec());
    no_bad_branch(&analysis, &fixture, ROOT);
    return_words(&analysis, &[0, 7]);
    for (label, op) in [
        ("copy", opcode::CALLDATACOPY),
        ("first_hash", opcode::KECCAK256),
        ("store", opcode::SSTORE),
        ("memory_copy", opcode::MCOPY),
        ("second_hash", opcode::KECCAK256),
        ("load", opcode::SLOAD),
    ] {
        executed_opcode(&analysis, &fixture, label, op);
    }
    assert!(reachable(&analysis, ROOT, fixture.labels["success"]));
    assert!(reachable(&analysis, ROOT, fixture.labels["outside"]));
    assert!(analysis.outcomes().iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome
                .data
                .read_word(&word(0), domain.borrowed())
                .singleton()
                == Some(U256::ZERO)
    }));
    let good: Vec<_> = analysis
        .outcomes()
        .iter()
        .filter(|outcome| {
            outcome.kind == OutcomeKind::Return
                && outcome
                    .data
                    .read_word(&word(0), native::product().borrowed())
                    .singleton()
                    == Some(U256::from(7))
        })
        .collect();
    assert!(
        !good.is_empty(),
        "the full hash/storage route must return 7"
    );
    let payload = analysis.states()[good[0].state].exit.as_ref().unwrap();
    let frame = payload.active();
    let input = &frame.calldata;
    let length = input.len().expression().unwrap();
    let ByteStateKind::WriteWord {
        previous: copied,
        offset,
        ..
    } = frame.memory.state_root().kind()
    else {
        panic!("the return writes its word after the two captured copies");
    };
    assert_eq!(offset.as_constant(), Some(U256::ZERO));
    let ByteStateKind::Copy {
        previous: first,
        destination,
        source,
        length: copy_length,
    } = copied.kind()
    else {
        panic!("MCOPY must retain the earlier immutable memory");
    };
    assert_eq!(destination.as_constant(), Some(U256::from(0x400)));
    assert_eq!(copy_length, length);
    assert_eq!(source.state, *first);
    assert_eq!(source.offset.as_constant(), Some(U256::from(0x80)));
    assert_eq!(&source.length, length);
    let ByteStateKind::Copy {
        previous,
        destination,
        source,
        length: first_length,
    } = first.kind()
    else {
        panic!("CALLDATACOPY must retain the input snapshot");
    };
    assert!(matches!(previous.kind(), ByteStateKind::Zero));
    assert_eq!(destination.as_constant(), Some(U256::from(0x80)));
    assert_eq!(source.state, *input.state_root());
    assert_eq!(
        input.state_root(),
        analysis.states()[0].entry.active().calldata.state_root()
    );
    assert_eq!(source.offset.as_constant(), Some(U256::ZERO));
    assert_eq!(&source.length, length);
    assert_eq!(first_length, length);
    let captured_length = frame
        .memory
        .symbolic_facts()
        .find(|(expr, _)| *expr == length)
        .unwrap()
        .1;
    let copied_payload = analysis
        .states()
        .iter()
        .find(|state| state.executed_pcs.contains(&(fixture.labels["copy"] + 1)))
        .unwrap()
        .exit
        .as_ref()
        .unwrap();
    let original_operand = copied_payload
        .active()
        .memory
        .symbolic_facts()
        .find(|(expr, _)| *expr == length)
        .unwrap()
        .1;
    assert_eq!(captured_length, original_operand);
    let expected_length = domain
        .reduce(&EvmValue::<NumericValue>::unsigned_range(U256::ZERO, U256::from(512)).unwrap())
        .value;
    assert_eq!(captured_length.numeric(), expected_length.numeric());
    assert_eq!(
        captured_length.provenance(),
        &Provenance::source(Origin::Calldata)
    );
    possible_lengths(&payload.relations, captured_length, &[0, 32, 33, 64, 512]);
    assert_eq!(
        payload.relations.can_equal(
            length,
            U256::from(513),
            &numeric_spec().relations(),
            native::product_with(
                native::spec(8, 4, 256).with_relations(numeric_spec().relations())
            )
            .borrowed()
        ),
        CheckResult::ProvenImpossible
    );
    assert_eq!(captured_length.expression(), input.len().expression());
    let root = good[0].store.slot_root(address(ROOT), false).unwrap();
    let SlotStateKind::Store {
        previous,
        key,
        value,
    } = root.kind()
    else {
        panic!("the symbolic key must survive as an ordered strong write");
    };
    assert_eq!(
        *previous,
        analysis.states()[0]
            .entry
            .store
            .slot_root(address(ROOT), false)
            .unwrap()
    );
    assert_eq!(value.as_constant(), Some(U256::from(7)));
    let ExprKind::Keccak { input: hashed } = key.kind() else {
        panic!("storage key must retain its complete captured hash input");
    };
    assert_eq!(hashed.state, *first);
    assert_eq!(hashed.offset.as_constant(), Some(U256::from(0x80)));
    assert_eq!(&hashed.length, length);
    let first_hash_state = analysis
        .states()
        .iter()
        .find(|state| {
            state
                .executed_pcs
                .contains(&(fixture.labels["first_hash"] + 1))
        })
        .unwrap();
    // This block also executes PUSH7/SWAP1 before the SSTORE block: [7, key].
    assert_eq!(first_hash_state.exit_stack.len(), 2);
    assert_eq!(first_hash_state.exit_stack[0], word(7));
    let first_key = &first_hash_state.exit_stack[1];
    let stored_key = good[0]
        .store
        .symbolic_facts(false)
        .find(|(expression, _)| *expression == key)
        .unwrap()
        .1;
    assert_eq!(stored_key, first_key);
    let key_value = EvmValue::<NumericValue>::top().with_expression(key.clone());
    let read = good[0]
        .store
        .read_checked(address(ROOT), &key_value, domain.borrowed())
        .unwrap();
    assert_eq!(read.numeric(), word(7).numeric());
    assert_eq!(read.provenance(), word(7).provenance());
    assert_eq!(
        read,
        good[0]
            .store
            .read_checked(address(ROOT), &key_value, domain.borrowed())
            .unwrap()
    );
    assert_eq!(
        payload.relations.local_value(
            &read.symbolic_expression().unwrap(),
            &numeric_spec().relations(),
            native::product_with(
                native::spec(8, 4, 256).with_relations(numeric_spec().relations())
            )
            .borrowed()
        ),
        EvmValueQuery::<NumericValue>::Exact(U256::from(7))
    );
    // Capture the actual second KECCAK before RETURN's later memory write.
    // Check the alias independently under its pre-SLOAD relations, before the
    // EVM success guard could contribute a constraint on the loaded value.
    let hash_state = analysis
        .states()
        .iter()
        .find(|state| {
            state
                .executed_pcs
                .contains(&(fixture.labels["second_hash"] + 1))
        })
        .unwrap();
    let hashed_payload = hash_state.exit.as_ref().unwrap();
    assert_eq!(hash_state.exit_stack.len(), 1);
    let actual_second_key = &hash_state.exit_stack[0];
    let second_view = hashed_payload
        .active()
        .memory
        .slice(&word(0x400), captured_length, domain.borrowed())
        .unwrap();
    let second_key = second_view
        .keccak(domain.borrowed(), config().max_memory_bytes)
        .unwrap();
    assert_eq!(second_key.numeric(), actual_second_key.numeric());
    // The native content API has no opcode source annotation. The real EVM
    // keys keep the full annotation supplied by the transfer consumer.
    assert_eq!(second_key.provenance(), &Provenance::top());
    assert_eq!(
        actual_second_key.provenance(),
        &Provenance::source(Origin::Environment)
    );
    assert_eq!(actual_second_key.provenance(), first_key.provenance());
    assert_eq!(second_key.expression(), actual_second_key.expression());
    let second_read = good[0]
        .store
        .read_checked(address(ROOT), &second_key, domain.borrowed())
        .unwrap();
    assert_eq!(
        hashed_payload.relations.local_value(
            &second_read.symbolic_expression().unwrap(),
            &numeric_spec().relations(),
            native::product_with(
                native::spec(8, 4, 256).with_relations(numeric_spec().relations())
            )
            .borrowed()
        ),
        EvmValueQuery::<NumericValue>::Exact(U256::from(7))
    );
}

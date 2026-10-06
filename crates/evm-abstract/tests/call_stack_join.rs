//! A repeated call joins its rollback preconditions and caller output contract.

use evm_abstract::{
    Address, Fork, U256,
    analysis::{Config, ExecutionConfig, MachineEdgeKind, Status, analyze_world},
    domain::{Domain, Value},
    ssa,
    world::{Account, ByteArray, Entry, World},
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> Value {
    Value::constant(U256::from(number))
}

#[test]
fn repeated_reverting_call_joins_checkpoints_and_output_ranges_before_resuming_root() {
    // The same DELEGATECALL first requests (offset=0, size=32), then loops with
    // (offset=32, size=64). Both entries have the same structural child identity.
    let caller = "5b6001545f545f5f6102006207a120f45060205f5560406001555f56";
    // Overwrite the caller's persistent and transient state, then REVERT with
    // two distinguishable words so copying either output range is observable.
    let callee = "60635f556063600155606360025d60ab5f5260cd60205260405ffd";
    let mut world = World::new(Fork::Osaka, "joined call checkpoint fixture");
    let mut account = Account::from_hex(caller, world.fork()).unwrap();
    account.storage.insert(U256::ZERO, word(0));
    account.storage.insert(U256::from(1), word(32));
    world.insert(address(0x101), account).unwrap();
    world
        .insert(
            address(0x200),
            Account::from_hex(callee, world.fork()).unwrap(),
        )
        .unwrap();
    let analysis = analyze_world(
        world,
        Entry {
            address: address(0x101),
            environment: evm_abstract::world::EvmEnvironment {
                to: (address(0x101)).into(),
                caller: (address(0x900)).into(),
                value: word(0),
                calldata: ByteArray::empty(),
                is_static: false,
                ..evm_abstract::world::EvmEnvironment::default()
            },
        },
        ExecutionConfig {
            // Exercise the ordinary worklist's frame joins directly.
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();

    let children: Vec<_> = analysis
        .states()
        .iter()
        .filter(|state| state.entry.call_stack.depth() == 2)
        .collect();
    assert_eq!(
        children.len(),
        1,
        "the fixture must join one child identity"
    );
    let child_state = children[0];
    let child = child_state.entry.call_stack.active_child().unwrap();
    let domain = Domain::default();
    let offsets = domain.join(&word(0), &word(32));
    let sizes = domain.join(&word(32), &word(64));
    assert_numeric_eq(child.continuation.output_offset.clone(), offsets.clone());
    assert_numeric_eq(child.continuation.output_size.clone(), sizes.clone());
    assert_numeric_eq(
        child
            .state
            .saved_store
            .state()
            .read(address(0x101), &word(0), domain),
        offsets.clone(),
    );
    assert_numeric_eq(
        child
            .state
            .saved_store
            .state()
            .read(address(0x101), &word(1), domain),
        sizes.clone(),
    );
    // The checkpoint stays independent of the callee's writes before REVERT.
    let executing_child = child_state.exit.as_ref().unwrap();
    assert_numeric_eq(
        executing_child.store.read(address(0x101), &word(0), domain),
        word(99),
    );
    assert_numeric_eq(
        executing_child
            .store
            .read_transient(address(0x101), &word(2), domain),
        word(99),
    );

    let reverts: Vec<_> = analysis
        .edges()
        .iter()
        .filter(|edge| edge.kind == MachineEdgeKind::Revert)
        .collect();
    assert_eq!(reverts.len(), 1);
    let edge = reverts[0];
    assert_eq!(edge.from, child_state.id);
    let resumed = &analysis.states()[edge.to].entry;
    assert_eq!(resumed.call_stack.depth(), 1);
    assert!(resumed.call_stack.active_child().is_none());
    assert_numeric_eq(
        resumed.store.read(address(0x101), &word(0), domain),
        offsets.clone(),
    );
    assert_numeric_eq(
        resumed.store.read(address(0x101), &word(1), domain),
        sizes.clone(),
    );
    assert_numeric_eq(
        resumed
            .store
            .read_transient(address(0x101), &word(2), domain),
        word(0),
    );
    assert_eq!(
        resumed
            .active()
            .stack
            .iter()
            .map(Value::singleton)
            .collect::<Vec<_>>(),
        vec![word(0).singleton()]
    );
    // Immediate gas rejection also reaches this parent state with empty data.
    assert!(
        resumed
            .active()
            .returndata
            .read_word(&word(0), domain)
            .contains(U256::from(0xab))
    );
    assert!(
        resumed
            .active()
            .returndata
            .read_word(&word(32), domain)
            .contains(U256::from(0xcd))
    );
    assert!(
        resumed
            .active()
            .memory
            .byte_at(31, domain)
            .contains(U256::from(0xab))
    );
    assert!(
        resumed
            .active()
            .memory
            .byte_at(63, domain)
            .contains(U256::from(0xab))
    );
    assert!(
        resumed
            .active()
            .memory
            .byte_at(95, domain)
            .contains(U256::from(0xcd))
    );
}

fn assert_numeric_eq(actual: Value, expected: Value) {
    assert_eq!(actual.constants(), expected.constants());
    assert_eq!(actual.known_bits(), expected.known_bits());
    for value in expected.constants().unwrap() {
        assert!(actual.contains(*value));
    }
    assert_eq!(actual.congruence(), expected.congruence());
}

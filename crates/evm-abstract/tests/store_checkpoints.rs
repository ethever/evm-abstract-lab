//! Complete Store versions must survive branching, joins and nested rollback.

use evm_abstract::{
    Address, Fork, U256,
    bytecode::Program,
    domain::{AbstractValue, Domain},
    world::{AbstractLog, Account, ByteArray, LogKey, Snapshot, Store, World},
};

fn word(value: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(value))
}

fn fixture() -> (Store, Address, Address) {
    let caller = Address::repeat_byte(0x11);
    let created = Address::repeat_byte(0x22);
    let mut world = World::new(Fork::Osaka, "checkpoint fixture");
    world.insert(caller, Account::empty()).unwrap();
    world.insert(created, Account::absent()).unwrap();
    (Store::new(&world), caller, created)
}

#[test]
fn unchanged_store_join_preserves_exact_summary_preconditions() {
    let (store, _, _) = fixture();
    let joined = store.join(&store, Domain::default());
    assert_eq!(
        joined, store,
        "joining unchanged paths must not invent lifecycle observations"
    );
    assert_eq!(joined.code_identity(), store.code_identity());
}

#[test]
fn lifecycle_join_retains_uncertainty_between_created_and_unchanged_paths() {
    let domain = Domain::default();
    let (original, caller, created) = fixture();
    let mut changed = original.clone();
    changed.begin_creation(created);
    changed.selfdestruct(created, caller, domain);
    let joined = original.join(&changed, domain);
    assert_eq!(joined.created_in_transaction(created), None);
    assert_eq!(joined.pending_destruction(created), None);
    assert_eq!(joined.created_in_transaction(caller), Some(false));
    assert_eq!(joined.pending_destruction(caller), Some(false));
}

#[test]
fn outer_rollback_restores_successful_deeper_effects_and_all_state_planes() {
    let domain = Domain::default();
    let (mut store, caller, created) = fixture();
    store.write(caller, &word(0), &word(3), domain);
    store.write_balance(caller, word(100));
    let before_call = store.clone();
    let outer = store.snapshot();

    store.write(caller, &word(1), &word(7), domain);
    store.write_transient(caller, &word(0), &word(9), domain);
    store.write_balance(caller, word(90));
    store.write_nonce(caller, word(1));
    let child = store.snapshot();
    store.begin_creation(created);
    store.deploy_code(created, Program::from_hex("00").unwrap());
    store.write(created, &word(0), &word(42), domain);
    store.write_balance(created, word(10));
    store.selfdestruct(created, caller, domain);
    store
        .emit_log(
            LogKey {
                address: created,
                code_address: created,
                pc: 0,
            },
            AbstractLog {
                topics: vec![word(42)],
                data: ByteArray::exact(&[42]),
            },
            domain,
        )
        .unwrap();
    drop(child); // Successful child retains its effects within the outer call.
    assert_eq!(store.pending_destruction(created), Some(true));
    assert_eq!(store.possible_logs().len(), 1);
    assert_ne!(store, before_call);

    store.restore(outer);
    assert_eq!(store, before_call);
    assert_eq!(store.read(caller, &word(0), domain), word(3));
    assert_eq!(store.code_identity(), before_call.code_identity());
    assert_eq!(store.work_size(), before_call.work_size());
}

#[test]
fn divergent_checkpoints_merge_pre_call_states_without_aliasing_live_versions() {
    let domain = Domain::default();
    let (mut left, caller, _) = fixture();
    let mut right = left.clone();
    left.write(caller, &word(0), &word(3), domain);
    right.write(caller, &word(0), &word(5), domain);
    let left_checkpoint = left.snapshot();
    let right_checkpoint = right.snapshot();

    left.write(caller, &AbstractValue::top(), &word(7), domain);
    right.havoc_all();
    assert_eq!(
        left_checkpoint.state().read(caller, &word(0), domain),
        word(3)
    );
    assert_eq!(
        right_checkpoint.state().read(caller, &word(0), domain),
        word(5)
    );

    let expected = left_checkpoint
        .state()
        .join(right_checkpoint.state(), domain);
    let merged = Snapshot::from_state(expected.clone());
    left.restore(merged);
    assert_eq!(left, expected);
    assert_eq!(
        left.read(caller, &word(0), domain),
        domain.join(&word(3), &word(5))
    );

    // Independent versions can be restored after another branch has been joined.
    left.restore(right_checkpoint);
    assert_eq!(left.read(caller, &word(0), domain), word(5));
    right.restore(left_checkpoint);
    assert_eq!(right.read(caller, &word(0), domain), word(3));
}

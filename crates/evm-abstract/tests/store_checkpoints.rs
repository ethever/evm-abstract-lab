//! Complete Store versions must survive branching, joins and nested rollback.

#[path = "support/native.rs"]
mod native;

use evm_abstract::domain::NumericValue;
use evm_abstract::{
    Address, Fork, U256,
    bytecode::Program,
    domain::{
        EvmValue,
        symbolic::{ExprKind, SlotStateKind},
    },
    world::{EvmAccount, EvmByteArray, EvmLog, EvmSnapshot, EvmStore, EvmWorld, LogKey},
};

fn word(value: u64) -> EvmValue<NumericValue> {
    EvmValue::<NumericValue>::constant(U256::from(value))
}

fn fixture() -> (EvmStore<NumericValue>, Address, Address) {
    let caller = Address::repeat_byte(0x11);
    let created = Address::repeat_byte(0x22);
    let mut world = EvmWorld::<NumericValue>::new(Fork::Osaka, "checkpoint fixture");
    world
        .insert(caller, EvmAccount::<NumericValue>::empty())
        .unwrap();
    world
        .insert(created, EvmAccount::<NumericValue>::absent())
        .unwrap();
    (
        EvmStore::<NumericValue>::new(&world, native::product().borrowed()),
        caller,
        created,
    )
}

#[test]
fn unchanged_store_join_preserves_exact_summary_preconditions() {
    let (store, _, _) = fixture();
    let joined = store.join(&store, native::product().borrowed());
    assert_eq!(
        joined, store,
        "joining unchanged paths must not invent lifecycle observations"
    );
    assert_eq!(joined.code_identity(), store.code_identity());
}

#[test]
fn cloned_snapshots_share_complete_store_and_preserve_value_format_and_work() {
    let domain = native::product();
    let (mut store, caller, _) = fixture();
    store.write(caller, &word(0), &word(3), domain.borrowed());
    store.write_transient(caller, &word(0), &word(4), domain.borrowed());
    store
        .emit_log(
            LogKey {
                address: caller,
                code_address: caller,
                code_hash: alloy_primitives::B256::ZERO,
                source_kind: evm_abstract::world::LogSourceKind::Runtime,
                pc: 0,
            },
            EvmLog {
                topics: vec![word(5)],
                data: EvmByteArray::<NumericValue>::exact(&[1, 2, 3]),
            },
            domain.borrowed(),
        )
        .unwrap();
    let saved = store.snapshot();
    let siblings: Vec<_> = (0..64).map(|_| saved.clone()).collect();
    for sibling in &siblings {
        assert!(std::ptr::eq(saved.state(), sibling.state()));
        assert_eq!(saved.state().work_size(), sibling.state().work_size());
    }
    let owned = snapshot_state::Checkpoint::from_state(saved.state().clone());
    assert_eq!(
        serde_json::to_value(&saved).unwrap(),
        serde_json::to_value(&owned).unwrap()
    );
    assert_eq!(format!("{saved:?}"), format!("{owned:?}"));
    assert_eq!(format!("{saved:#?}"), format!("{owned:#?}"));
    let expected = saved.state().clone();
    store.havoc_all();
    store.restore(saved);
    let mut reverted = expected.clone();
    // Havoc covers a call to RIPEMD-160; its possible touch is the sole
    // consensus exception to ordinary checkpoint rollback.
    reverted.may_touch_account(Address::from_word(U256::from(3).into()));
    assert_eq!(store, reverted);
    assert_eq!(store.work_size(), reverted.work_size());
    store.write(caller, &word(0), &word(99), domain.borrowed());
    assert_eq!(siblings[0].state(), &expected);
    assert_eq!(siblings[0].state().possible_logs().len(), 1);
    let unique = EvmSnapshot::<NumericValue>::from_state(expected.clone());
    store.restore(unique);
    assert_eq!(store, reverted);
}

#[test]
fn lifecycle_join_retains_uncertainty_between_created_and_unchanged_paths() {
    let domain = native::product();
    let (original, caller, created) = fixture();
    let mut changed = original.clone();
    changed.begin_creation(created);
    changed.selfdestruct(created, caller, domain.borrowed());
    let joined = original.join(&changed, domain.borrowed());
    assert_eq!(joined.created_in_transaction(created), None);
    assert_eq!(joined.pending_destruction(created), None);
    assert_eq!(joined.created_in_transaction(caller), Some(false));
    assert_eq!(joined.pending_destruction(caller), Some(false));
}

#[test]
fn outer_rollback_restores_successful_deeper_effects_and_all_state_planes() {
    let domain = native::product();
    let (mut store, caller, created) = fixture();
    store.write(caller, &word(0), &word(3), domain.borrowed());
    store.write_balance(caller, word(100));
    let before_call = store.clone();
    let outer = store.snapshot();

    store.write(caller, &word(1), &word(7), domain.borrowed());
    store.write_transient(caller, &word(0), &word(9), domain.borrowed());
    store.write_balance(caller, word(90));
    store.write_nonce(caller, word(1));
    let child = store.snapshot();
    store.begin_creation(created);
    store.deploy_code(created, Program::from_hex("00").unwrap());
    store.write(created, &word(0), &word(42), domain.borrowed());
    store.write_balance(created, word(10));
    store.selfdestruct(created, caller, domain.borrowed());
    store
        .emit_log(
            LogKey {
                address: created,
                code_address: created,
                code_hash: alloy_primitives::B256::ZERO,
                source_kind: evm_abstract::world::LogSourceKind::Runtime,
                pc: 0,
            },
            EvmLog {
                topics: vec![word(42)],
                data: EvmByteArray::<NumericValue>::exact(&[42]),
            },
            domain.borrowed(),
        )
        .unwrap();
    drop(child); // Successful child retains its effects within the outer call.
    assert_eq!(store.pending_destruction(created), Some(true));
    assert_eq!(store.possible_logs().len(), 1);
    assert_ne!(store, before_call);

    store.restore(outer);
    assert_eq!(store, before_call);
    assert_eq!(store.read(caller, &word(0), domain.borrowed()), word(3));
    assert_eq!(store.code_identity(), before_call.code_identity());
    assert_eq!(store.work_size(), before_call.work_size());
}

#[test]
fn divergent_checkpoints_merge_pre_call_states_without_aliasing_live_versions() {
    let domain = native::product();
    let (mut left, caller, _) = fixture();
    let mut right = left.clone();
    left.write(caller, &word(0), &word(3), domain.borrowed());
    right.write(caller, &word(0), &word(5), domain.borrowed());
    let left_checkpoint = left.snapshot();
    let right_checkpoint = right.snapshot();
    let left_before = left.clone();
    let right_before = right.clone();
    let left_read = left.read(caller, &word(0), domain.borrowed());
    let right_read = right.read(caller, &word(0), domain.borrowed());
    assert_eq!(left_read, word(3));
    assert_eq!(right_read, word(5));

    left.write(
        caller,
        &EvmValue::<NumericValue>::top(),
        &word(7),
        domain.borrowed(),
    );
    right.havoc_all();
    assert_eq!(left_checkpoint.state(), &left_before);
    assert_eq!(right_checkpoint.state(), &right_before);
    assert_eq!(
        left_checkpoint
            .state()
            .read(caller, &word(0), domain.borrowed()),
        word(3)
    );
    assert_eq!(
        right_checkpoint
            .state()
            .read(caller, &word(0), domain.borrowed()),
        word(5)
    );

    let expected = left_checkpoint
        .state()
        .join(right_checkpoint.state(), domain.borrowed());
    let joined_read = expected.read(caller, &word(0), domain.borrowed());
    let product = domain.join(&word(3), &word(5));
    assert_eq!(joined_read.numeric(), product.numeric());
    assert_eq!(joined_read.provenance(), product.provenance());
    let state = expected.slot_root(caller, false).unwrap();
    let SlotStateKind::Ite {
        condition,
        then_state,
        else_state,
    } = state.kind()
    else {
        panic!("joined storage must cover both pre-call alternatives");
    };
    assert!(matches!(condition.kind(), ExprKind::Fresh(_)));
    assert_eq!(*then_state, left_before.slot_root(caller, false).unwrap());
    assert_eq!(*else_state, right_before.slot_root(caller, false).unwrap());
    let ExprKind::Ite {
        condition: read_condition,
        then_value,
        else_value,
    } = joined_read.expression().unwrap().kind()
    else {
        panic!("normalized checkpoint read must retain the storage join guard");
    };
    assert_eq!(read_condition, condition);
    assert_eq!(then_value.as_constant(), Some(U256::from(3)));
    assert_eq!(else_value.as_constant(), Some(U256::from(5)));
    assert!(joined_read.contains(U256::from(3)));
    assert!(joined_read.contains(U256::from(5)));
    assert_eq!(
        expected.read(caller, &word(0), domain.borrowed()),
        joined_read
    );
    let merged = EvmSnapshot::<NumericValue>::from_state(expected.clone());
    left.restore(merged);
    assert_eq!(left, expected);
    assert_eq!(left.read(caller, &word(0), domain.borrowed()), joined_read);

    // Independent versions can be restored after another branch has been joined.
    left.restore(right_checkpoint);
    assert_eq!(left, right_before);
    assert_eq!(left.read(caller, &word(0), domain.borrowed()), right_read);
    right.restore(left_checkpoint);
    let mut right_reverted = left_before.clone();
    right_reverted.may_touch_account(Address::from_word(U256::from(3).into()));
    assert_eq!(right, right_reverted);
    assert_eq!(right.read(caller, &word(0), domain.borrowed()), left_read);
    assert_eq!(
        expected.read(caller, &word(0), domain.borrowed()),
        joined_read
    );
    assert_eq!(joined_read.numeric(), product.numeric());
    assert_eq!(joined_read.provenance(), product.provenance());
}

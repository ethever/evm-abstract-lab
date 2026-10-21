//! Focused witnesses for the checked all-account pre-finalization adapter.

use alloy_primitives::{Address, B256, U256};
use {super::super::StoreError, crate::world::test_support::TestStore};
use {
    crate::Fork, crate::bytecode::Program, crate::domain::relational::RelationLimits,
    crate::domain::symbolic::SlotStateKind, crate::domain::symbolic::SourceId,
    crate::domain::symbolic::StateError, crate::world::Existence, crate::world::LogKey,
    crate::world::LogSourceKind, crate::world::test_support::TestAccount,
    crate::world::test_support::TestAlgebra, crate::world::test_support::TestBytes,
    crate::world::test_support::TestLog, crate::world::test_support::TestValue,
    crate::world::test_support::TestWorld,
};
use {crate::world::test_support::TestEntry, std::collections::BTreeMap};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> TestValue {
    TestValue::constant(U256::from(number))
}

fn original() -> TestStore {
    let mut world = TestWorld::new(Fork::Osaka, "root terminal independent effects");
    for owner in [address(0x101), address(0x202)] {
        let mut account = TestAccount::from_hex("00", Fork::Osaka).unwrap();
        account.storage_unknown = true;
        account.balance = word(10);
        account.nonce = word(1);
        world.insert(owner, account).unwrap();
    }
    TestStore::new_checked(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    )
    .unwrap()
}

/// Only the adapter's unconstrained input roots are interpreted here. One map
/// makes conflicting assignments to the same retained cell fail jointly.
#[derive(Default)]
struct InputAssignment(BTreeMap<(SourceId, U256), U256>);

impl InputAssignment {
    fn cell(&mut self, store: &TestStore, owner: Address, transient: bool, value: u64) -> bool {
        let root = store.slot_root(owner, transient).unwrap();
        let SlotStateKind::Input { source, bindings } = root.kind() else {
            return false;
        };
        if !bindings.is_empty() {
            return false;
        }
        let value = U256::from(value);
        match self.0.entry((source.clone(), U256::from(8))) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(value);
                true
            }
            std::collections::btree_map::Entry::Occupied(entry) => *entry.get() == value,
        }
    }
}

#[test]
fn omitted_and_future_owners_have_unconstrained_effects_and_lifecycle() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let effects = TestStore::root_terminal_all_effects_checked(domain.borrowed()).unwrap();
    assert!(effects.addresses().is_empty());
    assert!(effects.global_storage_default().numeric().is_top());
    assert!(effects.global_transient_default().numeric().is_top());
    assert!(effects.balance_default().numeric().is_top());
    assert!(effects.code_default().is_none());
    assert!(effects.nonce_default().numeric().is_top());
    assert_eq!(effects.existence_default(), Existence::Unknown);
    assert_eq!(effects.created_default(), None);
    assert_eq!(effects.pending_destruction_default(), None);
    assert_eq!(effects.touched_default(), None);
    for owner in [address(0x101), address(0x202), address(0xffff)] {
        assert!(
            effects
                .read_checked(owner, &word(8), domain.borrowed())
                .unwrap()
                .numeric()
                .is_top()
        );
        assert!(
            effects
                .read_transient_checked(owner, &word(8), domain.borrowed())
                .unwrap()
                .numeric()
                .is_top()
        );
        assert!(effects.storage_default(owner).numeric().is_top());
        assert!(effects.transient_default(owner).numeric().is_top());
        assert!(effects.read_balance(owner).numeric().is_top());
        assert!(effects.nonce(owner).numeric().is_top());
        assert!(effects.code(owner).is_none());
        assert_eq!(effects.existence(owner), Existence::Unknown);
        assert_eq!(effects.created_in_transaction(owner), None);
        assert_eq!(effects.pending_destruction(owner), None);
        assert_eq!(effects.touched_in_transaction(owner), None);
    }
    assert_eq!(effects.symbolic_facts(false).count(), 0);
    assert_eq!(effects.symbolic_facts(true).count(), 0);
    assert!(effects.possible_logs().is_empty());
    assert!(effects.logs_unknown());

    // The same getters describe an ordinary Store; no coverage marker changes
    // what an omitted owner means, and ordinary lifecycle facts stay precise.
    let original = original();
    assert_eq!(original.created_default(), Some(false));
    assert_eq!(original.pending_destruction_default(), Some(false));
    assert_eq!(original.touched_default(), Some(false));
    assert!(original.code_default().is_none());
    assert!(original.nonce_default().numeric().is_top());
    assert_eq!(original.existence_default(), Existence::Unknown);
    assert!(original.code(address(0x101)).is_some());
    assert_eq!(original.nonce(address(0x101)).singleton(), Some(U256::ONE));
    assert_eq!(original.existence(address(0x101)), Existence::Present);
}

#[test]
fn input_saved_history_owner_and_plane_identities_remain_jointly_independent() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let original = original();
    let saved = original.snapshot();
    let mut current = original.clone();
    let old_value = current
        .read_checked(address(0x101), &word(8), domain.borrowed())
        .unwrap();
    current
        .write_checked(address(0x101), &word(9), &old_value, domain.borrowed())
        .unwrap();
    let unchanged = current.clone();
    assert!(current.symbolic_facts(false).count() > 0);

    let effects = TestStore::root_terminal_all_effects_checked(domain.borrowed()).unwrap();
    let later_effects = TestStore::root_terminal_all_effects_checked(domain.borrowed()).unwrap();
    let mut assignment = InputAssignment::default();
    assert!(assignment.cell(&original, address(0x101), false, 7));
    assert!(assignment.cell(saved.state(), address(0x101), false, 7));
    assert!(assignment.cell(&current, address(0x202), false, 7));
    for (owner, value) in [
        (address(0x101), 31),
        (address(0x202), 47),
        (address(0x999), 93),
    ] {
        assert!(assignment.cell(&effects, owner, false, value));
        assert!(assignment.cell(&effects, owner, true, value + 1));
    }
    assert!(assignment.cell(&later_effects, address(0x101), false, 61));
    assert!(!assignment.cell(&original, address(0x101), false, 31));
    assert!(!assignment.cell(saved.state(), address(0x101), false, 31));
    assert!(!assignment.cell(&effects, address(0x101), false, 47));
    assert_eq!(current, unchanged);
    assert_eq!(saved.state(), &original);
}

#[test]
fn repeated_callback_effects_include_new_owners_and_complete_log_payloads() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let effects = TestStore::root_terminal_all_effects_checked(domain.borrowed()).unwrap();
    let mut concrete = original();
    let a = address(0x101);
    let b = address(0x202);
    let new = address(0x999);
    concrete.begin_creation(new);
    concrete.deploy_code(new, Program::from_hex("00").unwrap());
    // Repeated callbacks can revisit the same owner and create an unlisted one.
    // These Store operations are a witness, not a callback CFG/SSA simulation.
    for (owner, value) in [(a, 31), (b, 47), (a, 52), (new, 93)] {
        concrete
            .write_checked(owner, &word(8), &word(value), domain.borrowed())
            .unwrap();
        concrete
            .write_transient_checked(owner, &word(8), &word(value + 1), domain.borrowed())
            .unwrap();
        concrete.write_balance(owner, word(value + 2));
        concrete.write_nonce(owner, word(value + 3));
        concrete.touch_account(owner);
    }
    let payload: Vec<_> = (0..=u8::MAX).cycle().take(513).collect();
    for (owner, pc) in [(a, 7), (b, 13), (a, 7)] {
        concrete
            .emit_log(
                LogKey {
                    address: owner,
                    code_address: owner,
                    code_hash: B256::ZERO,
                    source_kind: LogSourceKind::Runtime,
                    pc,
                },
                TestLog {
                    topics: vec![word(19)],
                    data: TestBytes::exact(&payload),
                },
                domain.borrowed(),
            )
            .unwrap();
    }
    concrete.selfdestruct(new, a, domain.borrowed());
    assert_eq!(concrete.created_in_transaction(new), Some(true));
    assert_eq!(concrete.pending_destruction(new), Some(true));
    assert_eq!(concrete.possible_logs().len(), 2);
    for log in concrete.possible_logs().values() {
        assert_eq!(
            log.data
                .exact_bytes(
                    crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                        .borrowed()
                )
                .as_deref(),
            Some(payload.as_slice())
        );
    }

    let mut assignment = InputAssignment::default();
    for (owner, value) in [(a, 52), (b, 47), (new, 93)] {
        assert!(assignment.cell(&effects, owner, false, value));
        assert!(assignment.cell(&effects, owner, true, value + 1));
        assert!(
            effects
                .read_balance(owner)
                .contains(concrete.read_balance(owner).singleton().unwrap())
        );
        assert!(
            effects
                .nonce(owner)
                .contains(concrete.nonce(owner).singleton().unwrap())
        );
        assert!(effects.code(owner).is_none());
        assert_eq!(effects.existence(owner), Existence::Unknown);
        assert_eq!(effects.created_in_transaction(owner), None);
        assert_eq!(effects.pending_destruction(owner), None);
        assert_eq!(effects.touched_in_transaction(owner), None);
    }
    // The finite observed-site map merges repeated visits. Only logs_unknown
    // admits additional finite ordered event sequences with arbitrary full data.
    assert!(effects.possible_logs().is_empty());
    assert!(effects.logs_unknown());
}

#[test]
fn checked_constructor_preserves_domain_refusal_and_cancellation() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
            .spec()
            .with_relations(RelationLimits {
                max_nodes: 1,
                max_depth: 1,
                ..RelationLimits::default()
            }),
    ));
    assert!(matches!(
        TestStore::root_terminal_all_effects_checked(domain.borrowed()),
        Err(StoreError::State(StateError::Limit { .. }))
    ));
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            TestStore::root_terminal_all_effects_checked(
                crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                    .borrowed()
            ),
            Err(StoreError::State(StateError::Cancelled))
        ));
    });
}

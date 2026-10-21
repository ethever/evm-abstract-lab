//! Joint source-cell witnesses for ordinary detailed transaction finalization.

use alloy_primitives::{Address, U256};
use {
    crate::Fork, crate::domain::symbolic::ExprId, crate::domain::symbolic::ExprKind,
    crate::domain::symbolic::SlotStateId, crate::domain::symbolic::SlotStateKind,
    crate::domain::symbolic::SourceId, crate::resource::WorkBudget,
    crate::world::test_support::TestAccount, crate::world::test_support::TestAlgebra,
    crate::world::test_support::TestValue, crate::world::test_support::TestWorld,
};
use {crate::world::test_support::TestEntry, std::collections::BTreeMap};
use {crate::world::test_support::TestSnapshot, crate::world::test_support::TestStore};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> TestValue {
    TestValue::constant(U256::from(number))
}

/// A single valuation is shared by initial, saved, historical and current
/// roots. Assigning the same immutable source cell twice must agree. These
/// fixtures use only constant keys and unobserved inputs; unsupported syntax
/// fails loudly instead of silently treating a retained term as numerical Top.
/// Each completed witness defines total arrays: every unmentioned cell is zero.
#[derive(Default)]
struct Witness(BTreeMap<(SourceId, U256), U256>);

impl Witness {
    fn expression(&mut self, expression: &ExprId, value: U256) -> bool {
        match expression.kind() {
            ExprKind::Constant(actual) => *actual == value,
            ExprKind::SlotRead { state, key } => self.root(
                state,
                key.as_constant().expect("constant fixture key"),
                value,
            ),
            other => panic!("unexpected witness expression: {other:?}"),
        }
    }

    fn root(&mut self, root: &SlotStateId, key: U256, value: U256) -> bool {
        match root.kind() {
            SlotStateKind::Zero => value == U256::ZERO,
            SlotStateKind::Input { source, bindings } => {
                assert!(bindings.is_empty(), "fixture inputs are unobserved");
                match self.0.entry((source.clone(), key)) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(value);
                        true
                    }
                    std::collections::btree_map::Entry::Occupied(entry) => *entry.get() == value,
                }
            }
            SlotStateKind::Store {
                previous,
                key: written,
                value: stored,
            } => {
                if written.as_constant().expect("constant fixture write") == key {
                    self.expression(stored, value)
                } else {
                    self.root(previous, key, value)
                }
            }
            other => panic!("unexpected witness root: {other:?}"),
        }
    }

    fn cell(&mut self, store: &TestStore, owner: u64, key: u64, value: u64) -> bool {
        self.root(
            &store.slot_root(address(owner), false).unwrap(),
            U256::from(key),
            U256::from(value),
        )
    }
}

struct Fixture {
    initial: TestStore,
    saved: TestSnapshot,
    history: TestStore,
    old_read: TestValue,
}

impl Fixture {
    fn new() -> Self {
        let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
        let mut world = TestWorld::new(Fork::Osaka, "detailed unlisted cleanup");
        world
            .insert(
                address(0x101),
                TestAccount::from_hex("00", world.fork()).unwrap(),
            )
            .unwrap();
        // C=0x303 and D=0x404 stay absent from every source fixture map.
        // The concrete initial world can nevertheless contain both as present,
        // code-empty, nonce-zero, balance-zero accounts, with C[8]=7, D[8]=11.
        let initial = TestStore::new_checked(&world, domain.borrowed()).unwrap();
        let saved = initial.snapshot();
        let old_read = initial
            .read_checked(address(0x303), &word(8), domain.borrowed())
            .unwrap();
        let mut history = initial.clone();
        history
            .write_checked(address(0x101), &word(9), &old_read, domain.borrowed())
            .unwrap();
        let fixture = Self {
            initial,
            saved,
            history,
            old_read,
        };
        for store in [&fixture.initial, fixture.saved.state(), &fixture.history] {
            assert!(!store.addresses().contains(&address(0x303)));
            assert!(!store.addresses().contains(&address(0x404)));
        }
        fixture
    }

    fn initial_valuation(&self) -> Witness {
        let mut witness = Witness::default();
        for store in [&self.initial, self.saved.state(), &self.history] {
            assert!(witness.cell(store, 0x303, 8, 7));
            assert!(witness.cell(store, 0x404, 8, 11));
        }
        assert!(witness.expression(self.old_read.expression().unwrap(), U256::from(7)));
        assert!(witness.cell(&self.history, 0x101, 9, 7));
        witness
    }
}

#[test]
fn unchanged_input_rejects_deleted_current_under_the_same_valuation() {
    let fixture = Fixture::new();
    let mut witness = fixture.initial_valuation();
    assert!(!witness.cell(&fixture.initial, 0x303, 8, 0));
    assert!(!witness.cell(fixture.saved.state(), 0x404, 8, 0));
    assert!(witness.cell(&fixture.initial, 0x303, 8, 7));
}

#[test]
fn unknown_static_finalization_admits_all_joint_unlisted_cleanup_choices() {
    let fixture = Fixture::new();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut current = fixture.history.clone();
    current
        .unknown_effects(&current.addresses(), None, true, domain.borrowed())
        .unwrap();
    for owner in [0x303, 0x404] {
        assert!(!current.addresses().contains(&address(owner)));
    }
    current
        .finalize_transaction_checked(domain.borrowed(), None, &mut WorkBudget::new(20_000_000))
        .unwrap();
    for (c, d) in [(0, 0), (0, 11), (7, 0), (7, 11)] {
        // A complete new environment per alternative, with the SAME fixed
        // initial/saved/history values inside each environment. Mixed tuples
        // rule out a single shared all-owner cleanup selector.
        let mut witness = fixture.initial_valuation();
        assert!(witness.cell(&current, 0x303, 8, c), "C={c}, D={d}");
        assert!(witness.cell(&current, 0x404, 8, d), "C={c}, D={d}");
        assert!(witness.cell(&current, 0x101, 9, 7));
        assert!(witness.expression(fixture.old_read.expression().unwrap(), U256::from(7)));
        assert!(!witness.cell(&current, 0x303, 8, if c == 0 { 7 } else { 0 }));
        assert!(!witness.cell(&current, 0x404, 8, if d == 0 { 11 } else { 0 }));
        assert!(witness.cell(&fixture.initial, 0x303, 8, 7));
        assert!(witness.cell(fixture.saved.state(), 0x404, 8, 11));
    }
}

#[test]
fn explicit_nonempty_owner_and_historical_facts_survive_global_base_forgetting() {
    let fixture = Fixture::new();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut current = fixture.history.clone();
    current
        .unknown_effects(&current.addresses(), None, true, domain.borrowed())
        .unwrap();
    let before = current.clone();
    current
        .prepare_unlisted_finalization(domain.borrowed())
        .unwrap();
    let known = address(0x101);
    assert_eq!(
        current.slot_root(known, false),
        before.slot_root(known, false)
    );
    assert_eq!(
        current.storage_default(known),
        before.storage_default(known)
    );
    assert_eq!(current.slots(), before.slots());
    assert_eq!(current.persistent.operands, before.persistent.operands);
    assert_eq!(
        current
            .persistent
            .initial_independence
            .at(known, U256::from(9)),
        before
            .persistent
            .initial_independence
            .at(known, U256::from(9))
    );
    assert!(current.persistent.initial_independence.global_default);
    assert!(!before.persistent.initial_independence.global_default);
    assert_ne!(
        current.global_storage_state(),
        before.global_storage_state()
    );
    let mut other_fields = current.clone();
    other_fields.persistent = before.persistent.clone();
    assert_eq!(other_fields, before);

    current
        .finalize_transaction_checked(domain.borrowed(), None, &mut WorkBudget::new(20_000_000))
        .unwrap();
    assert_eq!(
        current.slot_root(known, false),
        before.slot_root(known, false)
    );
    assert_eq!(
        current.storage_default(known),
        before.storage_default(known)
    );
    assert_eq!(current.slots(), before.slots());
    assert_eq!(current.persistent.operands, before.persistent.operands);
    let mut witness = fixture.initial_valuation();
    assert!(witness.cell(&current, 0x303, 8, 0));
    assert!(witness.cell(&current, 0x404, 8, 11));
    assert!(witness.cell(&current, 0x101, 9, 7));
}

#[test]
fn rollback_restores_global_flags_before_deciding_whether_to_forget() {
    let fixture = Fixture::new();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut current = fixture.history.clone();
    current
        .unknown_effects(&current.addresses(), None, true, domain.borrowed())
        .unwrap();
    current
        .finalize_transaction_checked(
            domain.borrowed(),
            Some(&fixture.saved),
            &mut WorkBudget::new(20_000_000),
        )
        .unwrap();
    assert_eq!(
        current.global_storage_state(),
        fixture.saved.state().global_storage_state()
    );
    for owner in [0x303, 0x404] {
        assert_eq!(
            current.slot_root(address(owner), false),
            fixture.saved.state().slot_root(address(owner), false)
        );
    }
    let mut witness = fixture.initial_valuation();
    assert!(!witness.cell(&current, 0x303, 8, 0));
    assert!(witness.cell(&current, 0x303, 8, 7));
    assert_eq!(fixture.saved.state(), &fixture.initial);
}

#[test]
fn untouched_global_defaults_do_not_forget_and_finalization_is_idempotent() {
    let fixture = Fixture::new();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut untouched = fixture.history.clone();
    untouched.finalize_transaction(domain.borrowed()).unwrap();
    assert_eq!(
        untouched.global_storage_state(),
        fixture.history.global_storage_state()
    );
    assert_eq!(untouched.persistent, fixture.history.persistent);

    let mut current = fixture.history.clone();
    current
        .unknown_effects(&current.addresses(), None, true, domain.borrowed())
        .unwrap();
    current.finalize_transaction(domain.borrowed()).unwrap();
    let once = current.clone();
    current
        .finalize_transaction_checked(domain.borrowed(), None, &mut WorkBudget::new(20_000_000))
        .unwrap();
    assert_eq!(current, once);
}

#[test]
fn global_pending_destruction_alone_forgets_unlisted_persistent_storage() {
    let fixture = Fixture::new();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut current = fixture.history.clone();
    assert_eq!(current.touched_default(), Some(false));
    current.pending_destruction_default = None;
    current
        .finalize_transaction_checked(domain.borrowed(), None, &mut WorkBudget::new(20_000_000))
        .unwrap();
    let mut witness = fixture.initial_valuation();
    assert!(witness.cell(&current, 0x303, 8, 0));
    assert!(witness.cell(&current, 0x404, 8, 11));
    assert_eq!(current.pending_destruction_default(), Some(false));
    let once = current.clone();
    current.finalize_transaction(domain.borrowed()).unwrap();
    assert_eq!(current, once);
}

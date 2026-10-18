//! Joint source-cell witnesses for ordinary detailed transaction finalization.

use super::super::{Snapshot, Store};
use crate::{
    Fork,
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, ExprKind, SlotStateId, SlotStateKind, SourceId},
    },
    resource::WorkBudget,
    world::{Account, World},
};
use alloy_primitives::{Address, U256};
use std::collections::{BTreeMap, btree_map::Entry};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

/// A single valuation is shared by initial, saved, historical and current
/// roots. Assigning the same immutable source cell twice must agree. These
/// fixtures use only constant keys and unobserved inputs; unsupported syntax
/// fails loudly instead of silently treating a retained term as numerical Top.
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
                    Entry::Vacant(entry) => {
                        entry.insert(value);
                        true
                    }
                    Entry::Occupied(entry) => *entry.get() == value,
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

    fn cell(&mut self, store: &Store, owner: u64, key: u64, value: u64) -> bool {
        self.root(
            &store.slot_root(address(owner), false).unwrap(),
            U256::from(key),
            U256::from(value),
        )
    }
}

struct Fixture {
    initial: Store,
    saved: Snapshot,
    history: Store,
    old_read: AbstractValue,
}

impl Fixture {
    fn new() -> Self {
        let domain = Domain::default();
        let mut world = World::new(Fork::Osaka, "detailed unlisted cleanup");
        world
            .insert(
                address(0x101),
                Account::from_hex("00", world.fork()).unwrap(),
            )
            .unwrap();
        // C=0x303 and D=0x404 stay absent from every source fixture map.
        // The concrete initial world can nevertheless contain both as present,
        // code-empty, nonce-zero, balance-zero accounts, with C[8]=7, D[8]=11.
        let initial = Store::new_checked(&world, domain).unwrap();
        let saved = initial.snapshot();
        let old_read = initial
            .read_checked(address(0x303), &word(8), domain)
            .unwrap();
        let mut history = initial.clone();
        history
            .write_checked(address(0x101), &word(9), &old_read, domain)
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
    let domain = Domain::default();
    let mut current = fixture.history.clone();
    current
        .unknown_effects(&current.addresses(), None, true, domain)
        .unwrap();
    for owner in [0x303, 0x404] {
        assert!(!current.addresses().contains(&address(owner)));
    }
    current
        .finalize_transaction_checked(domain, None, &mut WorkBudget::new(20_000_000))
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

//! Production projection/replay, independent snapshots and non-layout effects.

use super::*;
use crate::{
    analysis::{machine::fixtures, transfer},
    domain::{AbstractValue, symbolic::ExprId},
};
use alloy_primitives::U256;

#[test]
fn materializing_a_canonical_loop_bound_is_idempotent_without_reusing_sources() {
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut old = transfer::initial(&world, &entry, domain).unwrap();
    old.active_mut().stack = vec![AbstractValue::top().with_expression(ExprId::fresh().unwrap())];
    old.store
        .write_checked(
            fixtures::address(0x101),
            &fixtures::word(0),
            &fixtures::word(7),
            domain,
        )
        .unwrap();
    let mut next = old.clone();
    next.store
        .write_checked(
            fixtures::address(0x101),
            &fixtures::word(0),
            &fixtures::word(9),
            domain,
        )
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, live) = LoopBound::new(&old, &next, &entry, domain, &mut budget).unwrap();
    assert!(
        bound
            .update(&live, &entry, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    let sibling = instantiate(&bound.bound, domain).unwrap();
    assert_ne!(
        live.store
            .slot_root(fixtures::address(0x101), false)
            .unwrap(),
        sibling
            .store
            .slot_root(fixtures::address(0x101), false)
            .unwrap()
    );
    let current = live
        .store
        .read(fixtures::address(0x101), &fixtures::word(0), domain);
    assert!(current.contains(U256::from(7)) && current.contains(U256::from(9)));
    let saved = live.active().saved_store.state().read(
        fixtures::address(0x101),
        &fixtures::word(0),
        domain,
    );
    assert_eq!(saved.singleton(), Some(U256::ZERO));
    assert_ne!(
        live.store
            .slot_root(fixtures::address(0x101), false)
            .unwrap(),
        live.active()
            .saved_store
            .state()
            .slot_root(fixtures::address(0x101), false)
            .unwrap()
    );
}

#[test]
fn later_locations_flow_into_defaults_and_do_not_grow_the_loop_layout() {
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let old = transfer::initial(&world, &entry, domain).unwrap();
    let mut next = old.clone();
    let owner = fixtures::address(0x101);
    next.store
        .write_checked(owner, &fixtures::word(0), &fixtures::word(7), domain)
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, mut live) = LoopBound::new(&old, &next, &entry, domain, &mut budget).unwrap();
    live.store
        .write_checked(owner, &fixtures::word(999), &fixtures::word(99), domain)
        .unwrap();
    let (wider, live) = bound
        .update(&live, &entry, domain, &mut budget)
        .unwrap()
        .unwrap();
    assert!(
        live.store
            .read(owner, &fixtures::word(999), domain)
            .contains(U256::from(99))
    );
    assert_eq!(wider.layout.store, bound.layout.store);
    let mut another = live.clone();
    another
        .store
        .write_checked(owner, &fixtures::word(1000), &fixtures::word(101), domain)
        .unwrap();
    assert!(
        wider
            .update(&another, &entry, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    assert!(
        live.store
            .read(owner, &fixtures::word(1000), domain)
            .contains(U256::from(101))
    );
}

#[test]
fn rejected_loop_projection_leaves_inputs_and_old_bound_unchanged() {
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let old = transfer::initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, live) = LoopBound::new(&old, &old, &entry, domain, &mut budget).unwrap();
    let before = bound.bound.clone();
    let mut budget = WorkBudget::new(0);
    assert!(matches!(
        bound.update(&live, &entry, domain, &mut budget),
        Err(MachineError::Work)
    ));
    assert!(semantic_eq(&bound.bound, &before));
    assert_eq!(
        old.active()
            .saved_store
            .state()
            .read(fixtures::address(0x101), &fixtures::word(0), domain)
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn memory_and_transient_bounds_keep_both_inputs_and_roundtrip_stably() {
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut old = transfer::initial(&world, &entry, domain).unwrap();
    let owner = fixtures::address(0x101);
    old.active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(7), 4096, domain)
        .unwrap();
    old.store
        .write_transient_checked(owner, &fixtures::word(0), &fixtures::word(7), domain)
        .unwrap();
    let mut next = old.clone();
    next.active_mut()
        .memory
        .write_word(&fixtures::word(0), &fixtures::word(9), 4096, domain)
        .unwrap();
    next.store
        .write_transient_checked(owner, &fixtures::word(0), &fixtures::word(9), domain)
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, live) = LoopBound::new(&old, &next, &entry, domain, &mut budget).unwrap();
    let memory = live.active().memory.read_word(&fixtures::word(0), domain);
    let transient = live.store.read_transient(owner, &fixtures::word(0), domain);
    for value in [7, 9] {
        assert!(memory.contains(U256::from(value)));
        assert!(transient.contains(U256::from(value)));
    }
    assert!(
        bound
            .update(&live, &entry, domain, &mut budget)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        live.active()
            .saved_store
            .state()
            .read_transient(owner, &fixtures::word(0), domain)
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn unknown_alias_writes_weaken_retained_cells_as_well_as_the_default() {
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut old = transfer::initial(&world, &entry, domain).unwrap();
    let owner = fixtures::address(0x101);
    old.store
        .write_checked(owner, &fixtures::word(0), &fixtures::word(7), domain)
        .unwrap();
    let mut next = old.clone();
    next.store
        .write_checked(owner, &fixtures::word(0), &fixtures::word(9), domain)
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, mut live) = LoopBound::new(&old, &next, &entry, domain, &mut budget).unwrap();
    let key = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    live.store
        .write_checked(owner, &key, &fixtures::word(99), domain)
        .unwrap();
    let (_, live) = bound
        .update(&live, &entry, domain, &mut budget)
        .unwrap()
        .unwrap();
    for value in [7, 9, 99] {
        assert!(
            live.store
                .read(owner, &fixtures::word(0), domain)
                .contains(U256::from(value))
        );
    }
    assert!(
        live.store
            .read(owner, &fixtures::word(999), domain)
            .contains(U256::from(99))
    );
    assert_eq!(
        live.active()
            .saved_store
            .state()
            .read(owner, &fixtures::word(0), domain)
            .singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn out_of_layout_touches_and_log_sources_enter_complete_envelopes() {
    use crate::world::{AbstractLog, ByteArray, LogKey, LogSourceKind};
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let old = transfer::initial(&world, &entry, domain).unwrap();
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, mut live) = LoopBound::new(&old, &old, &entry, domain, &mut budget).unwrap();
    let owner = fixtures::address(0x101);
    let other = fixtures::address(0x200);
    let code_hash = live.active().key.code_hash;
    live.store.may_touch_account(other);
    live.store
        .emit_log(
            LogKey {
                address: owner,
                code_address: owner,
                code_hash,
                source_kind: LogSourceKind::Runtime,
                pc: 0,
            },
            AbstractLog {
                topics: vec![],
                data: ByteArray::empty(),
            },
            domain,
        )
        .unwrap();
    let (_, mut live) = bound
        .update(&live, &entry, domain, &mut budget)
        .unwrap()
        .unwrap();
    assert_ne!(live.store.touched_in_transaction(other), Some(false));
    assert!(live.store.logs_unknown());
    assert!(live.store.possible_logs().is_empty());
    // The consensus sticky touch survives restoration from an independent bound.
    let ripemd = fixtures::address(3);
    live.store.touch_account(ripemd);
    let snapshot = live.active().saved_store.clone();
    live.store.restore(snapshot);
    assert_eq!(live.store.touched_in_transaction(ripemd), Some(true));
}

#[test]
fn expression_retention_refusal_retries_a_complete_coarse_bound() {
    use crate::domain::relational::RelationLimits;
    let domain = Domain::default();
    let (world, entry) = fixtures::fixture(&[(0x101, "00")]);
    let mut old = transfer::initial(&world, &entry, domain).unwrap();
    let owner = fixtures::address(0x101);
    let value = AbstractValue::unsigned_range(U256::from(7), U256::from(9)).unwrap();
    for key in 0..8 {
        old.store
            .write_checked(owner, &fixtures::word(key), &value, domain)
            .unwrap();
    }
    let smaller = Domain::from_spec(domain.spec().with_relations(RelationLimits {
        max_nodes: 24,
        ..domain.spec().relations()
    }));
    let summary = form(&old, &entry, smaller).unwrap();
    assert!(
        instantiate(&summary, smaller).is_err(),
        "fixture must exercise retained expression refusal"
    );
    let mut budget = WorkBudget::new(1_000_000_000_000);
    let (bound, live) = LoopBound::new(&old, &old, &entry, smaller, &mut budget).unwrap();
    for key in 0..8 {
        for value in [7, 8, 9] {
            assert!(
                live.store
                    .read(owner, &fixtures::word(key), smaller)
                    .contains(U256::from(value))
            );
        }
    }
    assert_eq!(live.key(), old.key());
    assert!(live.store.logs_unknown());
    assert!(
        bound
            .update(&live, &entry, smaller, &mut budget)
            .unwrap()
            .is_none()
    );
}

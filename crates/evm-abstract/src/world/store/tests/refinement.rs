//! Path projection across account planes, scalars, log bytes and rollback.

use super::super::*;
use revm_bytecode::opcode;
use std::collections::{BTreeMap, BTreeSet};
use {
    crate::Fork, crate::domain::relational::CheckResult, crate::domain::relational::EvmValueQuery,
    crate::domain::relational::QueryReason, crate::domain::relational::RelationLimits,
    crate::domain::relational::RelationState, crate::world::GuardRefinementError,
    crate::world::test_support::TestAccount,
};

fn v(n: u64) -> TestValue {
    TestValue::constant(U256::from(n))
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        ..RelationLimits::default()
    }
}
fn truth(relations: &RelationState, e: &ExprId) -> Result<Option<bool>, QueryReason> {
    let p = policy();
    let zero = ExprId::operation(opcode::ISZERO, std::slice::from_ref(e), p.expressions())
        .map_err(|error| match error {
            ExprError::Cancelled => QueryReason::Cancelled,
            ExprError::Limit { .. } => QueryReason::ExpressionLimit,
            error => panic!("ISZERO construction failed: {error:?}"),
        })?;
    match relations.local_value(
        &zero,
        &p,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    ) {
        EvmValueQuery::Exact(v) if v == U256::ZERO => Ok(Some(true)),
        EvmValueQuery::Exact(v) if v == U256::ONE => Ok(Some(false)),
        EvmValueQuery::Bounds(bounds) => {
            // ISZERO is intrinsically Boolean, even when e is an arbitrary
            // nonzero EVM word rather than the canonical word one.
            assert!(bounds.contains(U256::ZERO));
            assert!(bounds.contains(U256::ONE));
            Ok(None)
        }
        EvmValueQuery::Undetermined(reason) => Err(reason),
        EvmValueQuery::ProvenImpossible => {
            panic!("guard query requires a reachable relation state")
        }
        EvmValueQuery::Exact(value) => panic!("ISZERO produced a non-Boolean word: {value}"),
    }
}

fn logged_store(
    world: &TestWorld,
    site: &LogKey,
    topic: u64,
    data: TestBytes,
    domain: TestAlgebra,
) -> TestStore {
    let mut store = TestStore::new(
        world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store
        .emit_log(
            site.clone(),
            TestLog {
                topics: vec![v(topic)],
                data,
            },
            domain.borrowed(),
        )
        .unwrap();
    store
}

#[test]
fn native_guard_projection_pairs_slots_balances_logs_and_restores_old_snapshot() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let address = Address::repeat_byte(0x21);
    let mut world = TestWorld::new(Fork::Osaka, "refine leaf");
    world.insert(address, TestAccount::empty()).unwrap();
    let site = LogKey {
        address,
        code_address: address,
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: crate::world::LogSourceKind::Runtime,
        pc: 7,
    };
    let old = logged_store(&world, &site, 3, TestBytes::exact(&[3]), domain);
    let mut changed = old.clone();
    changed
        .write_checked(address, &v(7), &v(42), domain.borrowed())
        .unwrap();
    changed
        .write_transient_checked(address, &v(8), &v(19), domain.borrowed())
        .unwrap();
    changed.write_balance(address, v(10));
    changed.write_nonce(address, v(9));
    let changed_log = logged_store(&world, &site, 1, TestBytes::exact(&[1]), domain);
    changed.possible_logs = changed_log.possible_logs;
    let guard = ExprId::fresh().unwrap();
    let joined = changed
        .join_guarded(
            &old,
            domain.borrowed(),
            guard.clone(),
            policy().expressions(),
        )
        .unwrap();
    let read_before = joined.read(address, &v(7), domain.borrowed());
    for (nonzero, persistent, transient, balance, nonce, byte) in
        [(true, 42, 19, 10, 9, 1), (false, 0, 0, 0, 0, 3)]
    {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&guard, nonzero, &policy(), domain.borrowed()),
            CheckResult::MayBeFeasible
        );
        let mut selected = joined.clone();
        let snapshot = selected.snapshot();
        assert!(
            selected
                .refine_guards(domain.borrowed(), policy().expressions(), &mut |e| truth(
                    &relations, e
                ))
                .unwrap()
        );
        assert_eq!(
            selected.slot_root(address, false).unwrap(),
            joined.slot_root(address, false).unwrap()
        );
        assert_eq!(
            selected.read(address, &v(7), domain.borrowed()).singleton(),
            Some(U256::from(persistent))
        );
        assert_eq!(
            selected
                .read_transient(address, &v(8), domain.borrowed())
                .singleton(),
            Some(U256::from(transient))
        );
        assert_eq!(
            selected.read_balance(address).singleton(),
            Some(U256::from(balance))
        );
        assert_eq!(selected.nonce(address).singleton(), Some(U256::from(nonce)));
        let event = &selected.possible_logs()[&site];
        assert_eq!(event.topics[0].singleton(), Some(U256::from(byte)));
        assert_eq!(
            event.data.byte_at(0, domain.borrowed()).singleton(),
            Some(U256::from(byte))
        );
        assert!(read_before.singleton().is_none());
        selected.restore(snapshot);
        assert_eq!(selected, joined);
    }
}

#[test]
fn conditional_projection_does_not_create_rpc_observations_and_errors_are_atomic() {
    let address = Address::repeat_byte(0x31);
    let mut world = TestWorld::new(Fork::Osaka, "unknown baseline");
    world.insert(address, TestAccount::unknown()).unwrap();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let old = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut written = old.clone();
    written
        .write_checked(address, &v(7), &v(9), domain.borrowed())
        .unwrap();
    let guard = ExprId::fresh().unwrap();
    let joined = written
        .join_guarded(
            &old,
            domain.borrowed(),
            guard.clone(),
            policy().expressions(),
        )
        .unwrap();
    let initial_requests = joined.missing_initial_slots(&world, address, &v(7));
    let mut selected = joined.clone();
    let mut relations = RelationState::default();
    assert_eq!(
        relations.assume(&guard, true, &policy(), domain.borrowed()),
        CheckResult::MayBeFeasible
    );
    assert!(
        selected
            .refine_guards(domain.borrowed(), policy().expressions(), &mut |e| truth(
                &relations, e
            ))
            .unwrap()
    );
    assert_eq!(
        selected.read(address, &v(7), domain.borrowed()).singleton(),
        Some(U256::from(9))
    );
    assert!(world.account(address).unwrap().storage.is_empty());
    assert_eq!(
        selected.missing_initial_slots(&world, address, &v(7)),
        initial_requests
    );
    let saved = selected.clone();
    assert_eq!(
        selected.refine_guards(domain.borrowed(), policy().expressions(), &mut |_| Err(
            QueryReason::Cancelled
        )),
        Err(GuardRefinementError::Proof(QueryReason::Cancelled))
    );
    assert_eq!(selected, saved);
}

struct CollectorFixture {
    world: TestWorld,
    owners: [Address; 2],
    site: LogKey,
    selector: ExprId,
    expected: BTreeSet<ExprId>,
    base_guard: ExprId,
    data_guards: BTreeSet<ExprId>,
    base_roots_only: TestStore,
    log_roots_only: TestStore,
    joined: TestStore,
}

fn scaled_guard(selector: &ExprId, factor: u64) -> ExprId {
    ExprId::operation(
        opcode::MUL,
        &[selector.clone(), ExprId::constant(U256::from(factor))],
        policy().expressions(),
    )
    .unwrap()
}

fn guarded_change(
    before: &TestStore,
    guard: &ExprId,
    change: impl FnOnce(&mut TestStore),
) -> TestStore {
    let mut after = before.clone();
    change(&mut after);
    after
        .join_guarded(
            before,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
            guard.clone(),
            policy().expressions(),
        )
        .unwrap()
}

fn guarded_base(world: &TestWorld, guard: &ExprId) -> TestStore {
    let initial = TestStore::new(
        world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut havoc = initial.clone();
    havoc.havoc_all();
    havoc
        .join_guarded(
            &initial,
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
            guard.clone(),
            policy().expressions(),
        )
        .unwrap()
}

fn collector_fixture() -> CollectorFixture {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let owners = [Address::repeat_byte(0x41), Address::repeat_byte(0x42)];
    let code_owner = Address::repeat_byte(0x43);
    let site = LogKey {
        address: owners[0],
        code_address: code_owner,
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: crate::world::LogSourceKind::Runtime,
        pc: 11,
    };
    let mut world = TestWorld::new(Fork::Osaka, "guard collector owners");
    world.insert(owners[0], TestAccount::empty()).unwrap();
    world.insert(owners[1], TestAccount::unknown()).unwrap();
    world.insert(code_owner, TestAccount::empty()).unwrap();

    let selector = ExprId::fresh().unwrap();
    let base_guard = selector.clone();
    let persistent_a = scaled_guard(&selector, 2);
    let transient_a = scaled_guard(&selector, 3);
    let persistent_b = scaled_guard(&selector, 4);
    let transient_b = scaled_guard(&selector, 5);
    let scalar_guard = scaled_guard(&selector, 6);
    let topic_guard = scaled_guard(&selector, 7);
    let data_inner = scaled_guard(&selector, 8);
    let data_outer = scaled_guard(&selector, 9);
    // These identities come from the independently chosen branch selectors,
    // rather than another traversal of the wrapper under test.
    let expected = BTreeSet::from([
        base_guard.clone(),
        persistent_a.clone(),
        transient_a.clone(),
        persistent_b.clone(),
        transient_b.clone(),
        scalar_guard.clone(),
        topic_guard.clone(),
        data_inner.clone(),
        data_outer.clone(),
    ]);
    assert_eq!(expected.len(), 9);

    let roots_world = TestWorld::new(Fork::Osaka, "guard collector roots only");
    let base_roots_only = guarded_base(&roots_world, &base_guard);
    let mut joined = guarded_base(&world, &base_guard);
    for (address, persistent, transient, balance, nonce) in
        [(owners[0], 0, 0, 0, 0), (owners[1], 24, 5, 4, 2)]
    {
        joined
            .write_checked(address, &v(7), &v(persistent), domain.borrowed())
            .unwrap();
        joined
            .write_transient_checked(address, &v(8), &v(transient), domain.borrowed())
            .unwrap();
        joined.write_balance(address, v(balance));
        joined.write_nonce(address, v(nonce));
    }
    for (address, persistent, transient, persistent_guard, transient_guard) in [
        (owners[0], 42, 19, &persistent_a, &transient_a),
        (owners[1], 52, 29, &persistent_b, &transient_b),
    ] {
        joined = guarded_change(&joined, persistent_guard, |store| {
            store
                .write_checked(address, &v(7), &v(persistent), domain.borrowed())
                .unwrap();
        });
        joined = guarded_change(&joined, transient_guard, |store| {
            store
                .write_transient_checked(address, &v(8), &v(transient), domain.borrowed())
                .unwrap();
        });
    }
    joined = guarded_change(&joined, &scalar_guard, |store| {
        store.write_balance(owners[0], v(10));
        store.write_nonce(owners[0], v(9));
        store.write_balance(owners[1], v(20));
        store.write_nonce(owners[1], v(12));
    });

    // Exact data supplies roots without first materializing symbolic byte reads.
    // Equal lengths and constant topics leave only data roots carrying these guards.
    let first = logged_store(
        &roots_world,
        &site,
        3,
        TestBytes::exact(&[1, 11, 21]),
        domain,
    );
    let second = logged_store(
        &roots_world,
        &site,
        3,
        TestBytes::exact(&[2, 12, 22]),
        domain,
    );
    let inner = first
        .join_guarded(
            &second,
            domain.borrowed(),
            data_inner.clone(),
            policy().expressions(),
        )
        .unwrap();
    let third = logged_store(
        &roots_world,
        &site,
        3,
        TestBytes::exact(&[3, 13, 23]),
        domain,
    );
    let log_roots_only = inner
        .join_guarded(
            &third,
            domain.borrowed(),
            data_outer.clone(),
            policy().expressions(),
        )
        .unwrap();
    let nested_data = log_roots_only.possible_logs()[&site].data.clone();
    let changed_topic = logged_store(&roots_world, &site, 1, nested_data, domain);
    let logs = changed_topic
        .join_guarded(
            &log_roots_only,
            domain.borrowed(),
            topic_guard,
            policy().expressions(),
        )
        .unwrap();
    // As in the native fixture, install log state built by actual emit/join operations.
    joined.possible_logs = logs.possible_logs;

    CollectorFixture {
        world,
        owners,
        site,
        selector,
        expected,
        base_guard,
        data_guards: BTreeSet::from([data_inner, data_outer]),
        base_roots_only,
        log_roots_only,
        joined,
    }
}

fn fixture_reads(fixture: &CollectorFixture, store: &TestStore) -> Vec<TestValue> {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut reads = Vec::new();
    for address in fixture.owners {
        reads.push(store.read(address, &v(7), domain.borrowed()));
        reads.push(store.read_transient(address, &v(8), domain.borrowed()));
        reads.push(store.read_balance(address));
        reads.push(store.nonce(address));
    }
    let log = &store.possible_logs()[&fixture.site];
    reads.push(log.topics[0].clone());
    for offset in 0..3 {
        reads.push(log.data.byte_at(offset, domain.borrowed()));
    }
    reads
}

#[test]
fn store_guard_collection_is_read_only_and_complete_on_live_and_restored_roots() {
    let fixture = collector_fixture();
    let limits = policy().expressions();
    assert_ne!(fixture.site.address, fixture.site.code_address);
    for address in fixture.owners {
        assert!(fixture.world.account(address).is_some());
        assert!(fixture.joined.slot_root(address, false).is_ok());
        assert!(fixture.joined.slot_root(address, true).is_ok());
    }
    assert!(fixture.world.account(fixture.site.code_address).is_some());

    let base_before = fixture.base_roots_only.clone();
    let mut base_values = 0;
    fixture.base_roots_only.visit_values(&mut |value| {
        base_values += 1;
        assert!(value.expression().is_none_or(|expression| !matches!(
            expression.kind(),
            crate::domain::symbolic::ExprKind::Ite { .. }
        )));
    });
    assert!(
        base_values > 0,
        "defaults and root operands are actually visited"
    );
    assert_eq!(
        fixture.base_roots_only.guard_conditions(limits).unwrap(),
        BTreeSet::from([fixture.base_guard.clone()])
    );
    assert_eq!(fixture.base_roots_only, base_before);

    let logs_before = fixture.log_roots_only.clone();
    let mut log_values = 0;
    fixture.log_roots_only.visit_values(&mut |value| {
        log_values += 1;
        assert!(value.expression().is_none_or(|expression| !matches!(
            expression.kind(),
            crate::domain::symbolic::ExprKind::Ite { .. }
        )));
    });
    assert!(
        log_values > 0,
        "the constant topic must actually be visited"
    );
    assert_eq!(
        fixture.log_roots_only.guard_conditions(limits).unwrap(),
        fixture.data_guards
    );
    assert_eq!(fixture.log_roots_only, logs_before);

    let mut current = fixture.joined.clone();
    let checkpoint = current.snapshot();
    let later_guard = ExprId::fresh().unwrap();
    current = guarded_change(&current, &later_guard, |store| {
        store
            .write_checked(
                fixture.owners[0],
                &v(99),
                &v(88),
                crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                    .borrowed(),
            )
            .unwrap();
    });
    let mut expected_current = fixture.expected.clone();
    assert!(expected_current.insert(later_guard));
    let current_before = current.clone();
    assert_eq!(current.guard_conditions(limits).unwrap(), expected_current);
    assert_eq!(current, current_before);

    let mut historical = current.clone();
    historical.restore(checkpoint);
    assert_eq!(historical, fixture.joined);
    assert_ne!(historical, current);
    let historical_before = historical.clone();
    assert_eq!(
        historical.guard_conditions(limits).unwrap(),
        fixture.expected
    );
    assert_eq!(historical, historical_before);
    assert_eq!(current, current_before);
}

#[test]
fn store_guard_collection_serial_proofs_match_native_projection_on_frozen_wrapper() {
    let fixture = collector_fixture();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = policy().expressions();
    let frozen = fixture.joined.clone();
    let collected = frozen.guard_conditions(limits).unwrap();
    assert_eq!(collected, fixture.expected);
    let expected_order: Vec<_> = fixture.expected.iter().cloned().collect();
    let historical_reads = fixture_reads(&fixture, &frozen);
    let historical_read = frozen.read(fixture.owners[1], &v(99), domain.borrowed());
    let initial_requests = frozen.missing_initial_slots(&fixture.world, fixture.owners[1], &v(99));
    assert!(!initial_requests.is_empty());

    for selector_value in [Some(0_u64), Some(3_u64), None] {
        let mut relations = RelationState::default();
        if let Some(value) = selector_value {
            let equal = ExprId::operation(
                opcode::EQ,
                &[
                    fixture.selector.clone(),
                    ExprId::constant(U256::from(value)),
                ],
                limits,
            )
            .unwrap();
            assert_eq!(
                relations.assume(&equal, true, &policy(), domain.borrowed()),
                CheckResult::MayBeFeasible
            );
            assert!(matches!(
                relations.local_value(&fixture.selector, &policy(), domain.borrowed()),
                EvmValueQuery::Exact(actual) if actual == U256::from(value)
            ));
        }

        let mut proved = BTreeMap::new();
        let mut proof_order = Vec::new();
        for guard in &collected {
            proof_order.push(guard.clone());
            let result = truth(&relations, guard).unwrap();
            assert_eq!(result, selector_value.map(|value| value != 0));
            assert!(proved.insert(guard.clone(), result).is_none());
        }
        assert_eq!(proof_order, expected_order);

        let mut projected = frozen.clone();
        let projected_checkpoint = projected.snapshot();
        let mut lookup_order = Vec::new();
        let projected_feasible = projected
            .refine_guards(domain.borrowed(), limits, &mut |guard| {
                lookup_order.push(guard.clone());
                Ok::<_, QueryReason>(
                    *proved
                        .get(guard)
                        .expect("proof for the exact frozen wrapper"),
                )
            })
            .unwrap();
        let mut native = frozen.clone();
        let native_checkpoint = native.snapshot();
        let mut native_order = Vec::new();
        let native_feasible = native
            .refine_guards(domain.borrowed(), limits, &mut |guard| {
                native_order.push(guard.clone());
                truth(&relations, guard)
            })
            .unwrap();
        assert_eq!(lookup_order, expected_order);
        assert_eq!(native_order, expected_order);
        assert_eq!(projected_feasible, native_feasible);
        assert!(
            projected_feasible,
            "unknown selectors retain a feasible wrapper"
        );
        assert_eq!(projected, native);

        let projected_reads = fixture_reads(&fixture, &projected);
        assert_eq!(projected_reads, fixture_reads(&fixture, &native));
        let expected_reads: Option<[u64; 12]> = selector_value.map(|value| {
            if value == 0 {
                [0, 0, 0, 0, 24, 5, 4, 2, 3, 3, 13, 23]
            } else {
                [42, 19, 10, 9, 52, 29, 20, 12, 1, 1, 11, 21]
            }
        });
        for (index, read) in projected_reads.iter().enumerate() {
            assert_eq!(
                read.singleton(),
                expected_reads.map(|values| U256::from(values[index]))
            );
        }
        if selector_value.is_none() {
            // The existing projector refreshes mutable caches even when every
            // proof is unknown; its observable reads retain the same facts.
            assert_eq!(projected_reads, historical_reads);
        }
        for address in fixture.owners {
            for transient in [false, true] {
                assert_eq!(
                    projected.slot_root(address, transient),
                    frozen.slot_root(address, transient)
                );
            }
        }
        for wrapper in [&projected, &native, &frozen] {
            assert_eq!(
                wrapper.missing_initial_slots(&fixture.world, fixture.owners[1], &v(99)),
                initial_requests
            );
        }
        // Frozen roots and values retain the historical expression/source identities.
        assert_eq!(fixture_reads(&fixture, &frozen), historical_reads);
        assert_eq!(
            frozen.read(fixture.owners[1], &v(99), domain.borrowed()),
            historical_read
        );
        assert!(
            fixture
                .world
                .account(fixture.owners[1])
                .unwrap()
                .storage
                .is_empty()
        );

        projected.restore(projected_checkpoint);
        native.restore(native_checkpoint);
        assert_eq!(projected, frozen);
        assert_eq!(native, frozen);
        assert_eq!(fixture_reads(&fixture, &projected), historical_reads);
        assert_eq!(
            projected.guard_conditions(limits).unwrap(),
            fixture.expected
        );
    }
}

#[test]
fn store_guard_collection_limits_and_serial_cancellation_preserve_wrappers() {
    let fixture = collector_fixture();
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = policy().expressions();
    let tiny = RelationLimits {
        max_nodes: 1,
        max_depth: 1,
        ..policy()
    }
    .expressions();
    let historical = fixture.joined.clone();
    let mut current = historical.clone();
    let checkpoint = current.snapshot();
    current
        .write_checked(fixture.owners[0], &v(99), &v(88), domain.borrowed())
        .unwrap();
    let current_before = current.clone();

    for wrapper in [
        &fixture.base_roots_only,
        &fixture.log_roots_only,
        &historical,
        &current,
    ] {
        let before = wrapper.clone();
        assert!(wrapper.guard_conditions(tiny).is_err());
        assert_eq!(*wrapper, before);
        let mut selected = wrapper.clone();
        let mut proof_calls = 0;
        assert!(
            selected
                .refine_guards(domain.borrowed(), tiny, &mut |_| {
                    proof_calls += 1;
                    Ok::<_, QueryReason>(None)
                })
                .is_err()
        );
        assert_eq!(proof_calls, 0, "collection failure precedes all proofs");
        assert_eq!(selected, before);
    }

    let expected_order: Vec<_> = fixture.expected.iter().cloned().collect();
    assert!(expected_order.len() > 2);
    for wrapper in [&historical, &current] {
        assert_eq!(wrapper.guard_conditions(limits).unwrap(), fixture.expected);
        let before = wrapper.clone();
        let mut selected = wrapper.clone();
        let mut proof_order = Vec::new();
        let error = selected.refine_guards(domain.borrowed(), limits, &mut |guard| {
            proof_order.push(guard.clone());
            if guard == &expected_order[1] {
                Err(QueryReason::Cancelled)
            } else {
                Ok(Some(true))
            }
        });
        assert_eq!(
            error,
            Err(GuardRefinementError::Proof(QueryReason::Cancelled))
        );
        assert_eq!(proof_order, expected_order[..2]);
        assert_eq!(selected, before, "no earlier proof is published on failure");
    }
    assert_eq!(historical, fixture.joined);
    assert_eq!(current, current_before);
    current.restore(checkpoint);
    assert_eq!(current, historical);
}

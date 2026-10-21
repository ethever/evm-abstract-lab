//! Root-backed canonicalization cannot strengthen RPC independence.

use super::super::*;
use {crate::Fork, crate::domain::symbolic::ExprLimits, crate::world::test_support::TestAccount};

fn value(n: u64) -> TestValue {
    TestValue::constant(U256::from(n))
}

#[test]
fn widening_form_preserves_roots_facts_accounts_logs_and_conservative_rpc_dependencies() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = domain.spec().relations().expressions();
    let address = Address::repeat_byte(0x47);
    let mut world = TestWorld::new(Fork::Osaka, "widening leaf");
    let mut account = TestAccount::unknown();
    account.storage.insert(U256::from(3), value(23));
    world.insert(address, account).unwrap();
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let token = ExprId::fresh().unwrap();
    let gas = TestValue::unsigned_range(U256::ZERO, U256::from(u64::MAX))
        .unwrap()
        .with_expression(token.clone());
    store
        .write_checked(address, &value(7), &gas, domain.borrowed())
        .unwrap();
    store
        .write_transient_checked(address, &value(8), &value(19), domain.borrowed())
        .unwrap();
    store.write_balance(address, value(11));
    store.write_nonce(address, value(9));
    store.touch_account(address);
    store.may_touch_account(Address::repeat_byte(0x48));
    let site = LogKey {
        address,
        code_address: address,
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: crate::world::LogSourceKind::Runtime,
        pc: 7,
    };
    store
        .emit_log(
            site,
            TestLog {
                topics: vec![gas.clone()],
                data: TestBytes::exact(&[2, 3]),
            },
            domain.borrowed(),
        )
        .unwrap();
    assert!(
        store
            .missing_initial_slots(&world, address, &value(7))
            .is_empty()
    );
    let original = store.clone();
    let widened = store.widening_form(domain.borrowed(), limits).unwrap();
    assert_eq!(widened.persistent.roots, store.persistent.roots);
    assert_eq!(widened.transient.roots, store.transient.roots);
    assert_eq!(widened.persistent.base, store.persistent.base);
    assert_eq!(widened.balances, store.balances);
    assert_eq!(widened.nonces, store.nonces);
    assert_eq!(widened.codes, store.codes);
    assert_eq!(widened.existence, store.existence);
    assert_eq!(widened.created, store.created);
    assert_eq!(widened.pending_destruction, store.pending_destruction);
    assert_eq!(widened.touched, store.touched);
    assert_eq!(widened.touched_default, store.touched_default);
    assert_eq!(
        widened.possible_logs.values().next().unwrap().topics,
        vec![gas.clone()]
    );
    assert_eq!(
        widened
            .possible_logs
            .values()
            .next()
            .unwrap()
            .data
            .exact_bytes(
                crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                    .borrowed()
            ),
        Some(vec![2, 3])
    );
    assert_eq!(widened.persistent.operands.get(&token), Some(&gas));
    assert_eq!(
        widened
            .read(address, &value(7), domain.borrowed())
            .numeric(),
        gas.numeric()
    );
    assert_eq!(
        widened
            .read(address, &value(3), domain.borrowed())
            .singleton(),
        Some(U256::from(23))
    );
    assert_eq!(
        widened
            .read_transient(address, &value(8), domain.borrowed())
            .singleton(),
        Some(U256::from(19))
    );
    assert_eq!(
        widened.missing_initial_slots(&world, address, &value(7)),
        vec![U256::from(7)]
    );
    assert!(
        widened
            .missing_initial_slots(&world, address, &value(3))
            .is_empty()
    );
    assert_eq!(
        widened,
        widened.widening_form(domain.borrowed(), limits).unwrap()
    );
    assert_eq!(store, original);
}

#[test]
fn widening_form_refreshes_redundant_slot_cache_and_rejects_errors_atomically() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = domain.spec().relations().expressions();
    let address = Address::repeat_byte(0x48);
    let mut world = TestWorld::new(Fork::Osaka, "cache leaf");
    world.insert(address, TestAccount::empty()).unwrap();
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store
        .write_checked(address, &value(7), &value(9), domain.borrowed())
        .unwrap();
    store
        .persistent
        .slots
        .insert((address, U256::from(7)), value(99));
    let original = store.clone();
    let widened = store.widening_form(domain.borrowed(), limits).unwrap();
    assert_eq!(
        widened
            .persistent
            .slots
            .get(&(address, U256::from(7)))
            .unwrap()
            .singleton(),
        Some(U256::from(9))
    );
    assert!(widened.persistent.default_at(address).contains(U256::ZERO));
    assert!(
        widened
            .persistent
            .default_at(address)
            .contains(U256::from(9))
    );
    assert!(
        store
            .widening_form(
                domain.borrowed(),
                ExprLimits {
                    max_nodes: 1,
                    max_depth: 1
                }
            )
            .is_err()
    );
    assert_eq!(store, original);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        assert!(matches!(
            store.widening_form(domain.borrowed(), limits),
            Err(StoreError::Expression(ExprError::Cancelled)
                | StoreError::State(StateError::Cancelled))
        ));
    });
    assert_eq!(store, original);
    let failure = StoreError::Expression(ExprError::Limit {
        nodes: 99,
        depth: 9,
    });
    store.persistent.failure = Some(failure.clone());
    let failed = store.clone();
    assert_eq!(store.widening_form(domain.borrowed(), limits), Err(failure));
    assert_eq!(store, failed);
}

#[test]
fn widening_form_refreshes_projected_account_and_log_scalar_constants() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let limits = domain.spec().relations().expressions();
    let world = TestWorld::new(Fork::Osaka, "projected scalar leaf");
    let address = Address::repeat_byte(0x53);
    let site = LogKey {
        address,
        code_address: address,
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: crate::world::LogSourceKind::Runtime,
        pc: 5,
    };
    let mut left = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut right = left.clone();
    for (store, number) in [(&mut left, 7u64), (&mut right, 9u64)] {
        store.write_balance(address, value(number));
        store.write_nonce(address, value(number));
        store.balance_default = value(number);
        store
            .emit_log(
                site.clone(),
                TestLog {
                    topics: vec![value(number)],
                    data: TestBytes::exact(&[1]),
                },
                domain.borrowed(),
            )
            .unwrap();
    }
    let selector = ExprId::fresh().unwrap();
    let mut projected = left
        .join_guarded(&right, domain.borrowed(), selector.clone(), limits)
        .unwrap();
    projected
        .rename_fresh(
            &BTreeMap::from([(selector, ExprId::constant(U256::ZERO))]),
            limits,
            domain.borrowed(),
        )
        .unwrap();
    let original = projected.clone();
    let widened = projected.widening_form(domain.borrowed(), limits).unwrap();
    let direct = right.widening_form(domain.borrowed(), limits).unwrap();
    for (actual, expected, before) in [
        (
            &widened.balances[&address],
            &direct.balances[&address],
            &original.balances[&address],
        ),
        (
            &widened.nonces[&address],
            &direct.nonces[&address],
            &original.nonces[&address],
        ),
        (
            &widened.balance_default,
            &direct.balance_default,
            &original.balance_default,
        ),
        (
            &widened.possible_logs[&site].topics[0],
            &direct.possible_logs[&site].topics[0],
            &original.possible_logs[&site].topics[0],
        ),
    ] {
        assert_eq!(actual.numeric(), expected.numeric());
        assert_eq!(actual.singleton(), Some(U256::from(9)));
        assert_eq!(actual.expression(), before.expression());
        assert_eq!(actual.provenance(), before.provenance());
    }
    assert_eq!(widened.persistent.roots, original.persistent.roots);
    assert_eq!(widened.transient.roots, original.transient.roots);
    assert_eq!(
        widened,
        widened.widening_form(domain.borrowed(), limits).unwrap()
    );
    assert_eq!(projected, original);
}

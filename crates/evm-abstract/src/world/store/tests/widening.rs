//! Root-backed canonicalization cannot strengthen RPC independence.

use super::super::*;
use crate::{Fork, domain::symbolic::ExprLimits, world::Account};

fn value(n: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}

#[test]
fn widening_form_preserves_roots_facts_accounts_logs_and_conservative_rpc_dependencies() {
    let domain = Domain::default();
    let limits = domain.spec().relations().expressions();
    let address = Address::repeat_byte(0x47);
    let mut world = World::new(Fork::Osaka, "widening leaf");
    let mut account = Account::unknown();
    account.storage.insert(U256::from(3), value(23));
    world.insert(address, account).unwrap();
    let mut store = Store::new(&world);
    let token = ExprId::fresh().unwrap();
    let gas = AbstractValue::unsigned_range(U256::ZERO, U256::from(u64::MAX))
        .unwrap()
        .with_expression(token.clone());
    store
        .write_checked(address, &value(7), &gas, domain)
        .unwrap();
    store
        .write_transient_checked(address, &value(8), &value(19), domain)
        .unwrap();
    store.write_balance(address, value(11));
    store.write_nonce(address, value(9));
    let site = LogKey {
        address,
        code_address: address,
        pc: 7,
    };
    store
        .emit_log(
            site,
            AbstractLog {
                topics: vec![gas.clone()],
                data: ByteArray::exact(&[2, 3]),
            },
            domain,
        )
        .unwrap();
    assert!(
        store
            .missing_initial_slots(&world, address, &value(7))
            .is_empty()
    );
    let original = store.clone();
    let widened = store.widening_form(domain, limits).unwrap();
    assert_eq!(widened.persistent.roots, store.persistent.roots);
    assert_eq!(widened.transient.roots, store.transient.roots);
    assert_eq!(widened.persistent.base, store.persistent.base);
    assert_eq!(widened.balances, store.balances);
    assert_eq!(widened.nonces, store.nonces);
    assert_eq!(widened.codes, store.codes);
    assert_eq!(widened.existence, store.existence);
    assert_eq!(widened.created, store.created);
    assert_eq!(widened.pending_destruction, store.pending_destruction);
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
            .exact_bytes(),
        Some(vec![2, 3])
    );
    assert_eq!(widened.persistent.operands.get(&token), Some(&gas));
    assert_eq!(
        widened.read(address, &value(7), domain).numeric(),
        gas.numeric()
    );
    assert_eq!(
        widened.read(address, &value(3), domain).singleton(),
        Some(U256::from(23))
    );
    assert_eq!(
        widened
            .read_transient(address, &value(8), domain)
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
    assert_eq!(widened, widened.widening_form(domain, limits).unwrap());
    assert_eq!(store, original);
}

#[test]
fn widening_form_refreshes_redundant_slot_cache_and_rejects_errors_atomically() {
    let domain = Domain::default();
    let limits = domain.spec().relations().expressions();
    let address = Address::repeat_byte(0x48);
    let mut world = World::new(Fork::Osaka, "cache leaf");
    world.insert(address, Account::empty()).unwrap();
    let mut store = Store::new(&world);
    store
        .write_checked(address, &value(7), &value(9), domain)
        .unwrap();
    store
        .persistent
        .slots
        .insert((address, U256::from(7)), value(99));
    let original = store.clone();
    let widened = store.widening_form(domain, limits).unwrap();
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
                domain,
                ExprLimits {
                    max_nodes: 1,
                    max_depth: 1
                }
            )
            .is_err()
    );
    assert_eq!(store, original);
    let cancellation = embedded_smt::Cancellation::new();
    cancellation.cancel();
    embedded_smt::with_cancellation(&cancellation, || {
        assert!(matches!(
            store.widening_form(domain, limits),
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
    assert_eq!(store.widening_form(domain, limits), Err(failure));
    assert_eq!(store, failed);
}

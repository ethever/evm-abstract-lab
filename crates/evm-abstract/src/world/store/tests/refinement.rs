//! Path projection across account planes, scalars, log bytes and rollback.

use super::super::*;
use crate::{
    Fork,
    domain::relational::{CheckResult, QueryReason, RelationLimits, RelationState, ValueQuery},
    world::{Account, GuardRefinementError},
};
use revm_bytecode::opcode;

fn v(n: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(n))
}
fn policy() -> RelationLimits {
    RelationLimits {
        max_nodes: 100_000,
        max_depth: 256,
        rlimit: 1_000_000,
        ..RelationLimits::default()
    }
}
fn truth(relations: &RelationState, e: &ExprId) -> Result<Option<bool>, QueryReason> {
    let p = policy();
    let zero = ExprId::operation(opcode::ISZERO, std::slice::from_ref(e), p.expressions()).unwrap();
    match relations.unique_value(&zero, &p) {
        ValueQuery::Unique(v) if v == U256::ZERO => Ok(Some(true)),
        ValueQuery::Unique(v) if v == U256::ONE => Ok(Some(false)),
        ValueQuery::Multiple => Ok(None),
        ValueQuery::Unknown(reason) => Err(reason),
        result => panic!("reachable Boolean proof expected: {result:?}"),
    }
}

#[test]
fn native_guard_projection_pairs_slots_balances_logs_and_restores_old_snapshot() {
    let domain = Domain::default();
    let address = Address::repeat_byte(0x21);
    let mut world = World::new(Fork::Osaka, "refine leaf");
    world.insert(address, Account::empty()).unwrap();
    let mut old = Store::new(&world);
    let site = LogKey {
        address,
        code_address: address,
        pc: 7,
    };
    old.emit_log(
        site.clone(),
        AbstractLog {
            topics: vec![v(3)],
            data: ByteArray::exact(&[3]),
        },
        domain,
    )
    .unwrap();
    let mut changed = old.clone();
    changed
        .write_checked(address, &v(7), &v(42), domain)
        .unwrap();
    changed
        .write_transient_checked(address, &v(8), &v(19), domain)
        .unwrap();
    changed.write_balance(address, v(10));
    changed.write_nonce(address, v(9));
    let mut changed_log = Store::new(&world);
    changed_log
        .emit_log(
            site.clone(),
            AbstractLog {
                topics: vec![v(1)],
                data: ByteArray::exact(&[1]),
            },
            domain,
        )
        .unwrap();
    changed.possible_logs = changed_log.possible_logs;
    let guard = ExprId::fresh().unwrap();
    let joined = changed
        .join_guarded(&old, domain, guard.clone(), policy().expressions())
        .unwrap();
    let read_before = joined.read(address, &v(7), domain);
    for (nonzero, persistent, transient, balance, nonce, byte) in
        [(true, 42, 19, 10, 9, 1), (false, 0, 0, 0, 0, 3)]
    {
        let mut relations = RelationState::default();
        assert_eq!(
            relations.assume(&guard, nonzero, &policy()),
            CheckResult::Sat
        );
        let mut selected = joined.clone();
        let snapshot = selected.snapshot();
        assert!(
            selected
                .refine_guards(domain, policy().expressions(), &mut |e| truth(
                    &relations, e
                ))
                .unwrap()
        );
        assert_eq!(
            selected.slot_root(address, false).unwrap(),
            joined.slot_root(address, false).unwrap()
        );
        assert_eq!(
            selected.read(address, &v(7), domain).singleton(),
            Some(U256::from(persistent))
        );
        assert_eq!(
            selected.read_transient(address, &v(8), domain).singleton(),
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
            event.data.byte_at(0, domain).singleton(),
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
    let mut world = World::new(Fork::Osaka, "unknown baseline");
    world.insert(address, Account::unknown()).unwrap();
    let domain = Domain::default();
    let old = Store::new(&world);
    let mut written = old.clone();
    written
        .write_checked(address, &v(7), &v(9), domain)
        .unwrap();
    let guard = ExprId::fresh().unwrap();
    let joined = written
        .join_guarded(&old, domain, guard.clone(), policy().expressions())
        .unwrap();
    let initial_requests = joined.missing_initial_slots(&world, address, &v(7));
    let mut selected = joined.clone();
    let mut relations = RelationState::default();
    assert_eq!(relations.assume(&guard, true, &policy()), CheckResult::Sat);
    assert!(
        selected
            .refine_guards(domain, policy().expressions(), &mut |e| truth(
                &relations, e
            ))
            .unwrap()
    );
    assert_eq!(
        selected.read(address, &v(7), domain).singleton(),
        Some(U256::from(9))
    );
    assert!(world.account(address).unwrap().storage.is_empty());
    assert_eq!(
        selected.missing_initial_slots(&world, address, &v(7)),
        initial_requests
    );
    let saved = selected.clone();
    assert_eq!(
        selected.refine_guards(domain, policy().expressions(), &mut |_| Err(
            QueryReason::Cancelled
        )),
        Err(GuardRefinementError::Proof(QueryReason::Cancelled))
    );
    assert_eq!(selected, saved);
}

use super::*;
use crate::{
    Fork,
    domain::AbstractValue,
    world::{Account, Existence, World},
};
use alloy_primitives::U256;
use std::num::NonZeroUsize;

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> AbstractValue {
    AbstractValue::constant(U256::from(number))
}

#[test]
fn nonempty_code_has_no_deletion_join_fee_or_candidate_capacity_product() {
    let mut fees = Vec::new();
    for bytes in [1, 3_124, 25_650] {
        for capacity in [1, 1_024] {
            let domain = Domain::new(NonZeroUsize::new(capacity).unwrap());
            let mut world = World::new(Fork::Osaka, "finalization code size");
            let mut account = Account::from_hex(&"00".repeat(bytes), world.fork()).unwrap();
            account.balance = AbstractValue::top();
            account.nonce = AbstractValue::top();
            world.insert(address(0x101), account).unwrap();
            world
                .insert(
                    address(0x102),
                    Account {
                        code: Code::Delegation(address(0x101)),
                        ..Account::unknown()
                    },
                )
                .unwrap();
            let mut store = Store::new(&world);
            store.touch_account(address(0x101));
            store.may_touch_account(address(0x102));
            let before = store.clone();
            let plan = Plan::new(&store);
            assert_eq!(plan.actions.len(), 2);
            assert!(plan.actions.iter().all(|(_, a)| *a == Deletion::Retain));
            let mut budget = WorkBudget::new(20_000_000);
            store
                .finalize_transaction_checked(domain, None, &mut budget)
                .unwrap();
            fees.push(budget.used());
            assert_eq!(store.code(address(0x101)), before.code(address(0x101)));
            assert_eq!(store.code(address(0x102)), before.code(address(0x102)));
            assert_eq!(store.existence(address(0x101)), Existence::Present);
        }
    }
    assert!(fees[0] > 0);
    assert!(fees.iter().all(|fee| *fee == fees[0]), "{fees:?}");
}

#[test]
fn plan_keeps_empty_touch_uncertainty_and_pending_nonempty_deletion() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "finite finalization lifecycle");
    for number in 0x100..=0x107 {
        let mut account = if number >= 0x106 {
            Account::from_hex("00", world.fork()).unwrap()
        } else {
            Account::empty()
        };
        if number == 0x102 {
            account.balance = domain.join(&word(0), &word(7));
        }
        if number == 0x103 {
            account.nonce = domain.join(&word(0), &word(1));
        }
        world.insert(address(number), account).unwrap();
    }
    let mut store = Store::new(&world);
    for number in [0x101, 0x102, 0x103, 0x105, 0x106, 0x107] {
        store.touch_account(address(number));
    }
    store.may_touch_account(address(0x104));
    store.pending_destruction.insert(address(0x105), Some(true));
    store.pending_destruction.insert(address(0x106), Some(true));
    store.pending_destruction.insert(address(0x107), None);
    let plan = Plan::new(&store);
    let expected = [
        Deletion::Retain,
        Deletion::Delete,
        Deletion::Join,
        Deletion::Join,
        Deletion::Join,
        Deletion::Delete,
        Deletion::Delete,
        Deletion::Join,
    ];
    assert_eq!(
        plan.actions.iter().map(|(_, a)| *a).collect::<Vec<_>>(),
        expected
    );
    let mut budget = WorkBudget::new(20_000_000);
    store
        .finalize_transaction_checked(domain, None, &mut budget)
        .unwrap();
    assert_eq!(store.existence(address(0x100)), Existence::Present);
    for number in [0x101, 0x105, 0x106] {
        assert_eq!(store.existence(address(number)), Existence::Absent);
        assert_eq!(store.code(address(number)), Some(&Code::Empty));
    }
    for number in [0x102, 0x103, 0x104, 0x107] {
        assert_eq!(store.existence(address(number)), Existence::Unknown);
    }
    assert!(store.read_balance(address(0x102)).contains(U256::from(7)));
    assert!(store.nonce(address(0x103)).contains(U256::ONE));
    assert_eq!(store.code(address(0x107)), Some(&Code::Unknown));
    for number in 0x100..=0x108 {
        assert_eq!(store.pending_destruction(address(number)), Some(false));
        assert_eq!(store.touched_in_transaction(address(number)), Some(false));
    }
}

#[test]
fn sticky_touch_is_planned_after_restore_and_late_refusal_keeps_live_state() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "finalization rollback");
    world.insert(address(3), Account::empty()).unwrap();
    let mut before = Store::new(&world);
    let saved = before.snapshot();
    before.touch_account(address(3));
    before.write_balance(address(3), word(9));
    before.write(address(3), &word(1), &word(7), domain);
    assert_eq!(Deletion::at(&before, address(3)), Deletion::Retain);
    assert_eq!(Deletion::at(saved.state(), address(3)), Deletion::Retain);
    let mut restored = before.clone();
    restored.restore(saved.clone());
    assert_eq!(Deletion::at(&restored, address(3)), Deletion::Delete);

    let mut complete = before.clone();
    let mut full = WorkBudget::new(20_000_000);
    complete
        .finalize_transaction_checked(domain, Some(&saved), &mut full)
        .unwrap();
    assert_eq!(complete.existence(address(3)), Existence::Absent);
    assert_eq!(
        complete.read_balance(address(3)).singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(complete.touched_in_transaction(address(3)), Some(false));
    for maximum in [0, 1, full.used() - 1] {
        let mut refused = before.clone();
        assert_eq!(
            refused.finalize_transaction_checked(
                domain,
                Some(&saved),
                &mut WorkBudget::new(maximum),
            ),
            Err(FinalizationError::Work)
        );
        assert_eq!(refused, before, "maximum={maximum}");
    }
    let cancellation = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&cancellation, || {
        let mut budget = WorkBudget::new(20_000_000);
        cancellation.cancel();
        let mut refused = before.clone();
        assert_eq!(
            refused.finalize_transaction_checked(domain, Some(&saved), &mut budget),
            Err(FinalizationError::Cancelled)
        );
        assert_eq!(refused, before);
    });
}

#[test]
fn may_delete_growth_includes_implicit_owners_and_preserves_unrelated_fields() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "finalization implicit owners");
    world
        .insert(
            address(0x101),
            Account::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let mut store = Store::new(&world);
    for number in [0x200, 0x300] {
        // Touch-only addresses have no explicit code/balance/nonce/root entry.
        store.may_touch_account(address(number));
    }
    store.write(address(0x101), &word(1), &word(9), domain);
    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    store
        .finalize_transaction_checked(domain, None, &mut budget)
        .unwrap();
    assert_eq!(store.code(address(0x101)), before.code(address(0x101)));
    assert_eq!(store.read(address(0x101), &word(1), domain), word(9));
    for number in [0x200, 0x300] {
        assert_eq!(store.existence(address(number)), Existence::Unknown);
        assert!(store.read_balance(address(number)).numeric().is_top());
        assert!(store.nonce(address(number)).numeric().is_top());
        assert_eq!(store.code(address(number)), Some(&Code::Unknown));
    }
}

#[test]
fn may_delete_retention_refusal_is_typed_and_never_installs_the_numeric_fallback() {
    use crate::domain::{
        relational::RelationLimits,
        symbolic::{ExprError, StateError},
    };

    let domain = Domain::from_spec(Domain::default().spec().with_relations(RelationLimits {
        // An unobserved slot root includes its source token (two nodes).
        // Its MAY-delete ITE adds a selector and the zero-state alternative.
        max_nodes: 2,
        ..RelationLimits::default()
    }));
    let limits = domain.spec().relations().expressions();
    let mut world = World::new(Fork::Osaka, "finalization retention refusal");
    world
        .insert(
            address(0x200),
            Account {
                code: Code::Empty,
                nonce: word(0),
                balance: domain.join(&word(0), &word(1)),
                existence: Existence::Present,
                ..Account::unknown()
            },
        )
        .unwrap();
    let mut store = Store::new(&world);
    store.touch_account(address(0x200));
    store.check_existing(limits).unwrap();
    assert_eq!(Deletion::at(&store, address(0x200)), Deletion::Join);

    // The unchanged public API demonstrates the existing fallback contract.
    let mut fallback = store.clone();
    fallback.finalize_transaction(domain);
    assert!(matches!(
        fallback.check_existing(limits),
        Err(StoreError::State(StateError::Limit { .. })
            | StoreError::Expression(ExprError::Limit { .. }))
    ));

    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    let error = store
        .finalize_transaction_checked(domain, None, &mut budget)
        .unwrap_err();
    assert!(matches!(
        error.into_store_error(),
        Some(
            StoreError::State(StateError::Limit { .. })
                | StoreError::Expression(ExprError::Limit { .. })
        )
    ));
    assert_eq!(store, before);
    assert!(!budget.exhausted());
}

#[test]
fn restored_ripemd_may_touch_changes_only_addr3_and_not_saved_or_other_owners() {
    let domain = Domain::default();
    let mut world = World::new(Fork::Osaka, "restored RIPEMD MAY touch");
    for number in [0x101, 0x201] {
        let mut account = Account::from_hex("00", world.fork()).unwrap();
        account.storage.insert(U256::ONE, word(number));
        world.insert(address(number), account).unwrap();
    }
    let mut current = Store::new(&world);
    let saved = current.snapshot();
    let saved_state = saved.state().clone();
    current.may_touch_account(address(3));
    current
        .write_checked(address(0x101), &word(1), &word(99), domain)
        .unwrap();
    let frozen = current.clone();
    let mut restored = current.clone();
    restored.restore(saved.clone());
    assert_eq!(restored.touched_in_transaction(address(3)), None);
    let plan = Plan::new(&restored);
    assert_eq!(
        plan.actions,
        vec![
            (address(3), Deletion::Join),
            (address(0x101), Deletion::Retain),
            (address(0x201), Deletion::Retain),
        ]
    );
    assert_eq!(plan.join_fields, restored.maybe_delete_fields(address(3)));
    let mut budget = WorkBudget::new(20_000_000);
    current
        .finalize_transaction_checked(domain, Some(&saved), &mut budget)
        .unwrap();
    for number in [0x101, 0x201] {
        let owner = address(number);
        assert_eq!(
            current.slot_root(owner, false),
            saved.state().slot_root(owner, false)
        );
        assert_eq!(
            current.read_checked(owner, &word(1), domain),
            saved.state().read_checked(owner, &word(1), domain)
        );
        assert_eq!(
            current.read_balance(owner),
            saved.state().read_balance(owner)
        );
        assert_eq!(current.nonce(owner), saved.state().nonce(owner));
        assert_eq!(current.code(owner), saved.state().code(owner));
    }
    assert_eq!(current.touched_in_transaction(address(3)), Some(false));
    assert_eq!(current.existence(address(3)), Existence::Unknown);
    assert_eq!(saved.state(), &saved_state);
    assert_eq!(frozen.read(address(0x101), &word(1), domain), word(99));
}

#[test]
fn may_delete_field_fee_is_independent_of_unrelated_cell_payloads() {
    let domain = Domain::default();
    let mut fields = Vec::new();
    for cells in [2, 64] {
        let mut world = World::new(Fork::Osaka, "local deletion field discovery");
        let mut account = Account::from_hex("00", world.fork()).unwrap();
        for key in 0..cells {
            account.storage.insert(U256::from(key), word(key + 7));
        }
        world.insert(address(0x101), account).unwrap();
        let mut store = Store::new(&world);
        store.may_touch_account(address(3));
        let plan = Plan::new(&store);
        assert_eq!(
            plan.actions
                .iter()
                .filter(|(_, a)| *a == Deletion::Join)
                .count(),
            1
        );
        fields.push(plan.join_fields);
        let mut extended = store.clone();
        extended
            .write_checked(address(3), &word(1), &word(9), domain)
            .unwrap();
        assert!(extended.maybe_delete_fields(address(3)) > plan.join_fields);
    }
    assert_eq!(fields[0], fields[1]);
}

#[test]
fn may_delete_batch_fee_avoids_owner_count_times_unrelated_fields() {
    let domain = Domain::default();
    let mut fees = Vec::new();
    for owners in [8u64, 32] {
        let mut world = World::new(Fork::Osaka, "MAY deletion work scaling");
        let mut unrelated = Account::from_hex("00", world.fork()).unwrap();
        unrelated.storage_unknown = true;
        for key in 0..owners * 4 {
            unrelated.storage.insert(U256::from(key), word(key + 1));
        }
        world.insert(address(0x900), unrelated).unwrap();
        for number in 0..owners {
            world
                .insert(address(0x1000 + number), Account::unknown())
                .unwrap();
        }
        let mut store = Store::new(&world);
        for number in 0..owners {
            store.may_touch_account(address(0x1000 + number));
        }
        let before = store.slot_root(address(0x900), false);
        let mut budget = WorkBudget::new(usize::MAX);
        store
            .finalize_transaction_checked(domain, None, &mut budget)
            .unwrap();
        assert_eq!(store.slot_root(address(0x900), false), before);
        fees.push(budget.used());
    }
    assert!(fees[1] > fees[0]);
    // Quadrupling both populations must stay below quadratic (16x) growth.
    // This checks the real ledger, not elapsed time or a mirrored fee formula.
    assert!(fees[1] < fees[0] * 8, "{fees:?}");
}

#[test]
fn may_delete_does_not_erase_a_transient_retention_failure_during_cleanup() {
    use crate::domain::symbolic::ExprError;

    let domain = Domain::default();
    let mut store = Store::new(&World::new(Fork::Osaka, "transient refusal before cleanup"));
    store.may_touch_account(address(3));
    let error = StoreError::Expression(ExprError::Limit {
        nodes: usize::MAX,
        depth: 1,
    });
    store.transient.failure = Some(error.clone());
    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    assert_eq!(
        store.finalize_transaction_checked(domain, None, &mut budget),
        Err(FinalizationError::Store(error.clone()))
    );
    assert_eq!(store, before);
    let mut fallback = before;
    fallback.finalize_transaction(domain);
    assert_eq!(
        fallback.check_existing(domain.spec().relations().expressions()),
        Err(error)
    );
}

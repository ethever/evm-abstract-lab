use super::*;

mod clone_invoice;
use alloy_primitives::U256;
use std::num::NonZeroUsize;
use {
    crate::Fork, crate::world::Existence, crate::world::test_support::TestAccount,
    crate::world::test_support::TestValue, crate::world::test_support::TestWorld,
};

fn address(number: u64) -> Address {
    Address::from_word(U256::from(number).into())
}

fn word(number: u64) -> TestValue {
    TestValue::constant(U256::from(number))
}

fn no_delete_owners(plan: &Plan) -> &[OwnerCleanupFootprint] {
    match &plan.fee {
        CleanupWork::OwnerLocal { owners } => owners,
        CleanupWork::Reset { .. } => panic!("expected a no-definite-deletion plan"),
    }
}

fn measure(store: &TestStore) -> Footprint {
    Footprint::measure(store, &mut WorkBudget::new(usize::MAX)).unwrap()
}

#[test]
fn nonempty_code_has_no_deletion_join_fee_or_candidate_capacity_product() {
    let mut fees = Vec::new();
    for bytes in [1, 3_124, 25_650] {
        for capacity in [1, 1_024] {
            let domain = crate::domain::ValueAlgebra::new(
                crate::domain::ProductAlgebra::from_spec(crate::domain::DomainSpec::new(
                    NonZeroUsize::new(capacity).unwrap(),
                    std::num::NonZeroUsize::new(8).unwrap(),
                    std::num::NonZeroUsize::new(256).unwrap(),
                )),
            );
            let mut world = TestWorld::new(Fork::Osaka, "finalization code size");
            let mut account = TestAccount::from_hex(&"00".repeat(bytes), world.fork()).unwrap();
            account.balance = TestValue::top();
            account.nonce = TestValue::top();
            world.insert(address(0x101), account).unwrap();
            world
                .insert(
                    address(0x102),
                    TestAccount {
                        code: Code::Delegation(address(0x101)),
                        ..TestAccount::unknown()
                    },
                )
                .unwrap();
            let mut store = TestStore::new(
                &world,
                crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
                    .borrowed(),
            );
            store.touch_account(address(0x101));
            store.may_touch_account(address(0x102));
            let before = store.clone();
            let plan = Plan::new(&store);
            assert_eq!(plan.actions.len(), 2);
            assert!(plan.actions.iter().all(|(_, a)| *a == Deletion::Retain));
            let mut budget = WorkBudget::new(20_000_000);
            store
                .finalize_transaction_checked(domain.borrowed(), None, &mut budget)
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
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut world = TestWorld::new(Fork::Osaka, "finite finalization lifecycle");
    for number in 0x100..=0x107 {
        let mut account = if number >= 0x106 {
            TestAccount::from_hex("00", world.fork()).unwrap()
        } else {
            TestAccount::empty()
        };
        if number == 0x102 {
            account.balance = domain.join(&word(0), &word(7));
        }
        if number == 0x103 {
            account.nonce = domain.join(&word(0), &word(1));
        }
        world.insert(address(number), account).unwrap();
    }
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
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
        .finalize_transaction_checked(domain.borrowed(), None, &mut budget)
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
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut world = TestWorld::new(Fork::Osaka, "finalization rollback");
    world.insert(address(3), TestAccount::empty()).unwrap();
    let mut before = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let saved = before.snapshot();
    before.touch_account(address(3));
    before.write_balance(address(3), word(9));
    before.write(address(3), &word(1), &word(7), domain.borrowed());
    assert_eq!(Deletion::at(&before, address(3)), Deletion::Retain);
    assert_eq!(Deletion::at(saved.state(), address(3)), Deletion::Retain);
    let mut restored = before.clone();
    restored.restore(saved.clone());
    assert_eq!(Deletion::at(&restored, address(3)), Deletion::Delete);

    let mut complete = before.clone();
    let mut full = WorkBudget::new(20_000_000);
    complete
        .finalize_transaction_checked(domain.borrowed(), Some(&saved), &mut full)
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
                domain.borrowed(),
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
            refused.finalize_transaction_checked(domain.borrowed(), Some(&saved), &mut budget),
            Err(FinalizationError::Cancelled)
        );
        assert_eq!(refused, before);
    });
}

#[test]
fn may_delete_growth_includes_implicit_owners_and_preserves_unrelated_fields() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut world = TestWorld::new(Fork::Osaka, "finalization implicit owners");
    world
        .insert(
            address(0x101),
            TestAccount::from_hex("00", world.fork()).unwrap(),
        )
        .unwrap();
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    for number in [0x200, 0x300] {
        // Touch-only addresses have no explicit code/balance/nonce/root entry.
        store.may_touch_account(address(number));
    }
    store.write(address(0x101), &word(1), &word(9), domain.borrowed());
    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    store
        .finalize_transaction_checked(domain.borrowed(), None, &mut budget)
        .unwrap();
    assert_eq!(store.code(address(0x101)), before.code(address(0x101)));
    assert_eq!(
        store.read(address(0x101), &word(1), domain.borrowed()),
        word(9)
    );
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

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::DomainSpec::default().with_relations(RelationLimits {
            // An unobserved slot root includes its source token (two nodes).
            // Its MAY-delete ITE adds a selector and the zero-state alternative.
            max_nodes: 2,
            ..RelationLimits::default()
        }),
    ));
    let limits = domain.spec().relations().expressions();
    let mut world = TestWorld::new(Fork::Osaka, "finalization retention refusal");
    world
        .insert(
            address(0x200),
            TestAccount {
                code: Code::Empty,
                nonce: word(0),
                balance: domain.join(&word(0), &word(1)),
                existence: Existence::Present,
                ..TestAccount::unknown()
            },
        )
        .unwrap();
    let mut store = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.touch_account(address(0x200));
    store.check_existing(limits).unwrap();
    assert_eq!(Deletion::at(&store, address(0x200)), Deletion::Join);

    // The private plan still retains its numerical fallback and failure marker;
    // the public Result API must refuse it without installing that fallback.
    let mut fallback = store.clone();
    assert!(fallback.finalize_transaction(domain.borrowed()).is_err());
    assert_eq!(fallback, store);
    Plan::new(&fallback)
        .apply(&mut fallback, domain.borrowed(), &mut || {
            Ok::<(), StoreError>(())
        })
        .unwrap();
    assert!(matches!(
        fallback.check_existing(limits),
        Err(StoreError::State(StateError::Limit { .. })
            | StoreError::Expression(ExprError::Limit { .. }))
    ));

    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    let error = store
        .finalize_transaction_checked(domain.borrowed(), None, &mut budget)
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
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut world = TestWorld::new(Fork::Osaka, "restored RIPEMD MAY touch");
    for number in [0x101, 0x201] {
        let mut account = TestAccount::from_hex("00", world.fork()).unwrap();
        account.storage.insert(U256::ONE, word(number));
        world.insert(address(number), account).unwrap();
    }
    let mut current = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let saved = current.snapshot();
    let saved_state = saved.state().clone();
    current.may_touch_account(address(3));
    current
        .write_checked(address(0x101), &word(1), &word(99), domain.borrowed())
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
    assert_eq!(
        no_delete_owners(&plan),
        &[restored.owner_cleanup_footprint(address(3), tree_work)]
    );
    let mut budget = WorkBudget::new(20_000_000);
    current
        .finalize_transaction_checked(domain.borrowed(), Some(&saved), &mut budget)
        .unwrap();
    for number in [0x101, 0x201] {
        let owner = address(number);
        assert_eq!(
            current.slot_root(owner, false),
            saved.state().slot_root(owner, false)
        );
        assert_eq!(
            current.read_checked(owner, &word(1), domain.borrowed()),
            saved
                .state()
                .read_checked(owner, &word(1), domain.borrowed())
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
    assert_eq!(
        frozen.read(address(0x101), &word(1), domain.borrowed()),
        word(99)
    );
}

#[test]
fn may_delete_field_fee_is_independent_of_unrelated_cell_payloads() {
    use crate::domain::relational::RelationLimits;

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::DomainSpec::default().with_relations(RelationLimits {
            max_nodes: 4_096,
            max_depth: 256,
            ..RelationLimits::default()
        }),
    ));
    let limits = domain.spec().relations().expressions();
    let mut fields = Vec::new();
    for cells in [2, 64] {
        let mut world = TestWorld::new(Fork::Osaka, "local deletion field discovery");
        let mut account = TestAccount::from_hex("00", world.fork()).unwrap();
        for key in 0..cells {
            account.storage.insert(U256::from(key), word(key + 7));
        }
        world.insert(address(0x101), account).unwrap();
        let mut store = TestStore::new_checked(&world, domain.borrowed()).unwrap();
        store.may_touch_account(address(3));
        store.check_existing(limits).unwrap();
        let plan = Plan::new(&store);
        assert_eq!(
            plan.actions
                .iter()
                .filter(|(_, a)| *a == Deletion::Join)
                .count(),
            1
        );
        let owner = no_delete_owners(&plan)[0];
        fields.push(owner);
        let mut extended = store.clone();
        extended
            .write_checked(address(3), &word(1), &word(9), domain.borrowed())
            .unwrap();
        extended.check_existing(limits).unwrap();
        assert!(
            extended
                .owner_cleanup_footprint(address(3), tree_work)
                .local_work()
                > owner.local_work()
        );
    }
    assert_eq!(fields[0], fields[1]);
}

#[test]
fn may_delete_batch_fee_avoids_owner_count_times_unrelated_fields() {
    use crate::domain::relational::RelationLimits;

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::DomainSpec::default().with_relations(RelationLimits {
            max_nodes: 4_096,
            max_depth: 256,
            ..RelationLimits::default()
        }),
    ));
    let limits = domain.spec().relations().expressions();
    let mut fees = Vec::new();
    for owners in [8u64, 32] {
        let mut world = TestWorld::new(Fork::Osaka, "MAY deletion work scaling");
        let mut unrelated = TestAccount::from_hex("00", world.fork()).unwrap();
        unrelated.storage_unknown = true;
        for key in 0..owners * 4 {
            unrelated.storage.insert(U256::from(key), word(key + 1));
        }
        world.insert(address(0x900), unrelated).unwrap();
        for number in 0..owners {
            world
                .insert(address(0x1000 + number), TestAccount::unknown())
                .unwrap();
        }
        let mut store = TestStore::new_checked(&world, domain.borrowed()).unwrap();
        for number in 0..owners {
            store.may_touch_account(address(0x1000 + number));
        }
        store.check_existing(limits).unwrap();
        let before = store.slot_root(address(0x900), false);
        let mut budget = WorkBudget::new(usize::MAX);
        store
            .finalize_transaction_checked(domain.borrowed(), None, &mut budget)
            .unwrap();
        store.check_existing(limits).unwrap();
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

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut store = TestStore::new(
        &TestWorld::new(Fork::Osaka, "transient refusal before cleanup"),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    store.may_touch_account(address(3));
    let error = StoreError::Expression(ExprError::Limit {
        nodes: usize::MAX,
        depth: 1,
    });
    store.transient.failure = Some(error.clone());
    let before = store.clone();
    let mut budget = WorkBudget::new(20_000_000);
    assert_eq!(
        store.finalize_transaction_checked(domain.borrowed(), None, &mut budget),
        Err(FinalizationError::Store(error.clone()))
    );
    assert_eq!(store, before);
    let mut fallback = before;
    assert_eq!(
        fallback.finalize_transaction(domain.borrowed()),
        Err(error.clone())
    );
    assert_eq!(fallback, store);
    Plan::new(&fallback)
        .apply(&mut fallback, domain.borrowed(), &mut || {
            Ok::<(), StoreError>(())
        })
        .unwrap();
    assert_eq!(
        fallback.check_existing(domain.spec().relations().expressions()),
        Err(error)
    );
}

#[test]
fn no_delete_receipt_separates_retains_from_may_work() {
    let mut store = TestStore::new(
        &TestWorld::new(Fork::Osaka, "no-delete receipt classes"),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    // An explicit untouched owner still contributes an action, but performs no
    // MAY validation, map mutation or local preparation.
    store.touched.insert(address(3), Some(false));
    let retain = Plan::new(&store);
    assert!(no_delete_owners(&retain).is_empty());
    let retained = retain.no_delete_work(measure(&store), no_delete_owners(&retain));
    assert!(retained.setup > 0);
    assert_eq!(retained.validation, 0);
    assert_eq!(retained.local, 0);
    assert_eq!(retained.maps, 0);
    assert_eq!(retained.cow, 0);

    store.may_touch_account(address(3));
    let may = Plan::new(&store);
    let owners = no_delete_owners(&may);
    assert_eq!(owners.len(), 1);
    let joined = may.no_delete_work(measure(&store), owners);
    assert_eq!(joined.setup, retained.setup);
    assert!(joined.validation > 0);
    assert!(joined.local > 0);
    assert!(joined.maps > 0);
    // Every modified map starts empty; new local nodes are not old-map COW.
    assert_eq!(joined.cow, 0);
    assert_eq!(joined.code_drop, 0);
}

#[test]
fn old_map_cow_is_paid_once_per_batch_and_selected_slots_only() {
    let mut world = TestWorld::new(Fork::Osaka, "MAY COW receipt ownership");
    let mut account = TestAccount::unknown();
    for key in 0..64 {
        account.storage.insert(U256::from(key), word(key + 7));
    }
    world.insert(address(0x900), account).unwrap();
    let base = TestStore::new(
        &world,
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut receipts = Vec::new();
    for count in [1, 8, 32] {
        let mut store = base.clone();
        for number in 0..count {
            store.may_touch_account(address(0x1000 + number));
        }
        let plan = Plan::new(&store);
        let owners = no_delete_owners(&plan);
        assert_eq!(owners.len(), count as usize);
        assert!(owners.iter().all(|owner| owner.planes[0].cells == 0));
        let footprint = measure(&store);
        let receipt = plan.no_delete_work(footprint, owners);
        assert_eq!(receipt.validation, footprint.validation_work);
        receipts.push(receipt);
    }
    // More MAY owners do not repay the same unrelated nonempty maps' COW.
    assert_eq!(receipts[0].cow, receipts[1].cow);
    assert_eq!(receipts[0].cow, receipts[2].cow);
    assert!(receipts[2].local > receipts[0].local);
    assert!(receipts[2].maps > receipts[0].maps);
    assert!(receipts[0].cow > 0);

    let mut selected = base;
    selected.may_touch_account(address(0x900));
    let plan = Plan::new(&selected);
    let owners = no_delete_owners(&plan);
    assert_eq!(owners[0].planes[0].cells, 64);
    let selected = plan.no_delete_work(measure(&selected), owners);
    assert!(selected.cow > receipts[0].cow);
    // Pin the common range tariff, including selected-key collection/get_mut.
    assert_eq!(selected.maps, 3_048);
}

#[test]
fn unrelated_root_weight_belongs_to_validation_and_not_map_height_or_handle_cow() {
    use crate::domain::symbolic::{ExprId, ExprLimits, SlotStateId};

    let mut root = SlotStateId::zero();
    let mut receipts = Vec::new();
    let mut maps = Vec::new();
    for writes in [0, 32] {
        for key in 0..writes {
            root = root
                .store(
                    ExprId::constant(U256::from(key)),
                    ExprId::constant(U256::from(key + 7)),
                    ExprLimits::default().into(),
                )
                .unwrap();
        }
        let mut store = TestStore::new(
            &TestWorld::new(Fork::Osaka, "root weight versus map entries"),
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        );
        store.persistent.roots.insert(address(0x900), root.clone());
        store.may_touch_account(address(3));
        let footprint = measure(&store);
        maps.push(footprint.maps);
        let plan = Plan::new(&store);
        receipts.push(plan.no_delete_work(footprint, no_delete_owners(&plan)));
    }
    assert_eq!(maps[0], maps[1]);
    assert!(receipts[1].validation > receipts[0].validation);
    assert_eq!(receipts[0].local, receipts[1].local);
    assert_eq!(receipts[0].maps, receipts[1].maps);
    assert_eq!(receipts[0].cow, receipts[1].cow);
}

#[test]
fn implicit_may_owners_each_pay_for_and_preserve_recursive_base_scoping() {
    use crate::domain::symbolic::{ExprId, ExprLimits, SlotStateId, SlotStateKind, SourceId};

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let plain = TestStore::new(
        &TestWorld::new(Fork::Osaka, "plain implicit owner"),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let mut store = TestStore::new(
        &TestWorld::new(Fork::Osaka, "recursive implicit owner base"),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    let base = SlotStateId::input(SourceId::fresh().unwrap())
        .join_guarded(
            &SlotStateId::input(SourceId::fresh().unwrap()),
            ExprId::fresh().unwrap(),
            ExprLimits::default().into(),
        )
        .unwrap();
    store.persistent.base = base.clone();
    for number in [3, 4] {
        store.may_touch_account(address(number));
    }
    let plan = Plan::new(&store);
    let owners = no_delete_owners(&plan);
    assert_eq!(owners.len(), 2);
    for owner in owners {
        assert_eq!(owner.planes[0].root_work, base.nodes());
        assert!(owner.planes[0].new_root);
        assert!(
            owner.local_work()
                > plain
                    .owner_cleanup_footprint(address(3), tree_work)
                    .local_work()
        );
    }
    store
        .finalize_transaction_checked(domain.borrowed(), None, &mut WorkBudget::new(usize::MAX))
        .unwrap();
    assert_eq!(store.persistent.base, base);
    for number in [3, 4] {
        let root = store.slot_root(address(number), false).unwrap();
        let SlotStateKind::Ite { then_state, .. } = root.kind() else {
            panic!("expected the owner's MAY-deletion alternative");
        };
        let SlotStateKind::Ite {
            then_state,
            else_state,
            ..
        } = then_state.kind()
        else {
            panic!("expected the scoped old ITE base");
        };
        for alternative in [then_state, else_state] {
            let SlotStateKind::Input { source, .. } = alternative.kind() else {
                panic!("expected a scoped input alternative");
            };
            assert_eq!(source.namespace(), U256::from(number) << 8);
        }
    }
}

#[test]
fn definite_and_mixed_deletion_charge_sufficient_work_and_refuse_atomically() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut charges = Vec::new();
    for with_may in [false, true] {
        let mut before = TestStore::new(
            &TestWorld::new(Fork::Osaka, "reset work accounting"),
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        );
        before.pending_destruction.insert(address(1), Some(true));
        if with_may {
            before.may_touch_account(address(2));
        }
        let footprint = measure(&before);
        let plan = Plan::new(&before);
        assert!(matches!(plan.fee, CleanupWork::Reset { .. }));
        assert!(plan.work(footprint) >= tree_work(footprint.fields));

        let mut completed = before.clone();
        let mut paid = WorkBudget::new(usize::MAX);
        completed
            .finalize_transaction_checked(domain.borrowed(), None, &mut paid)
            .unwrap();
        let charged = paid.used();
        assert!(charged >= plan.work(footprint));
        assert_eq!(completed.existence(address(1)), Existence::Absent);
        if with_may {
            assert_eq!(completed.existence(address(2)), Existence::Unknown);
        }
        for limit in [0, 1, charged - 1] {
            let mut refused = before.clone();
            let checkpoint = refused.snapshot();
            assert_eq!(
                refused.finalize_transaction_checked(
                    domain.borrowed(),
                    None,
                    &mut WorkBudget::new(limit)
                ),
                Err(FinalizationError::Work),
            );
            assert_eq!(&refused, checkpoint.state());
        }
        charges.push(charged);
    }
    // Mixed cleanup pays the additional guarded owner join. This is a work
    // relation derived from the two plans, rather than a historical fee value.
    assert!(charges[1] > charges[0]);
}

#[test]
fn no_delete_late_work_refusal_is_atomic_across_both_planes() {
    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default());
    let mut before = TestStore::new(
        &TestWorld::new(Fork::Osaka, "no-delete atomic work refusal"),
        crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
    );
    before
        .write_checked(address(0x900), &word(1), &word(7), domain.borrowed())
        .unwrap();
    before
        .write_transient_checked(address(0x900), &word(2), &word(9), domain.borrowed())
        .unwrap();
    before.may_touch_account(address(3));
    assert_eq!(no_delete_owners(&Plan::new(&before)).len(), 1);
    let saved = before.snapshot();
    let mut complete = before.clone();
    let mut full = WorkBudget::new(usize::MAX);
    complete
        .finalize_transaction_checked(domain.borrowed(), None, &mut full)
        .unwrap();
    assert_eq!(complete.existence(address(3)), Existence::Unknown);
    assert_eq!(
        complete.read(address(0x900), &word(1), domain.borrowed()),
        word(7)
    );
    assert_eq!(
        complete.read_transient(address(0x900), &word(2), domain.borrowed()),
        word(0)
    );
    for maximum in [0, 1, full.used() - 1] {
        let mut refused = before.clone();
        assert_eq!(
            refused.finalize_transaction_checked(
                domain.borrowed(),
                None,
                &mut WorkBudget::new(maximum)
            ),
            Err(FinalizationError::Work)
        );
        assert_eq!(refused, before, "maximum={maximum}");
        assert_eq!(saved.state(), &before);
    }
}

#[test]
fn no_delete_checks_oversized_unrelated_roots_before_transient_cleanup() {
    use crate::domain::{
        relational::RelationLimits,
        symbolic::{ExprId, ExprLimits, SlotStateId, StateError},
    };

    let domain = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::from_spec(
        crate::domain::DomainSpec::default().with_relations(RelationLimits {
            max_nodes: 6,
            ..RelationLimits::default()
        }),
    ));
    let mut oversized = SlotStateId::zero();
    for key in [1, 2] {
        oversized = oversized
            .store(
                ExprId::constant(U256::from(key)),
                ExprId::constant(U256::from(7)),
                ExprLimits::default().into(),
            )
            .unwrap();
    }
    assert!(oversized.nodes() > domain.spec().relations().max_nodes);
    for transient in [false, true] {
        let mut store = TestStore::new(
            &TestWorld::new(Fork::Osaka, "invalid old root before MAY"),
            crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default()).borrowed(),
        );
        store.may_touch_account(address(3));
        let plane = if transient {
            &mut store.transient
        } else {
            &mut store.persistent
        };
        plane.roots.insert(address(0x900), oversized.clone());
        let expected = store
            .check_existing(domain.spec().relations().expressions())
            .unwrap_err();
        assert!(matches!(
            expected,
            StoreError::State(StateError::Limit { .. })
        ));
        let before = store.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        assert_eq!(
            store.finalize_transaction_checked(domain.borrowed(), None, &mut budget),
            Err(FinalizationError::Store(expected))
        );
        assert_eq!(store, before, "transient={transient}");
        assert!(!budget.exhausted());
    }
}

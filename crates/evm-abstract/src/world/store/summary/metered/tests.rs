use super::*;
use crate::Address;
use crate::domain::ValueAlgebra;

mod native;
use native::{CALLS, Calls, Native, Policy};

fn word(value: u64) -> EvmValue<Native> {
    EvmValue::constant(U256::from(value))
}
fn private() -> EvmValue<Native> {
    EvmValue::from_numeric(Native::private())
}

fn plane() -> SummaryPlane<Native> {
    SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: word(0),
        initial_independence: super::super::InitialIndependence::new(false),
    }
}

fn store() -> EvmStoreSummary<Native> {
    EvmStoreSummary {
        persistent: plane(),
        transient: plane(),
        balances: OrderedMap::new(),
        balance_default: EvmValue::top(),
        codes: OrderedMap::new(),
        nonces: OrderedMap::new(),
        existence: OrderedMap::new(),
        created: OrderedMap::new(),
        created_default: Some(false),
        pending_destruction: OrderedMap::new(),
        pending_destruction_default: Some(false),
        touched: OrderedMap::new(),
        touched_default: Some(false),
        possible_logs: OrderedMap::new(),
        logs_unknown: false,
    }
}

fn log(pc: usize, policy: &ValueAlgebra<Policy>) -> (super::super::LogKey, SummaryLog<Native>) {
    (
        super::super::LogKey {
            address: Address::repeat_byte(1),
            code_address: Address::repeat_byte(2),
            code_hash: alloy_primitives::B256::ZERO,
            source_kind: crate::world::LogSourceKind::Runtime,
            pc,
        },
        SummaryLog {
            topics: vec![private()],
            data: crate::world::EvmByteArray::<Native>::exact(&[1, 2])
                .summary_projection(policy.borrowed())
                .unwrap(),
        },
    )
}

#[test]
fn paid_join_preserves_native_predicates_defaults_logs_and_lifecycle() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let owner = Address::repeat_byte(1);
    let mut left = store();
    left.persistent.slots.insert((owner, U256::ONE), private());
    left.transient.slots.insert((owner, U256::ONE), private());
    left.balances.insert(owner, private());
    left.nonces.insert(owner, private());
    left.created.insert(owner, Some(true));
    left.pending_destruction.insert(owner, Some(true));
    left.touched.insert(owner, Some(true));
    left.codes
        .insert(owner, Code::Delegation(Address::repeat_byte(2)));
    left.existence
        .insert(owner, super::super::Existence::Present);
    let (key, value) = log(0, &domain);
    left.possible_logs.insert(key, value);
    let mut right = left.clone();
    let (key, value) = log(1, &domain);
    right.possible_logs.insert(key, value);
    for widen in [false, true] {
        let expected = left.join(&right, domain.borrowed(), widen);
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(1_000_000);
        let actual = left
            .join_with_payment(&right, domain.borrowed(), widen, &mut budget)
            .unwrap();
        assert!(budget.used() > 0);
        assert!(CALLS.get().joins > 0 && CALLS.get().equal > 0);
        assert_eq!(actual, expected);
        let value = actual.persistent.at(owner, U256::ONE).numeric();
        assert!(!value.contains(U256::from(7)));
        assert!(value.observation().contains(U256::from(7)));
        assert_eq!(actual.possible_logs.len(), 2);
    }
}

#[test]
fn paid_layout_keeps_original_owner_defaults_and_saved_control_variants() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let owner = Address::repeat_byte(1);
    let outside = Address::repeat_byte(3);
    let mut input = store();
    input.persistent.slots.insert(
        (owner, U256::ONE),
        private()
            .with_identity(17, 2)
            .with_expression(crate::domain::symbolic::ExprId::fresh().unwrap()),
    );
    input
        .persistent
        .slots
        .insert((owner, U256::from(2)), word(9));
    input.persistent.slots.insert((outside, U256::ONE), word(8));
    input.transient.slots.insert((owner, U256::ONE), private());
    input.balance_default = word(5);
    input.balances.insert(outside, word(9));
    input.nonces.insert(owner, private());
    input.codes.insert(owner, Code::Delegation(outside));
    input
        .existence
        .insert(owner, super::super::Existence::Present);
    input.created.insert(outside, Some(true));
    input.touched.insert(owner, Some(true));
    let (key, value) = log(0, &domain);
    input.possible_logs.insert(key, value);
    let mut layout = input.clone();
    layout.persistent.slots.remove(&(owner, U256::from(2)));
    layout.persistent.slots.remove(&(outside, U256::ONE));
    layout.balances.clear();
    layout.balances.insert(owner, word(99));
    layout.codes.insert(owner, Code::Empty);
    layout
        .existence
        .insert(owner, super::super::Existence::Absent);
    let expected = input.project_to_layout(&layout, domain.borrowed());
    CALLS.set(Calls::default());
    let actual = input
        .project_to_layout_with_payment(&layout, domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert!(CALLS.get().singletons > 0 && CALLS.get().equal > 0);
    assert_eq!(actual, expected);
    assert_eq!(actual.balances[&owner].numeric().exact, Some(U256::from(5)));
    assert!(
        !actual
            .persistent
            .at(owner, U256::ONE)
            .numeric()
            .contains(U256::from(7))
    );
    assert_eq!(actual.codes[&owner], input.codes[&owner]);
    assert_eq!(actual.existence[&owner], input.existence[&owner]);
    assert_eq!(
        actual.persistent.at(owner, U256::ONE),
        input.persistent.at(owner, U256::ONE)
    );
    assert_eq!(actual.possible_logs.len(), 1);
}

#[test]
fn forget_numeric_pays_three_constructors_without_copying_discarded_native_values() {
    let _scope = CALLS.scope();
    let mut input = store();
    let mut numeric = Native::private();
    numeric.copy_fee = usize::MAX;
    input.balance_default = EvmValue::from_numeric(numeric);
    let expected = input.forget_loop_numeric();
    CALLS.set(Calls::default());
    let domain = ValueAlgebra::new(Policy::default());
    let actual = input
        .forget_loop_numeric_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert_eq!(CALLS.get().constructions, 3);
    assert_eq!(CALLS.get().clones, 0);
    assert_eq!(actual, expected);

    let domain = ValueAlgebra::new(Policy {
        constructor_fee: usize::MAX,
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        input.forget_loop_numeric_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000),),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get().constructions, 0);
}

#[test]
fn zero_budget_precedes_every_native_hook() {
    let _scope = CALLS.scope();
    let input = store();
    let domain = ValueAlgebra::new(Policy::default());
    for operation in 0..3 {
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(0);
        let result = match operation {
            0 => input.project_to_layout_with_payment(&input, domain.borrowed(), &mut budget),
            1 => input.join_with_payment(&input, domain.borrowed(), false, &mut budget),
            _ => input.forget_loop_numeric_with_payment(domain.borrowed(), &mut budget),
        };
        assert_eq!(result, Err(CloneError::Work));
        assert_eq!(CALLS.get(), Calls::default());
    }
}

#[test]
fn native_join_widen_and_equal_refuse_before_their_own_hooks() {
    let _scope = CALLS.scope();
    let input = store();
    for (policy, widen, denied) in [
        (
            Policy {
                join_fee: usize::MAX,
                ..Policy::default()
            },
            false,
            0,
        ),
        (
            Policy {
                widen_fee: usize::MAX,
                ..Policy::default()
            },
            true,
            1,
        ),
        (
            Policy {
                equality_fee: usize::MAX,
                ..Policy::default()
            },
            false,
            2,
        ),
    ] {
        let domain = ValueAlgebra::new(policy);
        let mut with_balance = input.clone();
        with_balance
            .balances
            .insert(Address::repeat_byte(1), private());
        CALLS.set(Calls::default());
        assert_eq!(
            with_balance.join_with_payment(
                &with_balance,
                domain.borrowed(),
                widen,
                &mut WorkBudget::new(1_000_000),
            ),
            Err(CloneError::Work)
        );
        let calls = CALLS.get();
        match denied {
            0 => assert_eq!(calls.joins, 0),
            1 => {
                assert_eq!(calls.joins, 1);
                assert_eq!(calls.widens, 0);
            }
            _ => {
                assert!(calls.joins > 0);
                assert_eq!(calls.equal, 0);
            }
        }
    }
}

#[test]
fn returned_projection_copy_price_is_requoted_before_canonicalization() {
    let _scope = CALLS.scope();
    let input = store();
    let domain = ValueAlgebra::new(Policy {
        projected_copy_fee: Some(usize::MAX),
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        input.project_to_layout_with_payment(
            &input,
            domain.borrowed(),
            &mut WorkBudget::new(1_000_000),
        ),
        Err(CloneError::Work)
    );
    let calls = CALLS.get();
    assert_eq!(calls.projects, 1);
    assert_eq!(calls.clones, 2); // global staging, then the paid projection
    assert_eq!(calls.canonical, 0);
}

#[test]
fn finite_returned_copy_price_is_paid_and_one_less_stops_the_last_copy() {
    let _scope = CALLS.scope();
    let input = store();
    let mut samples = Vec::new();
    for projected_copy_fee in [64, 640] {
        let domain = ValueAlgebra::new(Policy {
            projected_copy_fee: Some(projected_copy_fee),
            ..Policy::default()
        });
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(1_000_000);
        let actual = input
            .project_to_layout_with_payment(&input, domain.borrowed(), &mut budget)
            .unwrap();
        assert_eq!(
            actual.persistent.global_default.numeric().copy_fee,
            projected_copy_fee
        );
        let calls = CALLS.get();
        assert_eq!(calls.canonical, 2);
        samples.push(budget.used());
        CALLS.set(Calls::default());
        assert_eq!(
            input.project_to_layout_with_payment(
                &input,
                domain.borrowed(),
                &mut WorkBudget::new(budget.used() - 1),
            ),
            Err(CloneError::Work)
        );
        assert_eq!(CALLS.get().clones + 1, calls.clones);
    }
    assert_eq!(samples[1] - samples[0], 2 * (640 - 64));
}

#[test]
fn returned_fold_query_price_is_requoted_before_default_discovery() {
    let _scope = CALLS.scope();
    let mut input = store();
    input
        .persistent
        .slots
        .insert((Address::repeat_byte(1), U256::ONE), word(0));
    let layout = store();
    let domain = ValueAlgebra::new(Policy {
        joined_singleton_fee: Some(usize::MAX),
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        input.project_to_layout_with_payment(
            &layout,
            domain.borrowed(),
            &mut WorkBudget::new(1_000_000),
        ),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get().joins, 1);
    assert_eq!(CALLS.get().singletons, 0);
}

#[test]
fn growing_slot_family_pays_is_top_before_widening_cleanup() {
    let _scope = CALLS.scope();
    let mut left = store();
    let owner = Address::repeat_byte(1);
    left.persistent.slots.insert((owner, U256::ONE), word(1));
    let mut right = left.clone();
    right
        .persistent
        .slots
        .insert((owner, U256::from(2)), word(2));
    let domain = ValueAlgebra::new(Policy {
        capacity: 1,
        is_top_fee: usize::MAX,
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        left.join_with_payment(
            &right,
            domain.borrowed(),
            true,
            &mut WorkBudget::new(1_000_000),
        ),
        Err(CloneError::Work)
    );
    assert!(CALLS.get().widens > 0);
    assert_eq!(CALLS.get().is_top, 0);
}

#[test]
fn growing_slot_family_keeps_previous_cells_and_initial_independence() {
    let _scope = CALLS.scope();
    let owner = Address::repeat_byte(1);
    let mut left = store();
    left.persistent.slots.insert((owner, U256::ONE), word(1));
    left.persistent
        .initial_independence
        .set_slot(owner, U256::ONE, true);
    let mut right = left.clone();
    right
        .persistent
        .slots
        .insert((owner, U256::from(2)), word(2));
    right
        .persistent
        .initial_independence
        .set_slot(owner, U256::from(2), true);
    let domain = ValueAlgebra::new(Policy {
        capacity: 1,
        ..Policy::default()
    });
    let expected = left.join(&right, domain.borrowed(), true);
    CALLS.set(Calls::default());
    let actual = left
        .join_with_payment(
            &right,
            domain.borrowed(),
            true,
            &mut WorkBudget::new(1_000_000),
        )
        .unwrap();
    assert!(CALLS.get().is_top > 0);
    assert_eq!(actual, expected);
    assert!(actual.persistent.initial_independence.at(owner, U256::ONE));
    assert!(
        !actual
            .persistent
            .initial_independence
            .at(owner, U256::from(2))
    );
}

#[test]
fn layout_copy_and_query_refuse_before_their_own_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for copy_denied in [true, false] {
        let mut input = store();
        let mut numeric = Native::private();
        if copy_denied {
            numeric.copy_fee = usize::MAX;
        } else {
            numeric.singleton_fee = usize::MAX;
        }
        input.persistent.global_default = EvmValue::from_numeric(numeric);
        CALLS.set(Calls::default());
        assert_eq!(
            input.project_to_layout_with_payment(
                &input,
                domain.borrowed(),
                &mut WorkBudget::new(1_000_000),
            ),
            Err(CloneError::Work)
        );
        let calls = CALLS.get();
        assert_eq!(calls.singletons, 0);
        assert_eq!(calls.clones, usize::from(!copy_denied));
    }
}

#[test]
fn finite_native_equality_prices_change_the_actual_store_ledger() {
    let _scope = CALLS.scope();
    let mut input = store();
    input.balances.insert(Address::repeat_byte(1), private());
    let mut samples = Vec::new();
    for equality_fee in [64, 640] {
        let domain = ValueAlgebra::new(Policy {
            equality_fee,
            ..Policy::default()
        });
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(1_000_000);
        let actual = input
            .join_with_payment(&input, domain.borrowed(), false, &mut budget)
            .unwrap();
        samples.push((budget.used(), CALLS.get().equal));
        assert!(
            !actual.balances[&Address::repeat_byte(1)]
                .numeric()
                .contains(U256::from(7))
        );
    }
    assert!(samples[0].1 > 0);
    assert_eq!(samples[0].1, samples[1].1);
    assert_eq!(samples[1].0 - samples[0].0, samples[0].1 * (640 - 64));
}

#[test]
fn both_cancellation_owners_stop_store_operations_before_native_hooks() {
    let _scope = CALLS.scope();
    let input = store();
    let domain = ValueAlgebra::new(Policy::default());
    for captured in [false, true] {
        for operation in 0..3 {
            let cancellation = analysis_control::Cancellation::new();
            let mut budget = if captured {
                analysis_control::with_cancellation(&cancellation, || WorkBudget::new(1_000_000))
            } else {
                WorkBudget::new(1_000_000)
            };
            cancellation.cancel();
            CALLS.set(Calls::default());
            let mut run = || match operation {
                0 => input.project_to_layout_with_payment(&input, domain.borrowed(), &mut budget),
                1 => input.join_with_payment(&input, domain.borrowed(), false, &mut budget),
                _ => input.forget_loop_numeric_with_payment(domain.borrowed(), &mut budget),
            };
            let result = if captured {
                run()
            } else {
                analysis_control::with_cancellation(&cancellation, run)
            };
            assert_eq!(result, Err(CloneError::Cancelled));
            assert_eq!(CALLS.get(), Calls::default());
        }
    }
}

#[test]
fn forgotten_or_conflicting_logs_keep_the_same_unknown_event_semantics() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut input = store();
    let (key, mut value) = log(0, &domain);
    let mut numeric = Native::private();
    numeric.copy_fee = usize::MAX;
    value.topics[0] = EvmValue::from_numeric(numeric);
    input.possible_logs.insert(key.clone(), value);
    let layout = store();
    let expected = input.project_to_layout(&layout, domain.borrowed());
    CALLS.set(Calls::default());
    let actual = input
        .project_to_layout_with_payment(&layout, domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert_eq!(actual, expected);
    assert!(actual.logs_unknown && actual.possible_logs.is_empty());

    let mut other = input.clone();
    other
        .possible_logs
        .get_mut(&key)
        .unwrap()
        .topics
        .push(private());
    let expected = input.join(&other, domain.borrowed(), false);
    CALLS.set(Calls::default());
    let actual = input
        .join_with_payment(
            &other,
            domain.borrowed(),
            false,
            &mut WorkBudget::new(1_000_000),
        )
        .unwrap();
    assert_eq!(actual, expected);
    assert!(actual.logs_unknown && actual.possible_logs.is_empty());
}

use super::*;
use crate::domain::ValueAlgebra;

#[path = "../metered/tests/native.rs"]
mod native;
use native::{CALLS, Calls, Native, Policy};

fn word(value: u64) -> EvmValue<Native> {
    EvmValue::constant(U256::from(value))
}
fn private() -> EvmValue<Native> {
    EvmValue::from_numeric(Native::private())
}

fn empty() -> EvmStoreSummary<Native> {
    let plane = || SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: word(0),
        initial_independence: InitialIndependence::new(false),
    };
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

fn fixture(domain: &ValueAlgebra<Policy>) -> EvmStoreSummary<Native> {
    let owner = Address::repeat_byte(1);
    let other = Address::repeat_byte(2);
    let mut out = empty();
    let retained = private()
        .with_identity(17, 2)
        .with_expression(ExprId::fresh().unwrap());
    out.persistent
        .slots
        .insert((owner, U256::ONE), retained.clone());
    out.persistent.slots.insert((other, U256::ONE), retained);
    out.persistent.defaults.insert(other, EvmValue::top());
    out.transient.slots.insert((owner, U256::ONE), private());
    out.persistent
        .initial_independence
        .set_slot(owner, U256::ONE, true);
    out.balances.insert(owner, private());
    out.balances.insert(other, private());
    out.nonces.insert(owner, word(3));
    out.codes.insert(owner, Code::Delegation(other));
    out.existence.insert(owner, Existence::Present);
    out.created.insert(owner, Some(true));
    out.pending_destruction.insert(other, Some(true));
    out.touched.insert(owner, Some(true));
    out.possible_logs.insert(
        LogKey {
            address: owner,
            code_address: other,
            code_hash: alloy_primitives::B256::ZERO,
            source_kind: crate::world::LogSourceKind::Runtime,
            pc: 0,
        },
        SummaryLog {
            topics: vec![private()],
            data: crate::world::EvmByteArray::<Native>::exact(&[1, 2])
                .summary_projection(domain.borrowed())
                .unwrap(),
        },
    );
    out
}

#[test]
fn paid_replay_retains_private_native_operands_and_independent_roots() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = fixture(&domain);
    CALLS.set(Calls::default());
    let first = summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    let second = summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert!(CALLS.get().exports > 0 && CALLS.get().clones > 0);
    for owner in [Address::repeat_byte(1), Address::repeat_byte(2)] {
        assert_ne!(
            first.persistent.roots[&owner],
            second.persistent.roots[&owner]
        );
        let cached = &first.persistent.slots[&(owner, U256::ONE)];
        assert_eq!(cached, &summary.persistent.slots[&(owner, U256::ONE)]);
        assert!(!cached.numeric().contains(U256::from(7)));
    }
    assert!(first.persistent.operands.values().any(|value| {
        value.numeric().excludes_seven
            && value.same_identity(&summary.persistent.slots[&(Address::repeat_byte(1), U256::ONE)])
    }));
    assert!(first.balance_default.expression().is_none());
    assert_ne!(
        first.balances[&Address::repeat_byte(1)].expression(),
        first.balances[&Address::repeat_byte(2)].expression()
    );
    assert_eq!(first.codes, summary.codes);
    assert_eq!(first.touched, summary.touched);
    assert_eq!(first.possible_logs.len(), summary.possible_logs.len());
}

#[test]
fn paid_projection_reads_current_roots_and_preserves_native_private_constraints() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = fixture(&domain);
    let mut runtime = summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    let owner = Address::repeat_byte(1);
    // Root facts stay private; the stale presentation cache must not win.
    runtime
        .persistent
        .slots
        .insert((owner, U256::ONE), word(99));
    let expected = runtime.summary_projection(domain.borrowed()).unwrap();
    CALLS.set(Calls::default());
    let actual = runtime
        .summary_projection_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert!(CALLS.get().projects > 0 && CALLS.get().singletons > 0);
    assert_eq!(actual, expected);
    assert!(
        !actual
            .persistent
            .at(owner, U256::ONE)
            .numeric()
            .contains(U256::from(7))
    );
    assert!(
        actual
            .persistent
            .at(owner, U256::ONE)
            .numeric()
            .observation()
            .contains(U256::from(7))
    );
}

#[test]
fn paid_summary_equality_includes_private_predicates_metadata_and_control_fields() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let left = fixture(&domain);
    let owner = Address::repeat_byte(1);
    let mut variants = vec![left.clone()];
    let mut right = left.clone();
    right
        .persistent
        .slots
        .insert((owner, U256::ONE), EvmValue::top());
    variants.push(right);
    let mut right = left.clone();
    right.balances.insert(owner, private().with_identity(29, 3));
    variants.push(right);
    let mut right = left.clone();
    right.codes.insert(owner, Code::Empty);
    variants.push(right);
    let mut right = left.clone();
    right.existence.insert(owner, Existence::Absent);
    variants.push(right);
    let mut right = left.clone();
    right.created_default = None;
    variants.push(right);
    let mut right = left.clone();
    right.pending_destruction_default = None;
    variants.push(right);
    let mut right = left.clone();
    right.touched_default = None;
    variants.push(right);
    let mut right = left.clone();
    right.logs_unknown = true;
    variants.push(right);
    let mut right = left.clone();
    let key = right.possible_logs.keys().next().unwrap().clone();
    right.possible_logs.get_mut(&key).unwrap().topics[0] = word(3);
    variants.push(right);
    for right in variants {
        let expected = left == right;
        CALLS.set(Calls::default());
        let actual = left
            .eq_with_payment(&right, domain.borrowed(), &mut WorkBudget::new(1_000_000))
            .unwrap();
        assert!(CALLS.get().equal > 0);
        assert_eq!(actual, expected);
    }
}

#[test]
fn paid_equality_uses_finite_tariffs_and_refuses_max_before_native_eq() {
    let _scope = CALLS.scope();
    let summary = empty();
    let mut samples = Vec::new();
    for equality_fee in [64, 640] {
        let domain = ValueAlgebra::new(Policy {
            equality_fee,
            ..Policy::default()
        });
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(1_000_000);
        assert!(
            summary
                .eq_with_payment(&summary, domain.borrowed(), &mut budget)
                .unwrap()
        );
        samples.push((budget.used(), CALLS.get().equal));
    }
    assert_eq!(samples[0].1, samples[1].1);
    assert_eq!(samples[1].0 - samples[0].0, samples[0].1 * (640 - 64));
    let domain = ValueAlgebra::new(Policy {
        equality_fee: usize::MAX,
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        summary.eq_with_payment(&summary, domain.borrowed(), &mut WorkBudget::new(1_000_000)),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get().equal, 0);
}

#[test]
fn replay_refuses_actual_copy_and_query_fees_before_their_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for copy in [true, false] {
        let mut summary = empty();
        let mut numeric = Native::private();
        if copy {
            numeric.copy_fee = usize::MAX;
        } else {
            numeric.singleton_fee = usize::MAX;
        }
        summary.persistent.global_default = EvmValue::from_numeric(numeric);
        CALLS.set(Calls::default());
        assert_eq!(
            summary.instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000)),
            Err(SummaryError::Budget(CloneError::Work))
        );
        assert_eq!(CALLS.get().singletons, 0);
        assert_eq!(CALLS.get().clones, usize::from(!copy));
        assert_eq!(CALLS.get().constructions, 0);
    }
}

#[test]
fn replay_refuses_large_finite_copy_and_query_prices_without_unpaid_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for copy in [true, false] {
        let mut summary = empty();
        let mut numeric = Native::private();
        if copy {
            numeric.copy_fee = 100_000;
        } else {
            numeric.singleton_fee = 100_000;
        }
        summary.persistent.global_default = EvmValue::from_numeric(numeric);
        let original = summary.clone();
        CALLS.set(Calls::default());
        assert_eq!(
            summary.instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(10_000)),
            Err(SummaryError::Budget(CloneError::Work))
        );
        assert_eq!(CALLS.get().singletons, 0);
        assert_eq!(CALLS.get().clones, usize::from(!copy));
        assert_eq!(summary, original);
    }
}

#[test]
fn replay_charges_finite_query_price_on_the_actual_copied_default() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut samples = Vec::new();
    for singleton_fee in [32, 320] {
        let mut summary = empty();
        let mut numeric = Native::private();
        numeric.exact = Some(U256::ZERO);
        numeric.singleton_fee = singleton_fee;
        summary.persistent.global_default = EvmValue::from_numeric(numeric);
        CALLS.set(Calls::default());
        let mut budget = WorkBudget::new(1_000_000);
        summary
            .instantiate_with_payment(domain.borrowed(), &mut budget)
            .unwrap();
        assert_eq!(CALLS.get().singletons, 2);
        samples.push(budget.used());
    }
    assert_eq!(samples[1] - samples[0], 320 - 32);
}

#[test]
fn replay_validation_does_not_construct_synthetic_native_top_values() {
    let _scope = CALLS.scope();
    let summary = empty();
    let domain = ValueAlgebra::new(Policy {
        constructor_fee: usize::MAX,
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert_eq!(CALLS.get().constructions, 0);
}

#[test]
fn projection_requotes_actual_projected_result_before_canonicalization() {
    let _scope = CALLS.scope();
    let summary = empty();
    let ordinary = ValueAlgebra::new(Policy::default());
    let runtime = summary
        .instantiate_with_payment(ordinary.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    let expensive = ValueAlgebra::new(Policy {
        projected_copy_fee: Some(usize::MAX),
        ..Policy::default()
    });
    CALLS.set(Calls::default());
    assert_eq!(
        runtime
            .summary_projection_with_payment(expensive.borrowed(), &mut WorkBudget::new(1_000_000)),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(CALLS.get().projects, 1);
    assert_eq!(CALLS.get().canonical, 0);
}

#[test]
fn static_touches_preserve_roots_private_values_and_existing_must_touches() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let original = fixture(&domain);
    let runtime = original
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    let mut effects = empty();
    effects.touched_default = None;
    effects.touched.insert(Address::repeat_byte(2), Some(true));
    let expected = effects.apply_static_touches(&runtime);
    CALLS.set(Calls::default());
    let actual = effects
        .apply_static_touches_with_payment(&runtime, &mut WorkBudget::new(1_000_000))
        .unwrap();
    assert_eq!(CALLS.get().singletons, 0);
    assert_eq!(CALLS.get().constructions, 0);
    assert_eq!(actual, expected);
    assert_eq!(actual.persistent.roots, runtime.persistent.roots);
    assert_eq!(actual.persistent.operands, runtime.persistent.operands);
    assert_eq!(
        actual.touched_in_transaction(Address::repeat_byte(1)),
        Some(true)
    );
    assert_eq!(
        actual.touched_in_transaction(Address::repeat_byte(2)),
        Some(true)
    );
}

#[test]
fn static_delivery_refuses_snapshot_copy_before_cloning_native_payloads() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = empty();
    let mut runtime = summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    let mut numeric = Native::private();
    numeric.copy_fee = usize::MAX;
    runtime.balance_default = EvmValue::from_numeric(numeric);
    CALLS.set(Calls::default());
    assert_eq!(
        summary.apply_static_touches_with_payment(&runtime, &mut WorkBudget::new(1_000_000)),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get().clones, 0);
    assert_eq!(CALLS.get().singletons, 0);
}

#[test]
fn caller_errors_and_metadata_limits_remain_typed_sources() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut summary = empty();
    summary
        .balances
        .insert(Address::repeat_byte(1), private().with_symbolic_limit());
    assert!(matches!(
        summary.instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000)),
        Err(SummaryError::Source(_))
    ));
    let mut runtime = empty()
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    runtime.persistent.failure = Some(StoreError::Expression(
        crate::domain::symbolic::ExprError::Limit {
            nodes: usize::MAX,
            depth: 1,
        },
    ));
    assert!(matches!(
        runtime.summary_projection_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000)),
        Err(SummaryError::Source(_))
    ));
}

#[test]
fn caller_entrypoints_reject_zero_budget_without_native_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = empty();
    let runtime = summary
        .instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(1_000_000))
        .unwrap();
    CALLS.set(Calls::default());
    assert_eq!(
        summary.instantiate_with_payment(domain.borrowed(), &mut WorkBudget::new(0)),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(
        runtime.summary_projection_with_payment(domain.borrowed(), &mut WorkBudget::new(0)),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(
        summary.eq_with_payment(&summary, domain.borrowed(), &mut WorkBudget::new(0)),
        Err(CloneError::Work)
    );
    assert_eq!(
        summary.apply_static_touches_with_payment(&runtime, &mut WorkBudget::new(0)),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get(), Calls::default());
}

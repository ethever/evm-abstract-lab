//! Value-specific seam counters distinguish the targeted refusal from old-input refusal.
use super::super::super::{InitialIndependence, OrderedMap, SummaryPlane};
use super::super::{EvmStoreSummary, ProcessOwner};
use crate::{
    domain::{EvmValue, ValueAlgebra, WordElement},
    resource::{CloneError, SummaryError, WorkBudget},
};
use alloy_primitives::{Address, U256};
#[path = "fee_carrier.rs"]
mod carrier;
use carrier::{CALLS, Native, Policy, configure, targets};
fn state() -> EvmStoreSummary<Native> {
    let zero = || EvmValue::constant(U256::ZERO);
    let plane = || SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: zero(),
        initial_independence: InitialIndependence::new(true),
    };
    EvmStoreSummary {
        persistent: plane(),
        transient: plane(),
        balances: OrderedMap::new(),
        balance_default: zero(),
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
#[test]
fn returned_top_clone_refuses_at_actual_second_map_hook_before_copy() {
    let _scope = CALLS.scope();
    for copy_fee in [usize::MAX, 10_000_000] {
        configure(copy_fee, 32);
        let d = ValueAlgebra::new(Policy::default());
        let before = state();
        let mut budget = WorkBudget::new(100_000);
        let result = before.process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget,
        );
        assert!(matches!(result, Err(CloneError::Work)));
        let hooks = targets();
        assert_eq!(
            hooks.factories, 2,
            "both actual Top factories must be reached"
        );
        assert!(
            hooks.copy_quotes > 0,
            "the newly returned first Top must be quoted"
        );
        assert_eq!(hooks.copies, 0, "refusal precedes that Top's clone hook");
        assert!(CALLS.get().clones > 0, "old cheap input copies really ran");
        assert!(before.balances.is_empty() && before.nonces.is_empty() && before.codes.is_empty());
        assert!(budget.used() > 0);
    }
}
#[test]
fn finite_returned_top_clone_is_paid_and_reaches_the_target_copy() {
    let _scope = CALLS.scope();
    configure(50_000, 32);
    let d = ValueAlgebra::new(Policy::default());
    let before = state();
    let mut budget = WorkBudget::new(1_000_000);
    let child = before
        .process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    let hooks = targets();
    assert_eq!(hooks.factories, 2);
    assert!(hooks.copy_quotes > 0 && hooks.copies > 0);
    assert!(budget.used() >= 50_000);
    let mut refused = WorkBudget::new(20_000);
    assert!(matches!(
        child.process_deploy(
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut refused
        ),
        Err(CloneError::Work)
    ));
    assert!(
        child
            .codes
            .values()
            .all(|c| matches!(c, crate::world::Code::Empty))
    );
}
#[test]
fn replay_quotes_actual_returned_top_query_before_invoking_it() {
    let _scope = CALLS.scope();
    configure(64, usize::MAX);
    let d = ValueAlgebra::new(Policy::default());
    let before = state();
    let mut budget = WorkBudget::new(10_000_000);
    let child = before
        .process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    let before_hooks = targets();
    assert_eq!(before_hooks.factories, 2);
    assert!(matches!(
        child.instantiate_with_payment(d.borrowed(), &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    ));
    let hooks = targets();
    assert!(hooks.query_quotes > 0);
    assert_eq!(hooks.queries, 0, "MAX refuses before the new Top query");
    assert!(child.balances.len() == 2 && before.balances.is_empty());
}
#[test]
fn constructor_quote_refuses_before_factory_and_does_not_publish() {
    let _scope = CALLS.scope();
    configure(64, 32);
    let d = ValueAlgebra::new(Policy {
        constructor_fee: usize::MAX,
        ..Policy::default()
    });
    let before = state();
    let mut budget = WorkBudget::new(10_000_000);
    assert!(matches!(
        before.process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget
        ),
        Err(CloneError::Work)
    ));
    assert_eq!(targets().factories, 0);
    assert!(before.balances.is_empty());
}
#[test]
fn cancellation_before_staging_preserves_source_and_never_invokes_factory() {
    let _scope = CALLS.scope();
    configure(64, 32);
    let d = ValueAlgebra::new(Policy::default());
    let before = state();
    let payer = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(10_000_000));
    payer.cancel();
    assert!(matches!(
        before.process_begin_constructor(
            ProcessOwner::Residual,
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget
        ),
        Err(CloneError::Cancelled)
    ));
    assert_eq!(targets().factories, 0);
    assert!(before.codes.is_empty());
}

#[test]
fn private_non_target_native_predicates_and_typed_replay_source_survive() {
    let _scope = CALLS.scope();
    configure(64, 32);
    let d = ValueAlgebra::new(Policy::default());
    let mut before = state();
    let other = Address::repeat_byte(3);
    before
        .balances
        .insert(other, EvmValue::from_numeric(Native::private()));
    let mut budget = WorkBudget::new(10_000_000);
    let child = before
        .process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert!(
        !child
            .balances
            .get(&other)
            .unwrap()
            .numeric()
            .contains(U256::from(7))
    );
    assert!(
        child
            .balances
            .get(&other)
            .unwrap()
            .numeric()
            .contains(U256::from(8))
    );
    let mut poisoned = before;
    poisoned.balances.insert(
        other,
        EvmValue::from_numeric(Native::private()).with_symbolic_limit(),
    );
    let child = poisoned
        .process_begin_constructor(
            ProcessOwner::Named(Address::repeat_byte(1)),
            ProcessOwner::Named(Address::repeat_byte(2)),
            d.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert!(matches!(
        child.instantiate_with_payment(d.borrowed(), &mut budget),
        Err(SummaryError::Source(_))
    ));
    // The successful Store factory did not reinterpret a source failure as gas
    // failure, recover for free, erase the source flag, or mutate its input.
    assert!(
        poisoned
            .balances
            .get(&other)
            .unwrap()
            .symbolic_limit_reached()
    );
}

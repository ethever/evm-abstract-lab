//! Actual native hook counters, not estimates from idle CPU or wall time.

use super::{EvmStoreSummary, InitialIndependence, OrderedMap, SummaryPlane};
use crate::{
    domain::{EvmNumericAlgebra, EvmValue, ValueAlgebra, WordElement},
    resource::{CloneError, WorkBudget},
    world::process::{
        CallRoles, OwnerCatalogue, OwnerCatalogueLimits, ProcessAction, ProcessCallOperands,
        ProcessOwnerError, ProcessTargetAddress,
    },
};
use alloy_primitives::{Address, U256};

// Share the native counter carrier used by metered and payment controls.
use crate::world::store::summary::test_support::{CALLS, Calls, Native, Policy};

fn state() -> EvmStoreSummary<Native> {
    let plane = || SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: EvmValue::top(),
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

fn catalogue(names: &[Address]) -> std::sync::Arc<OwnerCatalogue> {
    OwnerCatalogue::freeze(
        names,
        &[],
        OwnerCatalogueLimits {
            names: names.len(),
            codes: 0,
        },
        &mut WorkBudget::new(1_000_000),
    )
    .unwrap()
}

#[test]
fn zero_work_and_expensive_top_refuse_before_any_native_constructor_query_or_clone() {
    let input = state();
    let catalogue = catalogue(&[]);
    for available in [0, 100_000] {
        let domain = ValueAlgebra::new(Policy {
            constructor_fee: 1_000_000,
            ..Policy::default()
        });
        let _scope = CALLS.scope();
        assert_eq!(
            input.process_complement(
                &catalogue,
                domain.borrowed(),
                &mut WorkBudget::new(available)
            ),
            Err(CloneError::Work)
        );
        assert_eq!(CALLS.get(), Calls::default());
    }
}

#[test]
fn expensive_actual_retained_copy_is_paid_before_private_numeric_clone() {
    let mut input = state();
    let owner = Address::repeat_byte(1);
    let mut native = Native::private();
    native.copy_fee = 1_000_000;
    input
        .persistent
        .defaults
        .insert(owner, EvmValue::from_numeric(native));
    let catalogue = catalogue(&[owner]);
    let domain = ValueAlgebra::new(Policy::default());
    let _scope = CALLS.scope();
    assert_eq!(
        input.process_complement(&catalogue, domain.borrowed(), &mut WorkBudget::new(100_000)),
        Err(CloneError::Work)
    );
    let calls = CALLS.get();
    assert_eq!(calls.clones, 0);
    assert_eq!(calls.singletons, 0);
    assert_eq!(
        calls.equal, 1,
        "the admitted comparison precedes the separately refused copy"
    );
    assert_eq!(calls.constructions, 1);
}

#[test]
fn actual_native_equality_tariff_precedes_the_comparison() {
    let mut input = state();
    let owner = Address::repeat_byte(1);
    input
        .persistent
        .defaults
        .insert(owner, EvmValue::from_numeric(Native::private()));
    let catalogue = catalogue(&[owner]);
    let domain = ValueAlgebra::new(Policy {
        equality_fee: 1_000_000,
        ..Policy::default()
    });
    let _scope = CALLS.scope();
    assert_eq!(
        input.process_complement(&catalogue, domain.borrowed(), &mut WorkBudget::new(100_000)),
        Err(CloneError::Work)
    );
    let calls = CALLS.get();
    assert_eq!(calls.equal, 0);
    assert_eq!(calls.clones, 0);
    assert_eq!(calls.singletons, 0);
    assert_eq!(calls.constructions, 1);
}

#[test]
fn independent_address_binding_pays_its_native_constructor_and_original_cancellation() {
    let parent = ProcessCallOperands {
        address: EvmValue::top(),
        caller: EvmValue::top(),
        value: EvmValue::top(),
    };
    let roles = CallRoles::for_action(ProcessAction::Call, false).unwrap();
    let domain = ValueAlgebra::new(Policy {
        constructor_fee: 1_000_000,
        ..Policy::default()
    });
    let token = analysis_control::Cancellation::new();
    let cancelled = analysis_control::with_cancellation(&token, || WorkBudget::new(2_000_000));
    token.cancel();
    for (mut budget, expected) in [
        (WorkBudget::new(100_000), ProcessOwnerError::Work),
        (cancelled, ProcessOwnerError::Cancelled),
    ] {
        let _scope = CALLS.scope();
        assert_eq!(
            roles
                .bind(
                    ProcessTargetAddress::Independent,
                    &parent,
                    domain.borrowed(),
                    &mut budget
                )
                .unwrap_err(),
            expected
        );
        assert_eq!(CALLS.get(), Calls::default());
    }
}

#[test]
fn paid_process_preparation_retains_private_named_predicates() {
    let mut input = state();
    let owner = Address::repeat_byte(1);
    input.persistent.slots.insert(
        (owner, U256::ONE),
        EvmValue::from_numeric(Native::private()),
    );
    let catalogue = catalogue(&[owner]);
    let domain = ValueAlgebra::new(Policy::default());
    let prepared = input
        .process_complement(
            &catalogue,
            domain.borrowed(),
            &mut WorkBudget::new(1_000_000),
        )
        .unwrap();
    assert!(
        !prepared
            .persistent
            .slots
            .get(&(owner, U256::ONE))
            .unwrap()
            .contains(U256::from(7))
    );
    assert!(prepared.persistent.global_default.contains(U256::from(7)));
}

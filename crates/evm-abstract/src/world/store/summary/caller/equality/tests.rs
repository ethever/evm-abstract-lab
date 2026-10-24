use super::super::SummaryLog;
use super::*;
use crate::{
    bytecode::Program,
    domain::ValueAlgebra,
    world::{
        Code, EvmByteArray, Existence, LogSourceKind,
        store::summary::test_support::{CALLS, Calls, Native, Policy},
    },
};

mod reference;

fn owner() -> Address {
    Address::repeat_byte(1)
}

fn slot() -> (Address, U256) {
    (owner(), U256::ONE)
}

fn word() -> EvmValue<Native> {
    EvmValue::from_numeric(Native::private())
}

fn key() -> LogKey {
    LogKey {
        address: owner(),
        code_address: Address::repeat_byte(2),
        code_hash: alloy_primitives::B256::ZERO,
        source_kind: LogSourceKind::Runtime,
        pc: 3,
    }
}

fn fixture(domain: &ValueAlgebra<Policy>) -> EvmStoreSummary<Native> {
    let plane = || SummaryPlane {
        slots: [(slot(), word())].into_iter().collect(),
        defaults: [(owner(), word())].into_iter().collect(),
        global_default: word(),
        initial_independence: InitialIndependence {
            slots: [(slot(), true)].into_iter().collect(),
            defaults: [(owner(), true)].into_iter().collect(),
            global_default: false,
        },
    };
    EvmStoreSummary {
        persistent: plane(),
        transient: plane(),
        balances: [(owner(), word())].into_iter().collect(),
        balance_default: word(),
        codes: [(
            owner(),
            Code::Runtime(Program::decode(&[0x60, 1, 0]).unwrap()),
        )]
        .into_iter()
        .collect(),
        nonces: [(owner(), word())].into_iter().collect(),
        existence: [(owner(), Existence::Present)].into_iter().collect(),
        created: [(owner(), Some(true))].into_iter().collect(),
        created_default: Some(false),
        pending_destruction: [(owner(), Some(true))].into_iter().collect(),
        pending_destruction_default: Some(false),
        touched: [(owner(), Some(true))].into_iter().collect(),
        touched_default: Some(false),
        possible_logs: [(
            key(),
            SummaryLog {
                topics: vec![word()],
                data: EvmByteArray::<Native>::exact(&[1, 2])
                    .summary_projection(domain.borrowed())
                    .unwrap(),
            },
        )]
        .into_iter()
        .collect(),
        logs_unknown: false,
    }
}

// One different field per case, including fixed owner coordinates and the
// private numeric predicate invisible in the carrier's observation.
fn variant(value: &mut EvmStoreSummary<Native>, case: usize, domain: &ValueAlgebra<Policy>) {
    match case {
        0 => {}
        1 => {
            let mut numeric = Native::private();
            numeric.excludes_seven = false;
            value
                .persistent
                .slots
                .insert(slot(), EvmValue::from_numeric(numeric));
        }
        2 => value.persistent.defaults.clear(),
        3 => value.persistent.global_default = EvmValue::top(),
        4 => value.persistent.initial_independence.slots.clear(),
        5 => value.persistent.initial_independence.defaults.clear(),
        6 => value.persistent.initial_independence.global_default = true,
        7 => value.transient.slots.clear(),
        8 => value.transient.defaults.clear(),
        9 => value.transient.global_default = EvmValue::top(),
        10 => value.transient.initial_independence.slots.clear(),
        11 => value.transient.initial_independence.defaults.clear(),
        12 => value.transient.initial_independence.global_default = true,
        13 => value.balances.clear(),
        14 => value.balance_default = EvmValue::top(),
        15 => value.codes.clear(),
        16 => *value.codes.get_mut(&owner()).unwrap() = Code::Empty,
        17 => {
            value.codes.insert(
                owner(),
                Code::Runtime(Program::decode(&[0x60, 2, 0]).unwrap()),
            );
        }
        18 => value.nonces.clear(),
        19 => {
            value.existence.insert(owner(), Existence::Absent);
        }
        20 => value.created.clear(),
        21 => value.created_default = None,
        22 => value.pending_destruction.clear(),
        23 => value.pending_destruction_default = None,
        24 => value.touched.clear(),
        25 => value.touched_default = None,
        26 => value.possible_logs.clear(),
        27..=31 => {
            let log = value.possible_logs.remove(&key()).unwrap();
            let mut changed = key();
            match case {
                27 => changed.address = Address::repeat_byte(3),
                28 => changed.code_address = Address::repeat_byte(3),
                29 => changed.code_hash = alloy_primitives::B256::repeat_byte(1),
                30 => changed.source_kind = LogSourceKind::Init,
                _ => changed.pc += 1,
            }
            value.possible_logs.insert(changed, log);
        }
        32 => value.possible_logs.get_mut(&key()).unwrap().topics.clear(),
        33 => value.possible_logs.get_mut(&key()).unwrap().topics[0] = EvmValue::top(),
        34 => {
            value.possible_logs.get_mut(&key()).unwrap().data =
                EvmByteArray::<Native>::exact(&[2, 1])
                    .summary_projection(domain.borrowed())
                    .unwrap();
        }
        35 => value.logs_unknown = true,
        36 => {
            value
                .persistent
                .slots
                .insert(slot(), word().with_identity(9, 2));
        }
        37 => {
            value.persistent.slots.insert(
                slot(),
                word().with_expression(crate::domain::symbolic::ExprId::fresh().unwrap()),
            );
        }
        38 => {
            let original = value.persistent.slots.remove(&slot()).unwrap();
            value
                .persistent
                .slots
                .insert((Address::repeat_byte(3), slot().1), original);
        }
        39 => {
            value.created.insert(owner(), None);
        }
        40 => {
            value.pending_destruction.insert(owner(), None);
        }
        41 => {
            value.touched.insert(owner(), None);
        }
        42 => {
            let code = value.codes.remove(&owner()).unwrap();
            value.codes.insert(Address::repeat_byte(3), code);
        }
        43 => {
            value.codes.insert(
                owner(),
                Code::Runtime(Program::decode(&[0x60, 1, 0]).unwrap()),
            );
        }
        44 => {
            value.persistent.slots.insert(
                slot(),
                word().with_symbol(crate::domain::identity::Symbol::Caller, Some(9)),
            );
        }
        45 => {
            let mut sourced = word();
            sourced.set_origin(crate::domain::provenance::Origin::Memory);
            value.persistent.slots.insert(slot(), sourced);
        }
        46 => {
            value
                .persistent
                .slots
                .insert(slot(), word().with_symbolic_limit());
        }
        47 => {
            value
                .persistent
                .initial_independence
                .slots
                .insert(slot(), false);
        }
        48 => {
            value
                .persistent
                .initial_independence
                .defaults
                .insert(owner(), false);
        }
        49 => {
            value
                .transient
                .initial_independence
                .slots
                .insert(slot(), false);
        }
        50 => {
            value
                .transient
                .initial_independence
                .defaults
                .insert(owner(), false);
        }
        _ => unreachable!(),
    }
}

#[test]
fn all_fields_and_native_visits_match_the_funded_previous_comparator() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let left = fixture(&domain);
    for case in 0..51 {
        let mut right = left.clone();
        variant(&mut right, case, &domain);
        let left_before = left.clone();
        let right_before = right.clone();
        let expected = left == right;
        // Temporary runtime identities are deliberately absent from persistent
        // Eq; immutable input identity, provenance and expressions remain in it.
        assert_eq!(expected, matches!(case, 0 | 36 | 43), "case {case}");
        if case == 1 {
            assert_eq!(
                left.persistent.slots[&slot()].numeric().observation(),
                right.persistent.slots[&slot()].numeric().observation(),
            );
        }
        CALLS.set(Calls::default());
        let old = left.reference_eq_with_payment(
            &right,
            domain.borrowed(),
            &mut WorkBudget::new(1_000_000),
        );
        let old_calls = CALLS.get();
        CALLS.set(Calls::default());
        let actual =
            left.eq_with_payment(&right, domain.borrowed(), &mut WorkBudget::new(1_000_000));
        assert_eq!(actual, old, "case {case}");
        assert_eq!(actual, Ok(expected), "case {case}");
        assert_eq!(CALLS.get(), old_calls, "case {case}");
        assert_eq!(left, left_before);
        assert_eq!(right, right_before);
    }
}

#[test]
fn exact_budget_and_minus_one_preserve_results_or_refuse_without_mutation() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let left = fixture(&domain);
    for case in [0, 1, 4, 19, 27, 33, 34, 35] {
        let mut right = left.clone();
        variant(&mut right, case, &domain);
        let before = right.clone();
        let mut measured = WorkBudget::new(1_000_000);
        let expected = left
            .eq_with_payment(&right, domain.borrowed(), &mut measured)
            .unwrap();
        let cost = measured.used();
        let mut exact = WorkBudget::new(cost);
        assert_eq!(
            left.eq_with_payment(&right, domain.borrowed(), &mut exact),
            Ok(expected)
        );
        assert_eq!(exact.used(), cost);
        let mut short = WorkBudget::new(cost - 1);
        assert_eq!(
            left.eq_with_payment(&right, domain.borrowed(), &mut short),
            Err(CloneError::Work)
        );
        let used = short.used();
        CALLS.set(Calls::default());
        assert_eq!(
            left.eq_with_payment(&right, domain.borrowed(), &mut short),
            Err(CloneError::Work)
        );
        assert_eq!(short.used(), used);
        assert_eq!(CALLS.get(), Calls::default());
        assert_eq!(right, before);
    }
}

#[test]
fn current_and_captured_cancellation_and_zero_budget_precede_native_hooks() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let summary = fixture(&domain);
    for captured in [false, true] {
        let cancellation = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&cancellation, || WorkBudget::new(1_000_000))
        } else {
            WorkBudget::new(1_000_000)
        };
        cancellation.cancel();
        CALLS.set(Calls::default());
        let mut run = || summary.eq_with_payment(&summary, domain.borrowed(), &mut budget);
        let result = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&cancellation, run)
        };
        assert_eq!(result, Err(CloneError::Cancelled));
        assert_eq!(budget.used(), 0);
        assert_eq!(CALLS.get(), Calls::default());
    }
    CALLS.set(Calls::default());
    assert_eq!(
        summary.eq_with_payment(&summary, domain.borrowed(), &mut WorkBudget::new(0)),
        Err(CloneError::Work)
    );
    assert_eq!(CALLS.get(), Calls::default());
}

#[test]
fn max_and_large_finite_native_prices_refuse_before_eq_even_on_metadata_mismatch() {
    let _scope = CALLS.scope();
    let ordinary = ValueAlgebra::new(Policy::default());
    let left = fixture(&ordinary);
    let mut right = left.clone();
    right.logs_unknown = true;
    for equality_fee in [usize::MAX, 1_000_000] {
        let domain = ValueAlgebra::new(Policy {
            equality_fee,
            ..Policy::default()
        });
        CALLS.set(Calls::default());
        assert_eq!(
            left.eq_with_payment(&right, domain.borrowed(), &mut WorkBudget::new(100_000)),
            Err(CloneError::Work)
        );
        assert_eq!(CALLS.get(), Calls::default());
    }
}

#[test]
fn readonly_structure_savings_leave_the_full_scalar_tariff_delta() {
    let _scope = CALLS.scope();
    let ordinary = ValueAlgebra::new(Policy::default());
    let summary = fixture(&ordinary);
    let mut samples = Vec::new();
    for equality_fee in [64, 640] {
        let domain = ValueAlgebra::new(Policy {
            equality_fee,
            ..Policy::default()
        });
        let mut old = WorkBudget::new(1_000_000);
        CALLS.set(Calls::default());
        assert!(
            summary
                .reference_eq_with_payment(&summary, domain.borrowed(), &mut old)
                .unwrap()
        );
        let old_calls = CALLS.get();
        let mut new = WorkBudget::new(1_000_000);
        CALLS.set(Calls::default());
        assert!(
            summary
                .eq_with_payment(&summary, domain.borrowed(), &mut new)
                .unwrap()
        );
        assert_eq!(CALLS.get(), old_calls);
        assert!(new.used() < old.used());
        println!(
            "equality_fee={equality_fee} old={} readonly={} native_eq={}",
            old.used(),
            new.used(),
            old_calls.equal,
        );
        samples.push((new.used(), CALLS.get().equal));
    }
    assert!(samples[0].1 > 0);
    assert_eq!(samples[0].1, samples[1].1);
    assert_eq!(samples[1].0 - samples[0].0, samples[0].1 * (640 - 64));
}

#[test]
fn larger_trees_keep_all_coordinates_and_native_visits() {
    let _scope = CALLS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let mut left = fixture(&domain);
    for index in 0..257 {
        let coordinate = (owner(), U256::from(index));
        left.persistent.slots.insert(coordinate, word());
        left.persistent
            .initial_independence
            .slots
            .insert(coordinate, true);
    }
    for different in [false, true] {
        let mut right = left.clone();
        if different {
            right
                .persistent
                .slots
                .insert((owner(), U256::from(256)), EvmValue::top());
        }
        let mut old_budget = WorkBudget::new(20_000_000);
        CALLS.set(Calls::default());
        let expected = left.reference_eq_with_payment(&right, domain.borrowed(), &mut old_budget);
        let old_calls = CALLS.get();
        let mut new_budget = WorkBudget::new(20_000_000);
        CALLS.set(Calls::default());
        assert_eq!(
            left.eq_with_payment(&right, domain.borrowed(), &mut new_budget),
            expected
        );
        assert_eq!(expected, Ok(!different));
        assert_eq!(CALLS.get(), old_calls);
        if different {
            assert_eq!(old_calls.equal, 257);
        } else {
            assert!(old_calls.equal > 257);
        }
        assert!(new_budget.used() < old_budget.used());
    }
}

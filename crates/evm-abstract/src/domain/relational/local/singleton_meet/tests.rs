use super::super::{Engine, LocalResult, Stop, Work};
use super::{ADMISSION_WORK, CANONICAL_WORK, CLONE_WORK, MEMBERSHIP_WORK, WORD_LIMBS};
use crate::{
    domain::{
        Domain, DomainSpec, FiniteConstantSet, NumericValue, Profile,
        congruence::Congruence,
        interval::Interval,
        known_bits::KnownBits,
        relational::{QueryReason, RelationLimits, RelationState, ValueQuery, intersect_numeric},
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::{collections::BTreeSet, num::NonZeroUsize};

fn engine_meet(
    left: &NumericValue,
    right: &NumericValue,
    domain: Domain,
    budget: &mut WorkBudget,
) -> LocalResult<NumericValue> {
    let limits = RelationLimits::default();
    let mut work = Work {
        remaining: limits.max_nodes,
        cancellation: analysis_control::current_cancellation(),
        ledger: Some(budget),
    };
    Engine::new(&mut work, &limits, domain).meet(left, right)
}

fn assert_generic(left: &NumericValue, right: &NumericValue, domain: Domain) {
    let expected = intersect_numeric(left, right, domain);
    let mut budget = WorkBudget::new(20_000_000);
    match engine_meet(left, right, domain, &mut budget) {
        Ok(value) => assert_eq!(Some(value), expected),
        Err(Stop::Bottom) => assert_eq!(expected, None),
        Err(other) => panic!("unexpected interruption: {other:?}"),
    }
    assert!(!budget.exhausted());
}

fn frozen_domains() -> [Domain; 6] {
    // Selected boundaries, rather than a full Cartesian policy matrix.
    [
        (Profile::Product, 1, 1, 1, false),
        (Profile::Product, 8, 4, 256, true),
        (Profile::Product, 512, 1, 2, true),
        (Profile::ConstantsOnly, 1, 1, 1, true),
        (Profile::ConstantsOnly, 8, 4, 256, false),
        (Profile::ConstantsOnly, 512, 1, 2, true),
    ]
    .map(|(profile, capacity, rounds, facts, enabled)| {
        let relations = RelationLimits {
            enabled,
            ..RelationLimits::default()
        };
        Domain::from_spec(
            DomainSpec::new(
                profile,
                NonZeroUsize::new(capacity).unwrap(),
                NonZeroUsize::new(rounds).unwrap(),
                NonZeroUsize::new(facts).unwrap(),
            )
            .with_relations(relations),
        )
    })
}

fn finite(values: impl IntoIterator<Item = U256>) -> NumericValue {
    NumericValue {
        finite: FiniteConstantSet::try_from_values(values.into_iter().collect()).unwrap(),
        ..NumericValue::top()
    }
}

#[test]
fn canonical_intersections_match_full_generic_components_and_frozen_profiles() {
    let zero = U256::ZERO;
    let one = U256::ONE;
    let seven = U256::from(7);
    let sign = one << 255;
    let boolean = NumericValue::unsigned_range(zero, one).unwrap();
    let nonzero = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let finite = finite([U256::from(3), seven, U256::from(9)]);
    let one_bit = NumericValue {
        bits: KnownBits::new(zero, U256::from(4)).unwrap(),
        ..NumericValue::top()
    };
    let zero_bit = NumericValue {
        bits: KnownBits::new(U256::from(4), zero).unwrap(),
        ..NumericValue::top()
    };
    let unsigned = NumericValue::unsigned_range(U256::from(5), U256::from(9)).unwrap();
    let signed = NumericValue {
        interval: Interval::new_signed((U256::MAX - U256::from(2)) ^ sign, U256::MAX ^ sign)
            .unwrap(),
        ..NumericValue::top()
    };
    let exact_congruence = NumericValue {
        congruence: Congruence::exact(seven),
        ..NumericValue::top()
    };
    let modulo = NumericValue {
        congruence: Congruence::new(U256::from(3), one).unwrap(),
        ..NumericValue::top()
    };
    let all_components = NumericValue {
        finite: FiniteConstantSet::try_from_values(BTreeSet::from([one, seven])).unwrap(),
        bits: KnownBits::new(zero, one).unwrap(),
        interval: Interval::new_unsigned(U256::from(5), U256::from(9)).unwrap(),
        congruence: Congruence::exact(seven),
        nonzero: true,
    };
    for (word, other) in [
        (zero, &boolean),
        (one, &boolean),
        (U256::from(2), &boolean),
        (seven, &nonzero),
        (zero, &nonzero),
        (seven, &finite),
        (U256::from(5), &finite),
        (seven, &one_bit),
        (U256::from(3), &one_bit),
        (U256::from(3), &zero_bit),
        (seven, &zero_bit),
        (seven, &unsigned),
        (U256::from(4), &unsigned),
        (U256::MAX, &signed),
        (seven, &signed),
        (seven, &exact_congruence),
        (U256::from(3), &exact_congruence),
        (seven, &modulo),
        (U256::from(8), &modulo),
        (seven, &all_components),
        (one, &all_components),
    ] {
        let canonical = NumericValue::constant(word);
        for domain in frozen_domains() {
            assert_generic(&canonical, other, domain);
            assert_generic(other, &canonical, domain);
        }
    }
}

#[test]
fn noncanonical_singletons_general_modulo_and_unproved_ranges_keep_generic_path() {
    let word = U256::from(7);
    let nonzero = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    for noncanonical in [
        finite([word]),
        NumericValue {
            bits: KnownBits::exact(word),
            ..NumericValue::top()
        },
        NumericValue {
            interval: Interval::exact(word),
            ..NumericValue::top()
        },
        NumericValue {
            congruence: Congruence::exact(word),
            ..NumericValue::top()
        },
        NumericValue {
            nonzero: false,
            ..NumericValue::constant(word)
        },
        NumericValue::unsigned_range(U256::ZERO, U256::ONE).unwrap(),
    ] {
        for (left, right) in [(&noncanonical, &nonzero), (&nonzero, &noncanonical)] {
            let mut work = Work {
                remaining: 1,
                cancellation: analysis_control::current_cancellation(),
                ledger: None,
            };
            assert!(
                super::meet(left, right, Domain::default(), &mut work)
                    .unwrap()
                    .is_none()
            );
            for domain in frozen_domains() {
                assert_generic(left, right, domain);
            }
        }
    }
    let modulo = NumericValue {
        congruence: Congruence::new(U256::from(3), U256::ONE).unwrap(),
        ..NumericValue::top()
    };
    let mut work = Work {
        remaining: 1,
        cancellation: analysis_control::current_cancellation(),
        ledger: None,
    };
    assert!(
        super::meet(
            &NumericValue::constant(word),
            &modulo,
            Domain::default(),
            &mut work
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn large_finite_membership_pays_input_bound_without_generic_reduction() {
    let other = finite((0_u64..512).map(U256::from));
    let canonical = NumericValue::constant(U256::from(511));
    let expected_work = canonical.work_size()
        + other.work_size()
        + 1
        + CANONICAL_WORK
        + ADMISSION_WORK
        + MEMBERSHIP_WORK
        + 512 * WORD_LIMBS
        + CLONE_WORK;
    let mut budget = WorkBudget::new(expected_work);
    assert_eq!(
        engine_meet(&canonical, &other, Domain::default(), &mut budget).unwrap(),
        canonical
    );
    assert_eq!(budget.used(), expected_work);
    assert!(!budget.exhausted());
    assert_generic(&canonical, &other, Domain::default());
}

#[test]
fn each_refused_stage_keeps_paid_prefix_and_does_not_reserve_generic_fallback() {
    let canonical = NumericValue::constant(U256::ONE);
    let other = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let initial = canonical.work_size() + other.work_size();
    let ends = [
        initial,
        initial + 1,
        initial + 1 + CANONICAL_WORK,
        initial + 1 + CANONICAL_WORK + ADMISSION_WORK,
        initial + 1 + CANONICAL_WORK + ADMISSION_WORK + MEMBERSHIP_WORK,
    ];
    let mut previous = 0;
    for end in ends {
        let mut budget = WorkBudget::new(end - 1);
        assert!(matches!(
            engine_meet(&canonical, &other, Domain::default(), &mut budget),
            Err(Stop::Pending(QueryReason::ExpressionLimit))
        ));
        assert_eq!(budget.used(), previous);
        assert!(!budget.exhausted());
        previous = end;
    }
    let mut budget = WorkBudget::new(previous + CLONE_WORK - 1);
    assert!(matches!(
        engine_meet(&canonical, &other, Domain::default(), &mut budget),
        Err(Stop::Pending(QueryReason::ExpressionLimit))
    ));
    assert_eq!(budget.used(), previous);
    assert!(!budget.exhausted());
}

#[test]
fn contradiction_pays_membership_without_cloning_and_shortcut_prices_stay_exact() {
    let zero = NumericValue::constant(U256::ZERO);
    let nonzero = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let expected = zero.work_size()
        + nonzero.work_size()
        + 1
        + CANONICAL_WORK
        + ADMISSION_WORK
        + MEMBERSHIP_WORK;
    let mut budget = WorkBudget::new(expected);
    assert!(matches!(
        engine_meet(&zero, &nonzero, Domain::default(), &mut budget),
        Err(Stop::Bottom)
    ));
    assert_eq!(budget.used(), expected);
    let top = NumericValue::top();
    for (left, right) in [(&zero, &zero), (&zero, &top), (&top, &zero)] {
        let old_price = left.work_size() + right.work_size();
        let mut budget = WorkBudget::new(old_price);
        assert_eq!(
            engine_meet(left, right, Domain::default(), &mut budget).unwrap(),
            zero
        );
        assert_eq!(budget.used(), old_price);
        assert!(!budget.exhausted());
    }
}

#[test]
fn refused_learning_and_cancellation_preserve_retained_numeric_facts() {
    let limits = RelationLimits::default();
    let expression = ExprId::fresh().unwrap();
    let retained = NumericValue {
        nonzero: true,
        ..NumericValue::top()
    };
    let canonical = NumericValue::constant(U256::ONE);
    // learn + two roots + one key-bucket step, then the actual structural key
    // comparison and known-value clone. Empty parent maps need no navigation.
    let known_prefix = 4 + expression.comparison_work(&expression) + retained.work_size();
    let before_clone = known_prefix
        + retained.work_size()
        + canonical.work_size()
        + 2
        + CANONICAL_WORK
        + ADMISSION_WORK
        + MEMBERSHIP_WORK;
    for cancelled in [false, true] {
        let cancellation = analysis_control::Cancellation::new();
        let mut budget = WorkBudget::new(before_clone + CLONE_WORK - 1);
        let mut work = Work {
            remaining: limits.max_nodes,
            cancellation: cancellation.clone(),
            ledger: None,
        };
        let mut engine = Engine::new(&mut work, &limits, Domain::default());
        engine
            .facts
            .insert(expression.clone(), retained.clone(), engine.work)
            .unwrap();
        engine.work.ledger = Some(&mut budget);
        if cancelled {
            cancellation.cancel();
        }
        let reason = if cancelled {
            QueryReason::Cancelled
        } else {
            QueryReason::ExpressionLimit
        };
        assert!(matches!(
            engine.learn(&expression, &canonical),
            Err(Stop::Pending(actual)) if actual == reason
        ));
        assert_eq!(
            engine.work.ledger.as_ref().unwrap().used(),
            if cancelled { 0 } else { before_clone }
        );
        engine.work.ledger = None;
        engine.work.cancellation = analysis_control::Cancellation::new();
        assert_eq!(
            engine.facts.get(&expression, engine.work).unwrap(),
            Some(&retained)
        );
    }
}

#[test]
fn observed_false_comparison_still_projects_an_exact_local_consumer_value() {
    let limits = RelationLimits::default();
    let x = ExprId::fresh().unwrap();
    let comparison = ExprId::operation(
        opcode::EQ,
        &[x, ExprId::constant(U256::from(7))],
        limits.expressions(),
    )
    .unwrap();
    let mut state = RelationState::default();
    state
        .observe(&comparison, &NumericValue::constant(U256::ZERO), &limits)
        .unwrap();
    let before = state.clone();
    let mut budget = WorkBudget::new(20_000_000);
    assert_eq!(
        state.local_value_budgeted(&comparison, &limits, &mut budget),
        ValueQuery::Exact(U256::ZERO)
    );
    assert_eq!(state, before);
    assert!(!budget.exhausted());
}

//! Actual Engine imports, with the previous reservation as the reference owner.

use super::super::{Engine, LocalResult, QueryReason, RelationLimits, Stop, Work, keys::KeyMap};
use super::reservation;
use crate::{
    domain::{
        AbstractValue, Domain, DomainSpec, FiniteConstantSet, NumericValue, Profile,
        congruence::Congruence,
        facts::{
            BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FactLattice, FiniteSet,
            Symbol, Term, UnaryFact, UnaryPredicate, WordBounds,
        },
        interval::Interval,
        known_bits::KnownBits,
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::{collections::BTreeSet, num::NonZeroUsize};

fn unary(predicate: UnaryPredicate) -> Fact {
    Fact::Unary(UnaryFact::new(Symbol::THIS, predicate))
}

// This is the previous Engine::import sequence. Both sides execute the same
// real FactLattice/reduce::import/learn mathematics; only their quote differs.
fn legacy_import(engine: &mut Engine<'_, '_>, expression: &ExprId, fact: Fact) -> LocalResult<()> {
    let known = engine.known(expression)?;
    let input = AbstractValue::from_numeric(known);
    engine
        .work
        .prepay(engine.domain.refinement_work(&input, 1))?;
    let mut lattice = FactLattice::new(engine.domain.spec().fact_limit());
    lattice.insert(fact)?;
    let value = crate::domain::reduce::import(&input, &lattice)?;
    engine.learn(expression, value.numeric())
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Bottom,
    Pending(QueryReason),
}

fn failure(stop: Stop) -> Failure {
    match stop {
        Stop::Bottom => Failure::Bottom,
        Stop::Pending(reason) => Failure::Pending(reason),
    }
}

// Observe retained entries without changing the real query's ledger/limits.
fn retained<T: Clone>(map: &KeyMap<T>, key: &ExprId) -> Option<T> {
    let mut probe = Work {
        remaining: usize::MAX,
        cancellation: analysis_control::Cancellation::default(),
        ledger: None,
    };
    map.get(key, &mut probe).unwrap().cloned()
}

#[derive(Debug, PartialEq, Eq)]
struct Entries {
    facts: Vec<Option<NumericValue>>,
    parents: Vec<Option<ExprId>>,
    memo: Vec<Option<NumericValue>>,
    terms: Vec<Option<ExprId>>,
}

fn entries(engine: &Engine<'_, '_>, expression: &ExprId, sentinel: &ExprId) -> Entries {
    let keys = [expression, sentinel];
    Entries {
        facts: keys
            .iter()
            .map(|key| retained(&engine.facts, key))
            .collect(),
        parents: keys
            .iter()
            .map(|key| retained(&engine.parents, key))
            .collect(),
        memo: keys.iter().map(|key| retained(&engine.memo, key)).collect(),
        terms: keys
            .iter()
            .map(|key| retained(&engine.terms, key))
            .collect(),
    }
}

#[derive(Debug)]
struct Outcome {
    result: Result<(), Failure>,
    before: Entries,
    after: Entries,
    used: usize,
    remaining: usize,
    exhausted: bool,
}

fn run(
    expression: &ExprId,
    initial: &NumericValue,
    fact: Fact,
    domain: Domain,
    legacy: bool,
) -> Outcome {
    let limits = RelationLimits::default();
    let mut budget = WorkBudget::new(1_000_000);
    let mut work = Work {
        remaining: 100_000,
        cancellation: analysis_control::Cancellation::default(),
        ledger: None,
    };
    let mut engine = Engine::new(&mut work, &limits, domain);
    if !initial.is_top() {
        engine
            .facts
            .insert(expression.clone(), initial.clone(), engine.work)
            .unwrap();
    }
    let sentinel = ExprId::constant(U256::MAX);
    let sentinel_value = NumericValue::constant(U256::from(19));
    engine
        .facts
        .insert(sentinel.clone(), sentinel_value.clone(), engine.work)
        .unwrap();
    engine
        .memo
        .insert(sentinel.clone(), sentinel_value, engine.work)
        .unwrap();
    engine
        .parents
        .insert(
            sentinel.clone(),
            ExprId::constant(U256::from(18)),
            engine.work,
        )
        .unwrap();
    engine
        .terms
        .insert(
            sentinel.clone(),
            ExprId::constant(U256::from(17)),
            engine.work,
        )
        .unwrap();
    let before = entries(&engine, expression, &sentinel);
    engine.work.remaining = 100_000;
    engine.work.ledger = Some(&mut budget);
    let result = if legacy {
        legacy_import(&mut engine, expression, fact)
    } else {
        engine.import(expression, fact)
    }
    .map_err(failure);
    Outcome {
        result,
        before,
        after: entries(&engine, expression, &sentinel),
        used: engine.work.ledger.as_ref().unwrap().used(),
        remaining: engine.work.remaining,
        exhausted: engine.work.ledger.as_ref().unwrap().exhausted(),
    }
}

fn compare(initial: NumericValue, fact: Fact, domain: Domain) -> Outcome {
    let expression = ExprId::fresh().unwrap();
    let input = AbstractValue::from_numeric(initial.clone());
    let new_fee = reservation(domain, &input, &fact);
    let old_fee = domain.refinement_work(&input, 1);
    let old = run(&expression, &initial, fact.clone(), domain, true);
    let new = run(&expression, &initial, fact, domain, false);
    assert_eq!(new.result, old.result);
    assert_eq!(new.before, old.before);
    assert_eq!(
        new.after, old.after,
        "compare complete retained numeric components"
    );
    assert_eq!(
        new.remaining, old.remaining,
        "local charges remain unchanged"
    );
    assert_eq!(
        new.used + old_fee,
        old.used + new_fee,
        "only the import reservation changes; learn/meet retain their fees"
    );
    assert!(!new.exhausted && !old.exhausted);
    new
}

#[test]
fn supported_unary_shapes_match_the_complete_legacy_numeric_result() {
    let predicates = [
        UnaryPredicate::KnownBits(BitConstraints::new(U256::from(8), U256::ONE).unwrap()),
        UnaryPredicate::KnownBits(BitConstraints::new(U256::ZERO, U256::ZERO).unwrap()),
        UnaryPredicate::KnownBits(BitConstraints::new(!U256::from(7), U256::from(7)).unwrap()),
        UnaryPredicate::BitSet(BitIndex::new(255).unwrap()),
        UnaryPredicate::BitClear(BitIndex::new(0).unwrap()),
        UnaryPredicate::UnsignedBounds(WordBounds::new(U256::from(7), U256::from(99)).unwrap()),
        UnaryPredicate::SignedBounds(WordBounds::new(U256::ZERO, U256::MAX >> 1).unwrap()),
        UnaryPredicate::SignedBounds(WordBounds::new(U256::ONE, U256::ONE).unwrap()),
        UnaryPredicate::NonZero,
        UnaryPredicate::IsAddress,
        UnaryPredicate::Exact(U256::from(7)),
        UnaryPredicate::Exact(U256::MAX),
        UnaryPredicate::IsZero,
    ];
    for initial in [
        NumericValue::top(),
        NumericValue::unknown_byte(),
        NumericValue::unknown_address(),
        NumericValue::constant(U256::ZERO),
        NumericValue::constant(U256::from(7)),
        NumericValue::constant(U256::MAX),
    ] {
        for predicate in &predicates {
            let fact = unary(predicate.clone());
            let input = AbstractValue::from_numeric(initial.clone());
            assert!(
                reservation(Domain::default(), &input, &fact)
                    < Domain::default().refinement_work(&input, 1)
            );
            compare(initial.clone(), fact, Domain::default());
        }
    }
    // Import does not visit an existing modulo constraint when finite stays Top.
    let mut modular = NumericValue::top();
    modular.congruence = Congruence::new(U256::from(3), U256::ONE).unwrap();
    compare(
        modular,
        unary(UnaryPredicate::BitClear(BitIndex::new(1).unwrap())),
        Domain::default(),
    );
}

#[test]
fn measured_binary_eq_and_ne_zero_match_legacy_in_either_operand_order() {
    let domain = Domain::default();
    let mut finite = NumericValue::top();
    finite.finite = FiniteConstantSet::constant(U256::from(7));
    for initial in [
        NumericValue::top(),
        finite,
        NumericValue::constant(U256::ZERO),
        NumericValue::constant(U256::from(7)),
        NumericValue::constant(U256::MAX),
    ] {
        for (predicate, value) in [
            (BinaryPredicate::Eq, U256::ZERO),
            (BinaryPredicate::Eq, U256::from(7)),
            (BinaryPredicate::Eq, U256::MAX),
            (BinaryPredicate::Ne, U256::ZERO),
        ] {
            for reverse in [false, true] {
                let (left, right) = if reverse {
                    (Term::Constant(value), Term::Symbol(Symbol::THIS))
                } else {
                    (Term::Symbol(Symbol::THIS), Term::Constant(value))
                };
                let fact = Fact::Binary(BinaryFact::new(left, predicate, right));
                let input = AbstractValue::from_numeric(initial.clone());
                assert!(reservation(domain, &input, &fact) < domain.refinement_work(&input, 1));
                compare(initial.clone(), fact, domain);
            }
        }
    }
    // The general Ne0 lattice retains its binary edge: atoms=2, so the existing
    // capacity error remains distinct from Bottom under a one-atom policy.
    let one = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    ));
    let fact = Fact::Binary(BinaryFact::new(
        Term::Symbol(Symbol::THIS),
        BinaryPredicate::Ne,
        Term::Constant(U256::ZERO),
    ));
    let outcome = compare(NumericValue::top(), fact, one);
    assert_eq!(
        outcome.result,
        Err(Failure::Pending(QueryReason::ScalarFactLimit))
    );
    assert_eq!(outcome.after, outcome.before);
}

#[test]
fn finite_filtering_keeps_the_exact_previous_reservation() {
    let domain = Domain::default();
    let mut finite = NumericValue::top();
    finite.finite =
        FiniteConstantSet::try_from_values(BTreeSet::from([U256::ZERO, U256::ONE, U256::from(2)]))
            .unwrap();
    for (initial, predicate) in [
        (finite.clone(), UnaryPredicate::NonZero),
        (
            finite,
            UnaryPredicate::KnownBits(BitConstraints::new(U256::ZERO, U256::from(8)).unwrap()),
        ),
    ] {
        let input = AbstractValue::from_numeric(initial.clone());
        let fact = unary(predicate);
        assert_eq!(
            reservation(domain, &input, &fact),
            domain.refinement_work(&input, 1)
        );
        let outcome = compare(initial, fact, domain);
        if outcome.result.is_err() {
            assert_eq!(outcome.after, outcome.before);
        }
    }
    let expression = ExprId::constant(U256::from(7));
    let old = run(
        &expression,
        &NumericValue::top(),
        unary(UnaryPredicate::Exact(U256::from(7))),
        domain,
        true,
    );
    let new = run(
        &expression,
        &NumericValue::top(),
        unary(UnaryPredicate::Exact(U256::from(7))),
        domain,
        false,
    );
    assert_eq!(new.result, old.result);
    assert_eq!(new.after, old.after);
    let input = AbstractValue::from_numeric(NumericValue::constant(U256::from(7)));
    let fact = unary(UnaryPredicate::Exact(U256::from(7)));
    assert_eq!(
        new.used + domain.refinement_work(&input, 1),
        old.used + reservation(domain, &input, &fact)
    );
}

#[test]
fn unsupported_kinds_subjects_and_exact_with_modulo_use_the_old_fee() {
    let domain = Domain::default();
    let input = AbstractValue::from_numeric(NumericValue::top());
    let facts = [
        unary(UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ONE, U256::from(3)])).unwrap(),
        )),
        unary(UnaryPredicate::Congruent(
            Congruence::new(U256::from(3), U256::ONE).unwrap(),
        )),
        unary(UnaryPredicate::IsCodeAddress),
        Fact::Unary(UnaryFact::new(Symbol::new(1), UnaryPredicate::NonZero)),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Ne,
            Term::Constant(U256::ONE),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Eq,
            Term::Symbol(Symbol::new(1)),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Ult,
            Term::Constant(U256::ONE),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Constant(U256::ONE),
            BinaryPredicate::Eq,
            Term::Symbol(Symbol::new(1)),
        )),
    ];
    for fact in facts {
        assert_eq!(
            reservation(domain, &input, &fact),
            domain.refinement_work(&input, 1)
        );
        compare(NumericValue::top(), fact, domain);
    }
    let mut modular_finite = NumericValue::top();
    modular_finite.finite = FiniteConstantSet::constant(U256::ONE);
    modular_finite.congruence = Congruence::new(U256::from(3), U256::ONE).unwrap();
    for fact in [
        unary(UnaryPredicate::NonZero),
        binary_nonzero(),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Eq,
            Term::Constant(U256::ONE),
        )),
    ] {
        let input = AbstractValue::from_numeric(modular_finite.clone());
        assert_eq!(
            reservation(domain, &input, &fact),
            domain.refinement_work(&input, 1)
        );
        compare(modular_finite.clone(), fact, domain);
    }
    for congruence in [Congruence::new(U256::from(3), U256::ONE).unwrap()] {
        let mut initial = NumericValue::top();
        initial.congruence = congruence;
        for predicate in [UnaryPredicate::Exact(U256::ONE), UnaryPredicate::IsZero] {
            let fact = unary(predicate);
            let input = AbstractValue::from_numeric(initial.clone());
            assert_eq!(
                reservation(domain, &input, &fact),
                domain.refinement_work(&input, 1)
            );
            compare(initial.clone(), fact, domain);
        }
    }
}

#[test]
fn direct_contradictions_and_fact_capacity_keep_legacy_errors_and_state() {
    let mut bits = NumericValue::top();
    bits.bits = KnownBits::exact(U256::ZERO);
    let mut zero = NumericValue::top();
    zero.interval = Interval::exact(U256::ZERO);
    for (initial, fact) in [
        (
            bits.clone(),
            unary(UnaryPredicate::BitSet(BitIndex::new(0).unwrap())),
        ),
        (bits, unary(UnaryPredicate::Exact(U256::ONE))),
        (zero, unary(UnaryPredicate::NonZero)),
    ] {
        let outcome = compare(initial, fact, Domain::default());
        assert_eq!(outcome.result, Err(Failure::Bottom));
        assert_eq!(outcome.after, outcome.before);
    }
    let domain = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    ));
    let outcome = compare(
        NumericValue::top(),
        unary(UnaryPredicate::IsAddress),
        domain,
    );
    assert_eq!(
        outcome.result,
        Err(Failure::Pending(QueryReason::ScalarFactLimit))
    );
    assert_eq!(outcome.after, outcome.before);
}

#[test]
fn cancelled_import_never_reserves_or_installs() {
    let limits = RelationLimits::default();
    let token = analysis_control::Cancellation::new();
    let mut budget = WorkBudget::new(100_000);
    let mut work = Work {
        remaining: 100,
        cancellation: token.clone(),
        ledger: Some(&mut budget),
    };
    let mut engine = Engine::new(&mut work, &limits, Domain::default());
    let expression = ExprId::fresh().unwrap();
    token.cancel();
    assert!(matches!(
        engine.import(&expression, unary(UnaryPredicate::NonZero)),
        Err(Stop::Pending(QueryReason::Cancelled))
    ));
    assert_eq!(engine.work.ledger.as_ref().unwrap().used(), 0);
    assert_eq!(engine.work.remaining, 100);
    assert!(retained(&engine.facts, &expression).is_none());
    // Keep both reservation checkpoints, including without a root ledger.
    assert!(matches!(
        engine.work.prepay(1),
        Err(Stop::Pending(QueryReason::Cancelled))
    ));
    engine.work.ledger = None;
    assert!(matches!(
        engine.work.prepay(1),
        Err(Stop::Pending(QueryReason::Cancelled))
    ));
}

fn binary_nonzero() -> Fact {
    Fact::Binary(BinaryFact::new(
        Term::Symbol(Symbol::THIS),
        BinaryPredicate::Ne,
        Term::Constant(U256::ZERO),
    ))
}

fn bounded_import(maximum: usize, local: usize) -> Outcome {
    let limits = RelationLimits::default();
    let mut budget = WorkBudget::new(maximum);
    let mut work = Work {
        remaining: local,
        cancellation: analysis_control::Cancellation::default(),
        ledger: Some(&mut budget),
    };
    let mut engine = Engine::new(&mut work, &limits, Domain::default());
    let expression = ExprId::fresh().unwrap();
    let sentinel = ExprId::constant(U256::MAX);
    let before = entries(&engine, &expression, &sentinel);
    let result = engine
        .import(&expression, binary_nonzero())
        .map_err(failure);
    Outcome {
        result,
        before,
        after: entries(&engine, &expression, &sentinel),
        used: engine.work.ledger.as_ref().unwrap().used(),
        remaining: engine.work.remaining,
        exhausted: engine.work.ledger.as_ref().unwrap().exhausted(),
    }
}

#[test]
fn root_reservation_refusal_and_exact_total_boundary_are_atomic() {
    let input = AbstractValue::from_numeric(NumericValue::top());
    let fee = reservation(Domain::default(), &input, &binary_nonzero());
    // known's root step pays one; the import reservation cannot fit afterward.
    let refused = bounded_import(fee, 100);
    assert_eq!(
        refused.result,
        Err(Failure::Pending(QueryReason::ExpressionLimit))
    );
    assert_eq!(refused.used, 1);
    assert_eq!(refused.remaining, 99);
    assert!(!refused.exhausted);
    assert_eq!(refused.after, refused.before);
    let baseline = bounded_import(100_000, 100);
    assert_eq!(baseline.result, Ok(()));
    let exact = bounded_import(baseline.used, 100);
    assert_eq!(exact.result, Ok(()));
    assert_eq!(exact.used, baseline.used);
    assert_eq!(exact.after.facts[0], baseline.after.facts[0]);
    let refused = bounded_import(baseline.used - 1, 100);
    assert_eq!(
        refused.result,
        Err(Failure::Pending(QueryReason::ExpressionLimit))
    );
    assert_eq!(refused.after, refused.before);
    assert!(refused.used >= fee && refused.used < baseline.used);
    assert!(!refused.exhausted);
}

#[test]
fn local_refusal_keeps_paid_import_work_without_installing_a_result() {
    let input = AbstractValue::from_numeric(NumericValue::top());
    let fee = reservation(Domain::default(), &input, &binary_nonzero());
    for local in [0, 1, 4] {
        let refused = bounded_import(100_000, local);
        assert_eq!(
            refused.result,
            Err(Failure::Pending(QueryReason::ExpressionLimit))
        );
        assert_eq!(refused.after, refused.before);
        assert_eq!(refused.remaining, 0);
        if local == 0 {
            assert_eq!(refused.used, 0);
        } else {
            assert!(refused.used >= fee);
        }
        assert!(!refused.exhausted);
    }
}

#[test]
fn saturated_unsupported_quote_is_exactly_the_legacy_reservation() {
    let domain = Domain::from_spec(DomainSpec::new(
        Profile::Product,
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ));
    let input = AbstractValue::from_numeric(NumericValue::top());
    let fact = unary(UnaryPredicate::IsCodeAddress);
    assert_eq!(domain.refinement_work(&input, 1), usize::MAX);
    assert_eq!(reservation(domain, &input, &fact), usize::MAX);
}

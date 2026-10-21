//! Native Engine imports preserve concrete fact semantics and atomic admission.

use super::super::{Engine, QueryReason, RelationLimits, Stop, Work, keys::KeyMap};
use super::reservation;
use crate::{
    domain::{
        DomainSpec, EvmValue, FiniteConstantSet, NumericValue, ProductAlgebra, ValueAlgebra,
        congruence::Congruence,
        facts::{
            BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FiniteSet, Symbol, Term,
            UnaryFact, UnaryPredicate, WordBounds,
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

fn entries(
    engine: &Engine<'_, '_, '_, ProductAlgebra>,
    expression: &ExprId,
    sentinel: &ExprId,
) -> Entries {
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

fn run(expression: &ExprId, initial: &NumericValue, fact: Fact, domain: ProductAlgebra) -> Outcome {
    let limits = RelationLimits::default();
    let mut budget = WorkBudget::new(1_000_000);
    let mut work = Work {
        remaining: 100_000,
        cancellation: analysis_control::Cancellation::default(),
        ledger: None,
    };
    let algebra = crate::domain::ValueAlgebra::new(domain);
    let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
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
    let result = engine.import(expression, fact).map_err(failure);
    Outcome {
        result,
        before,
        after: entries(&engine, expression, &sentinel),
        used: engine.work.ledger.as_ref().unwrap().used(),
        remaining: engine.work.remaining,
        exhausted: engine.work.ledger.as_ref().unwrap().exhausted(),
    }
}

// Direct word predicates are independent of Engine import, reduction and its
// reservation implementation. Unknown external symbols impose no scalar fact.
fn admits(fact: &Fact, word: U256) -> bool {
    match fact {
        Fact::Unary(fact) if fact.subject == Symbol::THIS => match &fact.predicate {
            UnaryPredicate::Exact(exact) => word == *exact,
            UnaryPredicate::MemberOf(values) => values.values().contains(&word),
            UnaryPredicate::KnownBits(bits) => {
                word & bits.zero() == U256::ZERO && word & bits.one() == bits.one()
            }
            UnaryPredicate::BitSet(bit) => word.bit(usize::from(bit.get())),
            UnaryPredicate::BitClear(bit) => !word.bit(usize::from(bit.get())),
            UnaryPredicate::UnsignedBounds(bounds) => {
                bounds.lower() <= word && word <= bounds.upper()
            }
            UnaryPredicate::SignedBounds(bounds) => {
                let biased = word ^ (U256::ONE << 255usize);
                bounds.lower() <= biased && biased <= bounds.upper()
            }
            UnaryPredicate::Congruent(congruence) => {
                if let Some(exact) = congruence.singleton() {
                    word == exact
                } else {
                    congruence
                        .modulus_residue()
                        .is_none_or(|(modulus, residue)| word % modulus == residue)
                }
            }
            UnaryPredicate::IsZero => word == U256::ZERO,
            UnaryPredicate::NonZero => word != U256::ZERO,
            UnaryPredicate::IsAddress => word <= U256::MAX >> 96usize,
            UnaryPredicate::IsCodeAddress | UnaryPredicate::PossibleOrigins(_) => true,
        },
        Fact::Unary(_) => true,
        Fact::Binary(fact) => {
            let term = |term| match term {
                Term::Symbol(symbol) if symbol == Symbol::THIS => Some(word),
                Term::Constant(value) => Some(value),
                Term::Symbol(_) => None,
            };
            let Some((left, right)) = term(fact.left).zip(term(fact.right)) else {
                return true;
            };
            match fact.predicate {
                BinaryPredicate::Eq => left == right,
                BinaryPredicate::Ne => left != right,
                BinaryPredicate::Ult => left < right,
                BinaryPredicate::Ule => left <= right,
                BinaryPredicate::Slt => {
                    (left ^ (U256::ONE << 255usize)) < (right ^ (U256::ONE << 255usize))
                }
                BinaryPredicate::Sle => {
                    (left ^ (U256::ONE << 255usize)) <= (right ^ (U256::ONE << 255usize))
                }
            }
        }
        _ => panic!("this import oracle requires an explicit scalar predicate"),
    }
}

fn check_import(initial: NumericValue, fact: Fact, domain: ProductAlgebra) -> Outcome {
    let expression = ExprId::fresh().unwrap();
    let result = run(&expression, &initial, fact.clone(), domain);
    let samples = (0u64..=256).map(U256::from).chain([
        U256::ONE << 255usize,
        (U256::ONE << 255usize) - U256::ONE,
        U256::MAX,
    ]);
    match &result.result {
        Ok(()) => {
            let numeric = result.after.facts[0].as_ref().unwrap_or(&initial);
            // An open product cannot encode exclusion of an arbitrary nonzero
            // literal. This unsupported relation must keep the original
            // carrier; finite covers can still filter that literal exactly.
            let open_exclusion = initial.constants().is_none()
                && matches!(&fact, Fact::Binary(binary)
                    if binary.predicate == BinaryPredicate::Ne
                        && matches!((binary.left, binary.right),
                            (Term::Symbol(Symbol::THIS), Term::Constant(value))
                            | (Term::Constant(value), Term::Symbol(Symbol::THIS))
                            if value != U256::ZERO));
            if open_exclusion {
                assert_eq!(numeric, &initial);
            }
            for word in samples {
                let admitted = initial.contains(word) && admits(&fact, word);
                if open_exclusion {
                    assert!(!admitted || numeric.contains(word));
                } else {
                    assert_eq!(
                        numeric.contains(word),
                        admitted,
                        "fact={fact:?}, initial={initial:?}, word={word}"
                    );
                }
            }
            // Import updates only the intended fact; unrelated keyed entries
            // and the parent/memo/term maps remain unchanged.
            assert_eq!(result.after.facts[1], result.before.facts[1]);
            assert_eq!(result.after.parents, result.before.parents);
            assert_eq!(result.after.memo, result.before.memo);
            assert_eq!(result.after.terms, result.before.terms);
            assert!(result.used > 0);
        }
        Err(Failure::Bottom) => {
            assert!(
                samples
                    .into_iter()
                    .all(|word| !initial.contains(word) || !admits(&fact, word))
            );
            assert_eq!(result.after, result.before);
        }
        Err(Failure::Pending(reason)) => {
            assert_eq!(*reason, QueryReason::ScalarFactLimit);
            assert_eq!(result.after, result.before);
        }
    }
    assert!(!result.exhausted);
    result
}

#[test]
fn supported_unary_imports_match_independent_concrete_predicates() {
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
            let input = EvmValue::<NumericValue>::from_numeric(initial.clone());
            assert!(
                reservation(
                    ValueAlgebra::new(ProductAlgebra::default()).borrowed(),
                    &input,
                    &fact
                ) < ValueAlgebra::new(ProductAlgebra::default()).refinement_work(&input, 1)
            );
            check_import(initial.clone(), fact, ProductAlgebra::default());
        }
    }
    // Import does not visit an existing modulo constraint when finite stays Top.
    let mut modular = NumericValue::top();
    modular.congruence = Congruence::new(U256::from(3), U256::ONE).unwrap();
    check_import(
        modular,
        unary(UnaryPredicate::BitClear(BitIndex::new(1).unwrap())),
        ProductAlgebra::default(),
    );
}

#[test]
fn binary_eq_and_ne_zero_imports_match_concrete_predicates_in_both_orders() {
    let domain = ProductAlgebra::default();
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
                let input = EvmValue::<NumericValue>::from_numeric(initial.clone());
                assert!(
                    reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact)
                        < ValueAlgebra::new(domain).refinement_work(&input, 1)
                );
                check_import(initial.clone(), fact, domain);
            }
        }
    }
    // The general Ne0 lattice retains its binary edge: atoms=2, so the existing
    // capacity error remains distinct from Bottom under a one-atom policy.
    let one = ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    ));
    let fact = Fact::Binary(BinaryFact::new(
        Term::Symbol(Symbol::THIS),
        BinaryPredicate::Ne,
        Term::Constant(U256::ZERO),
    ));
    let outcome = check_import(NumericValue::top(), fact, one);
    assert_eq!(
        outcome.result,
        Err(Failure::Pending(QueryReason::ScalarFactLimit))
    );
    assert_eq!(outcome.after, outcome.before);
}

#[test]
fn finite_filtering_pays_the_full_input_reservation() {
    let domain = ProductAlgebra::default();
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
        let input = EvmValue::<NumericValue>::from_numeric(initial.clone());
        let fact = unary(predicate);
        assert_eq!(
            reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact),
            ValueAlgebra::new(domain).refinement_work(&input, 1)
        );
        let outcome = check_import(initial, fact, domain);
        if outcome.result.is_err() {
            assert_eq!(outcome.after, outcome.before);
        }
    }
    let expression = ExprId::constant(U256::from(7));
    let result = run(
        &expression,
        &NumericValue::top(),
        unary(UnaryPredicate::Exact(U256::from(7))),
        domain,
    );
    assert_eq!(result.result, Ok(()));
    // A literal is already authoritative; importing the same literal must not
    // add a redundant keyed fact or invalidate unrelated retained entries.
    assert_eq!(result.after, result.before);
    assert!(!result.exhausted);
}

#[test]
fn unsupported_shortcut_shapes_reserve_the_complete_refinement_work() {
    let domain = ProductAlgebra::default();
    let input = EvmValue::<NumericValue>::from_numeric(NumericValue::top());
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
            reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact),
            ValueAlgebra::new(domain).refinement_work(&input, 1)
        );
        check_import(NumericValue::top(), fact, domain);
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
        let input = EvmValue::<NumericValue>::from_numeric(modular_finite.clone());
        assert_eq!(
            reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact),
            ValueAlgebra::new(domain).refinement_work(&input, 1)
        );
        check_import(modular_finite.clone(), fact, domain);
    }
    for congruence in [Congruence::new(U256::from(3), U256::ONE).unwrap()] {
        let mut initial = NumericValue::top();
        initial.congruence = congruence;
        for predicate in [UnaryPredicate::Exact(U256::ONE), UnaryPredicate::IsZero] {
            let fact = unary(predicate);
            let input = EvmValue::<NumericValue>::from_numeric(initial.clone());
            assert_eq!(
                reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact),
                ValueAlgebra::new(domain).refinement_work(&input, 1)
            );
            check_import(initial.clone(), fact, domain);
        }
    }
}

#[test]
fn direct_contradictions_and_fact_capacity_keep_typed_errors_and_atomic_state() {
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
        let outcome = check_import(initial, fact, ProductAlgebra::default());
        assert_eq!(outcome.result, Err(Failure::Bottom));
        assert_eq!(outcome.after, outcome.before);
    }
    let domain = ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    ));
    let outcome = check_import(
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
    let algebra = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
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
    let algebra = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
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
    let input = EvmValue::<NumericValue>::from_numeric(NumericValue::top());
    let fee = reservation(
        ValueAlgebra::new(ProductAlgebra::default()).borrowed(),
        &input,
        &binary_nonzero(),
    );
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
    let input = EvmValue::<NumericValue>::from_numeric(NumericValue::top());
    let fee = reservation(
        ValueAlgebra::new(ProductAlgebra::default()).borrowed(),
        &input,
        &binary_nonzero(),
    );
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
fn saturated_unsupported_quote_refuses_the_whole_refinement_reservation() {
    let domain = ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(usize::MAX).unwrap(),
        NonZeroUsize::new(256).unwrap(),
    ));
    let input = EvmValue::<NumericValue>::from_numeric(NumericValue::top());
    let fact = unary(UnaryPredicate::IsCodeAddress);
    assert_eq!(
        ValueAlgebra::new(domain).refinement_work(&input, 1),
        usize::MAX
    );
    assert_eq!(
        reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact),
        usize::MAX
    );
}

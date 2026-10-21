//! Native Engine imports preserve concrete fact semantics and atomic admission.

use super::super::{Engine, QueryReason, RelationLimits, Stop, Work, keys::KeyMap};
use super::reservation;
use crate::{
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, FiniteSetAlgebra,
        NumericReduction, NumericValue, ProductAlgebra, ValueAlgebra, WordElement, WordObservation,
        congruence::Congruence,
        facts::{
            BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FactError, FiniteSet,
            OperationRelation, RelationalFact, Symbol, Term, UnaryFact, UnaryPredicate, WordBounds,
        },
        interval::Interval,
        known_bits::KnownBits,
        symbolic::ExprId,
    },
    resource::WorkBudget,
};
use alloy_primitives::U256;
use std::{cell::Cell, collections::BTreeSet, num::NonZeroUsize, rc::Rc};

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
struct Entries<N: WordElement> {
    facts: Vec<Option<N>>,
    parents: Vec<Option<ExprId>>,
    memo: Vec<Option<N>>,
    terms: Vec<Option<ExprId>>,
}

fn entries<D: EvmNumericAlgebra>(
    engine: &Engine<'_, '_, '_, D>,
    expression: &ExprId,
    sentinel: &ExprId,
) -> Entries<D::Element> {
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
struct Outcome<N: WordElement> {
    result: Result<(), Failure>,
    before: Entries<N>,
    after: Entries<N>,
    used: usize,
    remaining: usize,
    exhausted: bool,
}

fn run<D: EvmNumericAlgebra>(
    expression: &ExprId,
    initial: &D::Element,
    fact: Fact,
    domain: D,
) -> Outcome<D::Element> {
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
    let sentinel_value = D::Element::constant(U256::from(19));
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

fn check_import(
    initial: NumericValue,
    fact: Fact,
    domain: ProductAlgebra,
) -> Outcome<NumericValue> {
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
fn finite_filtering_pays_actual_import_work_and_preserves_validation() {
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
        // The shortcut rejects multi-member input. This is now a detached
        // actual-payload import, not a second complete local closure payment.
        let quote = reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact);
        assert!(quote >= input.numeric().work_size());
        assert!(quote < ValueAlgebra::new(domain).refinement_work(&input, 1));
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
fn ineligible_builtin_shapes_use_detached_import_or_honest_fallback() {
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
        let quote = reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact);
        if matches!(&fact, Fact::Binary(binary)
            if matches!((binary.left, binary.right), (Term::Symbol(_), Term::Symbol(_))))
        {
            assert_eq!(quote, ValueAlgebra::new(domain).refinement_work(&input, 1));
        } else {
            // Only the former exact fee equality changes: the import no longer
            // reserves downstream reduction already paid by learn/meet.
            assert!(quote < ValueAlgebra::new(domain).refinement_work(&input, 1));
        }
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
        let quote = reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact);
        if matches!(&fact, Fact::Binary(binary)
            if matches!((binary.left, binary.right), (Term::Symbol(_), Term::Symbol(_))))
        {
            assert_eq!(quote, ValueAlgebra::new(domain).refinement_work(&input, 1));
        } else {
            // Only the former exact fee equality changes: the import no longer
            // reserves downstream reduction already paid by learn/meet.
            assert!(quote < ValueAlgebra::new(domain).refinement_work(&input, 1));
        }
        check_import(modular_finite.clone(), fact, domain);
    }
    for congruence in [Congruence::new(U256::from(3), U256::ONE).unwrap()] {
        let mut initial = NumericValue::top();
        initial.congruence = congruence;
        for predicate in [UnaryPredicate::Exact(U256::ONE), UnaryPredicate::IsZero] {
            let fact = unary(predicate);
            let input = EvmValue::<NumericValue>::from_numeric(initial.clone());
            assert!(
                reservation(ValueAlgebra::new(domain).borrowed(), &input, &fact)
                    < ValueAlgebra::new(domain).refinement_work(&input, 1)
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

fn bounded_import(maximum: usize, local: usize) -> Outcome<NumericValue> {
    bounded_native_import(
        ProductAlgebra::default(),
        binary_nonzero(),
        maximum,
        local,
        analysis_control::Cancellation::default(),
    )
}

fn bounded_native_import<D: EvmNumericAlgebra>(
    domain: D,
    fact: Fact,
    maximum: usize,
    local: usize,
    cancellation: analysis_control::Cancellation,
) -> Outcome<D::Element> {
    let limits = RelationLimits::default();
    let mut budget = WorkBudget::new(maximum);
    let mut work = Work {
        remaining: local,
        cancellation,
        ledger: Some(&mut budget),
    };
    let algebra = ValueAlgebra::new(domain);
    let mut engine = Engine::new(&mut work, &limits, algebra.borrowed());
    let expression = ExprId::fresh().unwrap();
    let sentinel = ExprId::constant(U256::MAX);
    let before = entries(&engine, &expression, &sentinel);
    let result = engine.import(&expression, fact).map_err(failure);
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

#[test]
fn native_finite_import_uses_actual_small_fact_and_keeps_complete_candidates() {
    let domain = FiniteSetAlgebra::default();
    let algebra = ValueAlgebra::new(domain);
    let input = EvmValue::from_numeric(FiniteConstantSet::top());
    let fact = unary(UnaryPredicate::NonZero);
    let quote = reservation(algebra.borrowed(), &input, &fact);
    let whole = algebra.refinement_work(&input, 1);
    assert!(!domain.supports_product_scalar_shortcuts());
    assert_eq!(
        quote,
        domain
            .refinement_import_work(input.numeric(), 1, 0, super::COMMON)
            .unwrap()
            + 1,
    );
    assert!(quote < whole);
    let expression = ExprId::fresh().unwrap();
    let initial =
        FiniteConstantSet::try_from_values(BTreeSet::from([U256::ZERO, U256::ONE, U256::from(2)]))
            .unwrap();
    let outcome = run(&expression, &initial, fact, domain);
    assert_eq!(outcome.result, Ok(()));
    let result = outcome.after.facts[0].as_ref().unwrap();
    assert_eq!(
        result.as_values(),
        Some(&BTreeSet::from([U256::ONE, U256::from(2)]))
    );
    assert!(!outcome.exhausted);
}

#[derive(Debug, Default)]
struct Access {
    copies: Cell<usize>,
    visits: Cell<usize>,
}

// The oddness guarantee is deliberately absent from the exported observation.
// Quoting must retain this actual carrier, not rebuild it from standard views.
#[derive(Debug, serde::Serialize)]
struct Tracked {
    members: FiniteConstantSet,
    odd: bool,
    #[serde(skip)]
    access: Rc<Access>,
}
impl Clone for Tracked {
    fn clone(&self) -> Self {
        self.access.copies.set(self.access.copies.get() + 1);
        Self {
            members: self.members.clone(),
            odd: self.odd,
            access: Rc::clone(&self.access),
        }
    }
}
impl PartialEq for Tracked {
    fn eq(&self, other: &Self) -> bool {
        self.members == other.members && self.odd == other.odd
    }
}
impl Eq for Tracked {}
impl std::fmt::Display for Tracked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/odd={}", self.members, self.odd)
    }
}
impl WordElement for Tracked {
    fn top() -> Self {
        Self {
            members: FiniteConstantSet::top(),
            odd: false,
            access: Rc::default(),
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            members: FiniteConstantSet::constant(value),
            ..Self::top()
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self {
            members: FiniteConstantSet::from_observation(observation),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        self.access.visits.set(self.access.visits.get() + 1);
        self.members.observation()
    }
    fn constrain(&self, observation: &WordObservation) -> Result<Self, FactError> {
        self.access.visits.set(self.access.visits.get() + 1);
        let members = self
            .members
            .constrain(observation)?
            .filter(|word| !self.odd || word.bit(0))
            .map_err(|_| FactError::Contradiction {
                subject: Symbol::THIS,
            })?;
        Ok(Self {
            members,
            ..self.clone()
        })
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        &self.members
    }
    fn contains(&self, word: U256) -> bool {
        self.access.visits.set(self.access.visits.get() + 1);
        self.members.contains(word) && (!self.odd || word.bit(0))
    }
    fn singleton(&self) -> Option<U256> {
        self.access.visits.set(self.access.visits.get() + 1);
        self.members
            .singleton()
            .filter(|word| !self.odd || word.bit(0))
    }
    fn is_top(&self) -> bool {
        self.members.is_top() && !self.odd
    }
    fn work_size(&self) -> usize {
        WordElement::work_size(&self.members).saturating_add(1)
    }
    fn clone_work(&self) -> usize {
        self.members.clone_work() + 2
    }
}

#[derive(Debug)]
struct ImportPolicy {
    finite: FiniteSetAlgebra,
    staged: bool,
    refuse: bool,
    last: Cell<Option<(usize, usize, usize)>>,
    saw_odd: Cell<bool>,
    reductions: Cell<usize>,
}
impl ImportPolicy {
    fn new(staged: bool) -> Self {
        Self {
            finite: FiniteSetAlgebra::default(),
            staged,
            refuse: false,
            last: Cell::new(None),
            saw_odd: Cell::new(false),
            reductions: Cell::new(0),
        }
    }
}
impl EvmNumericAlgebra for ImportPolicy {
    type Element = Tracked;
    fn spec(&self) -> DomainSpec {
        self.finite.spec()
    }
    fn project_numeric(&self, value: &Tracked) -> Tracked {
        value.clone()
    }
    fn join_numeric(&self, left: &Tracked, right: &Tracked) -> Tracked {
        Tracked {
            members: self.finite.join_numeric(&left.members, &right.members),
            odd: left.odd && right.odd,
            ..left.clone()
        }
    }
    fn widen_numeric(&self, left: &Tracked, right: &Tracked) -> Tracked {
        self.join_numeric(left, right)
    }
    fn widen_procedure_numeric(&self, left: &Tracked, right: &Tracked) -> Tracked {
        self.join_numeric(left, right)
    }
    fn reduce_numeric(&self, value: &Tracked) -> NumericReduction<Tracked> {
        self.reductions.set(self.reductions.get() + 1);
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _: u8,
        _: impl ExactSizeIterator<Item = &'a Tracked>,
    ) -> NumericReduction<Tracked> {
        panic!("local fact import must not run an instruction transfer")
    }
    fn operation_work<'a>(&self, _: u8, _: impl Iterator<Item = &'a Tracked>) -> usize {
        usize::MAX
    }
    fn projection_work(&self, value: &Tracked) -> usize {
        value.clone_work()
    }
    fn refinement_work(&self, value: &Tracked, facts: usize) -> usize {
        self.finite
            .refinement_work(&value.members, facts)
            .saturating_add(value.clone_work() * 4)
    }
    fn refinement_import_work(
        &self,
        value: &Tracked,
        facts: usize,
        finite: usize,
        structural: usize,
    ) -> Option<usize> {
        self.last.set(Some((facts, finite, structural)));
        self.saw_odd.set(value.odd);
        if !self.staged {
            return None;
        }
        if self.refuse {
            return Some(usize::MAX);
        }
        self.finite
            .refinement_import_work(&value.members, facts, finite, structural)
            .map(|work| work.saturating_add(value.clone_work() * 4))
    }
}

#[test]
fn member_payload_is_counted_before_native_copy_or_candidate_visits() {
    let policy = ImportPolicy::new(true);
    let access = Rc::<Access>::default();
    let input = EvmValue::from_numeric(Tracked {
        members: FiniteConstantSet::try_from_values(BTreeSet::from([U256::ONE, U256::from(3)]))
            .unwrap(),
        odd: true,
        access: Rc::clone(&access),
    })
    .with_expression(ExprId::fresh().unwrap());
    let mut previous = 0;
    for count in [1usize, 64, 257] {
        let fact = unary(UnaryPredicate::MemberOf(
            FiniteSet::new((0..count).map(U256::from).collect()).unwrap(),
        ));
        let quote = reservation(ValueAlgebra::new(&policy).borrowed(), &input, &fact);
        assert_eq!(policy.last.get(), Some((1, 2 + count, super::COMMON)));
        assert!(policy.saw_odd.get());
        assert!(quote > previous);
        previous = quote;
        assert_eq!((access.copies.get(), access.visits.get()), (0, 0));
        assert_eq!(policy.reductions.get(), 0);
    }
}

#[test]
fn synthetic_literal_bounds_do_not_evaluate_borrowed_facts() {
    let policy = ImportPolicy::new(true);
    let input = EvmValue::from_numeric(Tracked {
        odd: true,
        ..Tracked::top()
    });
    let access = Rc::clone(&input.numeric().access);
    for (predicate, members) in [
        (UnaryPredicate::Exact(U256::ONE), 1),
        (UnaryPredicate::IsZero, 1),
        (
            UnaryPredicate::KnownBits(BitConstraints::new(U256::MAX, U256::ZERO).unwrap()),
            1,
        ),
        (
            UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::ZERO).unwrap()),
            1,
        ),
        (
            UnaryPredicate::SignedBounds(WordBounds::new(U256::ONE, U256::ONE).unwrap()),
            1,
        ),
        (UnaryPredicate::Congruent(Congruence::exact(U256::ONE)), 1),
        (UnaryPredicate::BitSet(BitIndex::new(0).unwrap()), 0),
        (UnaryPredicate::NonZero, 0),
        (UnaryPredicate::IsAddress, 0),
        (UnaryPredicate::IsCodeAddress, 0),
    ] {
        reservation(
            ValueAlgebra::new(&policy).borrowed(),
            &input,
            &unary(predicate),
        );
        assert_eq!(policy.last.get(), Some((1, members, super::COMMON)));
        assert_eq!((access.copies.get(), access.visits.get()), (0, 0));
    }
    for predicate in [
        BinaryPredicate::Eq,
        BinaryPredicate::Ne,
        BinaryPredicate::Ult,
        BinaryPredicate::Ule,
        BinaryPredicate::Slt,
        BinaryPredicate::Sle,
    ] {
        for reverse in [false, true] {
            let (left, right) = if reverse {
                (Term::Constant(U256::ZERO), Term::Symbol(Symbol::THIS))
            } else {
                (Term::Symbol(Symbol::THIS), Term::Constant(U256::ZERO))
            };
            reservation(
                ValueAlgebra::new(&policy).borrowed(),
                &input,
                &Fact::Binary(BinaryFact::new(left, predicate, right)),
            );
            let (facts, members, structural) = policy.last.get().unwrap();
            assert_eq!(facts, 1);
            assert_eq!(members, usize::from(predicate != BinaryPredicate::Ne));
            assert!(structural > super::COMMON);
            assert_eq!((access.copies.get(), access.visits.get()), (0, 0));
        }
    }
}

#[test]
fn none_and_unproved_shapes_keep_exact_whole_quote_without_native_work() {
    let input = EvmValue::from_numeric(Tracked {
        odd: true,
        ..Tracked::top()
    });
    let access = Rc::clone(&input.numeric().access);
    let policy = ImportPolicy::new(false);
    let algebra = ValueAlgebra::new(&policy);
    assert_eq!(
        reservation(algebra.borrowed(), &input, &unary(UnaryPredicate::NonZero)),
        algebra.refinement_work(&input, 1)
    );
    assert!(policy.last.get().is_some());
    assert_eq!((access.copies.get(), access.visits.get()), (0, 0));
    let policy = ImportPolicy::new(true);
    let algebra = ValueAlgebra::new(&policy);
    for fact in [
        unary(UnaryPredicate::PossibleOrigins(
            crate::domain::provenance::OriginSet::source(
                crate::domain::provenance::Origin::Storage,
            ),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Eq,
            Term::Symbol(Symbol::new(1)),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Constant(U256::ONE),
            BinaryPredicate::Eq,
            Term::Constant(U256::ONE),
        )),
        Fact::Relation(RelationalFact::Operation(
            OperationRelation::new(
                revm_bytecode::opcode::ADD,
                Symbol::THIS,
                vec![Term::Constant(U256::ONE), Term::Constant(U256::ONE)],
            )
            .unwrap(),
        )),
    ] {
        assert_eq!(
            reservation(algebra.borrowed(), &input, &fact),
            algebra.refinement_work(&input, 1)
        );
        assert_eq!(policy.last.get(), None);
        assert_eq!((access.copies.get(), access.visits.get()), (0, 0));
    }
}

#[test]
fn detached_import_preserves_hidden_constraint_and_paid_downstream_meet() {
    let policy = ImportPolicy::new(true);
    let expression = ExprId::fresh().unwrap();
    let initial = Tracked {
        odd: true,
        ..Tracked::top()
    };
    let fact = unary(UnaryPredicate::MemberOf(
        FiniteSet::new((0u64..=3).map(U256::from).collect()).unwrap(),
    ));
    let outcome = run(&expression, &initial, fact, &policy);
    assert_eq!(outcome.result, Ok(()));
    let retained = outcome.after.facts[0].as_ref().unwrap();
    assert!(retained.odd);
    assert_eq!(
        retained.members.as_values(),
        Some(&BTreeSet::from([U256::ONE, U256::from(3)]))
    );
    // Import itself does not reduce. This reduction belongs to the separately
    // paid learn/meet intersection and must not disappear with the new quote.
    assert!(policy.reductions.get() > 0);
    let contradiction = run(
        &expression,
        &initial,
        unary(UnaryPredicate::Exact(U256::from(2))),
        &policy,
    );
    assert_eq!(contradiction.result, Err(Failure::Bottom));
    assert_eq!(contradiction.after, contradiction.before);
    assert!(!contradiction.exhausted);
}

#[test]
fn native_import_cost_and_cancel_refusal_remain_atomic() {
    let policy = FiniteSetAlgebra::default();
    let fact = unary(UnaryPredicate::Exact(U256::ONE));
    let baseline = bounded_native_import(
        policy,
        fact.clone(),
        1_000_000,
        100,
        analysis_control::Cancellation::default(),
    );
    assert_eq!(baseline.result, Ok(()));
    let quote = reservation(
        ValueAlgebra::new(policy).borrowed(),
        &EvmValue::from_numeric(FiniteConstantSet::top()),
        &fact,
    );
    assert!(baseline.used > quote);
    for maximum in [quote, baseline.used - 1] {
        let refused = bounded_native_import(
            policy,
            fact.clone(),
            maximum,
            100,
            analysis_control::Cancellation::default(),
        );
        assert_eq!(
            refused.result,
            Err(Failure::Pending(QueryReason::ExpressionLimit))
        );
        assert_eq!(refused.after, refused.before);
        assert!(!refused.exhausted);
    }
    let exact = bounded_native_import(
        policy,
        fact.clone(),
        baseline.used,
        100,
        analysis_control::Cancellation::default(),
    );
    assert_eq!(exact.result, Ok(()));
    assert_eq!(exact.used, baseline.used);
    assert_eq!(exact.after.facts, baseline.after.facts);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    let cancelled = bounded_native_import(policy, fact.clone(), 1_000_000, 100, cancellation);
    assert_eq!(
        cancelled.result,
        Err(Failure::Pending(QueryReason::Cancelled))
    );
    assert_eq!(cancelled.after, cancelled.before);
    assert_eq!((cancelled.used, cancelled.remaining), (0, 100));
    let mut custom = ImportPolicy::new(true);
    custom.refuse = true;
    let refused = bounded_native_import(
        &custom,
        fact,
        1_000_000,
        100,
        analysis_control::Cancellation::default(),
    );
    assert_eq!(
        refused.result,
        Err(Failure::Pending(QueryReason::ExpressionLimit))
    );
    assert_eq!(refused.after, refused.before);
    assert_eq!(refused.used, 1);
    assert_eq!(custom.reductions.get(), 0);
}

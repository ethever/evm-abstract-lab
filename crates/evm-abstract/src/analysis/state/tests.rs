//! Literal proof propagation must not discover an opaque numeric singleton.

use super::identify;
use crate::{
    U256,
    analysis::state::query::tests::carrier::{Probe, Word},
    domain::{
        Domain, EvmValue, FiniteConstantSet, NumericValue, WordElement, WordObservation,
        facts::UnaryPredicate,
        identity::Symbol,
        interval::Interval,
        known_bits::KnownBits,
        provenance::Origin,
        symbolic::{ExprId, ExprKind},
    },
};
use serde::Serialize;
use std::{collections::BTreeSet, fmt, rc::Rc};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct CachedWord {
    opaque: Word,
    candidates: Option<BTreeSet<U256>>,
    finite: FiniteConstantSet,
    bits: KnownBits,
    interval: Interval,
}

impl fmt::Display for CachedWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.opaque.fmt(f)
    }
}

impl WordElement for CachedWord {
    fn top() -> Self {
        Self {
            opaque: Word::top(),
            candidates: None,
            finite: FiniteConstantSet::top(),
            bits: KnownBits::top(),
            interval: Interval::top(),
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            opaque: Word::constant(word),
            ..Self::top()
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self {
            opaque: Word::from_observation(facts),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        self.opaque.observation()
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.candidates.as_ref().or_else(|| self.finite.as_values())
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        &self.finite
    }
    fn known_bits(&self) -> &KnownBits {
        &self.bits
    }
    fn interval(&self) -> &Interval {
        &self.interval
    }
    fn singleton(&self) -> Option<U256> {
        self.opaque.singleton()
    }
    fn singleton_work(&self) -> usize {
        self.opaque.singleton_work()
    }
    fn is_top(&self) -> bool {
        self.opaque.is_top()
    }
    fn work_size(&self) -> usize {
        32
    }
}

#[test]
fn canonical_literal_keeps_each_independent_cached_proof_without_opaque_queries() {
    for view in 0..4 {
        let probe = Rc::new(Probe::new(usize::MAX));
        let mut numeric = CachedWord::constant(U256::from(7));
        numeric.opaque = numeric.opaque.watched(&probe);
        match view {
            0 => numeric.finite = FiniteConstantSet::constant(U256::from(7)),
            1 => {
                numeric.candidates = Some(BTreeSet::from([U256::from(7)]));
                assert!(numeric.finite.is_top());
                assert_eq!(numeric.bits, KnownBits::top());
                assert_eq!(numeric.interval, Interval::top());
            }
            2 => numeric.bits = KnownBits::exact(U256::from(7)),
            _ => numeric.interval = Interval::exact(U256::from(7)),
        }
        let original = EvmValue::constant(U256::from(7))
            .with_numeric(numeric)
            .with_identity(91, 3);
        let actual = identify(original.clone(), true);
        assert_eq!(
            actual, original,
            "cached view {view} must not acquire Fresh"
        );
        assert!(actual.same_identity(&original));
        assert_eq!(probe.counts(), (0, 0, 0));
    }
}

#[test]
fn opaque_literals_and_unknowns_still_get_independent_expressions_without_queries() {
    let probe = Rc::new(Probe::new(usize::MAX));
    for numeric in [CachedWord::constant(U256::from(7)), CachedWord::top()] {
        let mut numeric = numeric;
        numeric.opaque = numeric.opaque.watched(&probe);
        // Constant provenance alone cannot replace an unavailable scalar proof.
        let original = EvmValue::constant(U256::from(7)).with_numeric(numeric);
        let first = identify(original.clone(), true);
        let second = identify(original.clone(), true);
        assert!(matches!(
            first.expression().unwrap().kind(),
            ExprKind::Fresh(_)
        ));
        assert_ne!(first.expression(), second.expression());
        assert_eq!(first.numeric(), original.numeric());
        assert_eq!(first.provenance(), original.provenance());
        assert_eq!(identify(original.clone(), false), original);
    }
    assert_eq!(probe.counts(), (0, 0, 0));
}

#[test]
fn existing_expressions_and_literal_metadata_are_preserved_without_queries() {
    let probe = Rc::new(Probe::new(usize::MAX));
    let mut numeric = CachedWord::constant(U256::from(7));
    numeric.opaque = numeric.opaque.watched(&probe);
    numeric.bits = KnownBits::exact(U256::from(7));
    let value = EvmValue::constant(U256::from(7))
        .with_numeric(numeric)
        .with_identity(91, 3)
        .with_symbolic_limit();
    for expression in [ExprId::constant(U256::from(7)), ExprId::fresh().unwrap()] {
        let original = value.clone().with_expression(expression);
        for enabled in [false, true] {
            let actual = identify(original.clone(), enabled);
            assert_eq!(actual, original);
            assert!(actual.same_identity(&original));
        }
    }
    assert_eq!(probe.counts(), (0, 0, 0));
}

#[test]
fn exact_observations_inputs_roles_and_limits_do_not_become_syntactic_literals() {
    let literal = EvmValue::constant(U256::from(7)).with_identity(91, 3);
    let mut storage = literal.clone();
    storage.set_origin(Origin::Storage);
    let mut input = literal
        .clone()
        .with_numeric(NumericValue::top())
        .with_symbol(Symbol::CalldataWord(U256::ZERO), Some(93))
        .with_numeric(NumericValue::constant(U256::from(7)));
    input.forget_expression();
    assert!(input.identity().input().is_some());
    let mut address = Domain::default()
        .from_facts(&[UnaryPredicate::IsCodeAddress])
        .unwrap()
        .with_numeric(NumericValue::constant(U256::from(7)))
        .with_identity(91, 3);
    address.set_origin(Origin::Constant);
    assert!(address.provenance().is_code_address());
    for original in [
        storage,
        input,
        address,
        literal.clone().with_symbolic_limit(),
        literal.with_origin(Origin::Storage),
    ] {
        assert!(original.expression().is_none());
        let mut actual = identify(original.clone(), true);
        assert!(matches!(
            actual.expression().unwrap().kind(),
            ExprKind::Fresh(_)
        ));
        assert!(actual.same_identity(&original));
        actual.forget_expression();
        assert_eq!(
            actual, original,
            "only the independent expression may change"
        );
    }
}

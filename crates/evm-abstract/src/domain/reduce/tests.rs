use super::export;
use crate::{
    U256,
    domain::{
        DomainSpec, EvmValue, NumericValue,
        congruence::Congruence,
        facts::{
            BitConstraints, Fact, FactError, FactLattice, FiniteSet, Symbol, UnaryFact,
            UnaryPredicate, WordBounds,
        },
        interval::Interval,
        known_bits::KnownBits,
        provenance::{Origin, OriginSet},
    },
};
use std::{collections::BTreeSet, num::NonZeroUsize};

fn domain(facts: usize) -> DomainSpec {
    DomainSpec::new(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(facts).unwrap(),
    )
}

fn unary(predicate: UnaryPredicate) -> Fact {
    Fact::Unary(UnaryFact::new(Symbol::THIS, predicate))
}

fn five_facts() -> Vec<Fact> {
    let five = U256::from(5);
    let signed = (U256::ONE << 255) + five;
    vec![
        unary(UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([five])).unwrap(),
        )),
        unary(UnaryPredicate::KnownBits(
            BitConstraints::new(!five, five).unwrap(),
        )),
        unary(UnaryPredicate::UnsignedBounds(
            WordBounds::new(five, five).unwrap(),
        )),
        unary(UnaryPredicate::SignedBounds(
            WordBounds::new(signed, signed).unwrap(),
        )),
        unary(UnaryPredicate::Congruent(Congruence::exact(five))),
        unary(UnaryPredicate::NonZero),
        unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
            Origin::Constant,
        ))),
    ]
}

#[test]
fn export_exact_capacity_and_one_below_keep_the_expected_ordered_prefix() {
    let value = EvmValue::<NumericValue>::constant(U256::from(5));
    for (capacity, changes, result) in [
        (7, 7, Ok(())),
        (
            6,
            6,
            Err(FactError::Capacity {
                max_atoms: 6,
                required: 7,
            }),
        ),
    ] {
        let mut lattice = FactLattice::new(capacity);
        let mut strengthened = 0;
        assert_eq!(
            export(domain(capacity), &value, &mut lattice, &mut strengthened),
            result
        );
        assert_eq!(strengthened, changes);
        let mut expected = five_facts();
        if capacity == 6 {
            // Origins precede NonZero in export; the latter was never reached.
            expected.remove(5);
        }
        assert_eq!(lattice.facts(), expected);
        assert_eq!(lattice.atoms(), capacity);
        // Repeated feedback neither discards the prefix nor counts it twice.
        assert_eq!(
            export(domain(capacity), &value, &mut lattice, &mut strengthened),
            result
        );
        assert_eq!(strengthened, changes);
        assert_eq!(lattice.facts(), expected);
    }
}

#[test]
fn export_contradiction_preserves_the_successful_facts_before_it() {
    let mut numeric = NumericValue::top();
    numeric.bits = KnownBits::exact(U256::ZERO);
    numeric.interval = Interval::exact(U256::ONE);
    let value = EvmValue::from_numeric(numeric);
    let mut lattice = FactLattice::new(8);
    let mut changes = 0;
    assert_eq!(
        export(domain(8), &value, &mut lattice, &mut changes),
        Err(FactError::Contradiction {
            subject: Symbol::THIS
        })
    );
    assert_eq!(changes, 1);
    assert_eq!(
        lattice.facts(),
        vec![unary(UnaryPredicate::KnownBits(
            BitConstraints::new(U256::MAX, U256::ZERO).unwrap(),
        ))]
    );
    assert_eq!(
        lattice.scalar(Symbol::THIS).unwrap().singleton(),
        Some(U256::ZERO)
    );
}

#[test]
fn export_origin_conflict_preserves_the_numeric_prefix_and_original_origin() {
    let mut lattice = FactLattice::new(16);
    lattice
        .insert(unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
            Origin::Storage,
        ))))
        .unwrap();
    let mut changes = 0;
    assert_eq!(
        export(
            domain(16),
            &EvmValue::<NumericValue>::constant(U256::from(5)),
            &mut lattice,
            &mut changes
        ),
        Err(FactError::OriginContradiction {
            subject: Symbol::THIS
        })
    );
    assert_eq!(changes, 5);
    let mut expected = five_facts();
    expected.remove(5);
    *expected.last_mut().unwrap() = unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
        Origin::Storage,
    )));
    assert_eq!(lattice.facts(), expected);
    assert_eq!(
        lattice.scalar(Symbol::THIS).unwrap().singleton(),
        Some(U256::from(5))
    );
}

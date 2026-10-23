use super::{
    BTreeSet, BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FactChange, FactError,
    FactLattice, FiniteSet, OperationRelation, Origin, OriginSet, RelationalFact, SCALAR_MEETS,
    SCALAR_VALIDATIONS, Symbol, Term, UnaryFact, UnaryPredicate, WordBounds, reset_scalar_work,
    scalar_work, unary,
};
use crate::domain::congruence::Congruence;
use alloy_primitives::U256;

#[test]
fn independent_slots_reuse_validated_feedback_but_validate_every_change() {
    let _meets = SCALAR_MEETS.scope();
    let _validations = SCALAR_VALIDATIONS.scope();
    let mut lattice = FactLattice::new(8);
    let other = Symbol::new(7);
    lattice
        .insert(Fact::Unary(UnaryFact::new(other, UnaryPredicate::IsZero)))
        .unwrap();
    for predicate in [
        UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ONE, U256::from(3)])).unwrap(),
        ),
        UnaryPredicate::BitSet(BitIndex::new(0).unwrap()),
        UnaryPredicate::NonZero,
        UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Storage)),
    ] {
        reset_scalar_work();
        assert_eq!(
            lattice.insert(unary(predicate.clone())),
            Ok(FactChange::Strengthened)
        );
        assert_eq!(scalar_work(), (0, 1));
        reset_scalar_work();
        assert_eq!(lattice.insert(unary(predicate)), Ok(FactChange::Unchanged));
        assert_eq!(
            scalar_work(),
            (0, 0),
            "committed unchanged slots are already valid"
        );
    }
    assert_eq!(
        lattice.facts(),
        vec![
            unary(UnaryPredicate::MemberOf(
                FiniteSet::new(BTreeSet::from([U256::ONE, U256::from(3)])).unwrap(),
            )),
            unary(UnaryPredicate::KnownBits(
                BitConstraints::new(U256::ZERO, U256::ONE).unwrap()
            )),
            unary(UnaryPredicate::NonZero),
            unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
                Origin::Storage
            ))),
            Fact::Unary(UnaryFact::new(
                other,
                UnaryPredicate::MemberOf(FiniteSet::new(BTreeSet::from([U256::ZERO])).unwrap(),)
            )),
        ]
    );
    assert_eq!(lattice.atoms(), 6);
    assert!(!lattice.equivalent(Symbol::THIS, other));
}

#[test]
fn independent_slot_refusal_keeps_numeric_and_source_facts_and_error_order() {
    let mut lattice = FactLattice::new(3);
    for predicate in [
        UnaryPredicate::Exact(U256::ONE),
        UnaryPredicate::NonZero,
        UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Storage)),
    ] {
        lattice.insert(unary(predicate)).unwrap();
    }
    let expected = lattice.facts();
    for (predicate, error) in [
        (
            UnaryPredicate::IsCodeAddress,
            FactError::Capacity {
                max_atoms: 3,
                required: 4,
            },
        ),
        (
            UnaryPredicate::IsZero,
            FactError::Contradiction {
                subject: Symbol::THIS,
            },
        ),
        (
            UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Memory)),
            FactError::OriginContradiction {
                subject: Symbol::THIS,
            },
        ),
        // The full input set is refused before intersection with the singleton.
        (
            UnaryPredicate::MemberOf(FiniteSet::new((0u64..4).map(U256::from).collect()).unwrap()),
            FactError::Capacity {
                max_atoms: 3,
                required: 4,
            },
        ),
    ] {
        assert_eq!(lattice.insert(unary(predicate)), Err(error));
        assert_eq!(lattice.facts(), expected);
        assert_eq!(lattice.atoms(), 3);
    }
    let scalar = lattice.scalar(Symbol::THIS).unwrap();
    assert_eq!(scalar.singleton(), Some(U256::ONE));
    assert_eq!(
        scalar.possible_origins(),
        Some(&OriginSet::source(Origin::Storage))
    );
    assert!(!scalar.is_code_address());
}

#[test]
fn changing_then_filtering_a_finite_slot_keeps_the_original_unchanged_result() {
    let mut lattice = FactLattice::new(2);
    lattice
        .insert(unary(UnaryPredicate::BitSet(BitIndex::new(0).unwrap())))
        .unwrap();
    lattice
        .insert(unary(UnaryPredicate::Exact(U256::ONE)))
        .unwrap();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ZERO, U256::ONE])).unwrap(),
        ))),
        Ok(FactChange::Unchanged)
    );
    assert_eq!(lattice.atoms(), 2);
    assert_eq!(
        lattice.scalar(Symbol::THIS).unwrap().finite(),
        Some(&BTreeSet::from([U256::ONE]))
    );
}

#[test]
fn slot_staging_matches_the_original_public_transaction_after_every_fact() {
    let bias = U256::ONE << 255;
    let predicates = [
        UnaryPredicate::Exact(U256::ZERO),
        UnaryPredicate::Exact(U256::ONE),
        UnaryPredicate::Exact(U256::MAX),
        UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ZERO, U256::ONE, U256::MAX])).unwrap(),
        ),
        UnaryPredicate::KnownBits(BitConstraints::new(U256::ZERO, U256::ZERO).unwrap()),
        UnaryPredicate::KnownBits(BitConstraints::new(U256::MAX, U256::ZERO).unwrap()),
        UnaryPredicate::BitSet(BitIndex::new(0).unwrap()),
        UnaryPredicate::BitClear(BitIndex::new(255).unwrap()),
        UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ZERO, U256::ONE).unwrap()),
        UnaryPredicate::UnsignedBounds(WordBounds::new(U256::ONE, U256::ONE).unwrap()),
        UnaryPredicate::SignedBounds(WordBounds::new(bias, bias + U256::ONE).unwrap()),
        UnaryPredicate::Congruent(Congruence::top()),
        UnaryPredicate::Congruent(Congruence::exact(U256::ZERO)),
        UnaryPredicate::congruent(U256::from(2), U256::ONE).unwrap(),
        UnaryPredicate::IsZero,
        UnaryPredicate::NonZero,
        UnaryPredicate::IsAddress,
        UnaryPredicate::IsCodeAddress,
        UnaryPredicate::PossibleOrigins(OriginSet::top()),
        UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Storage)),
        UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Memory)),
    ];
    let other = Symbol::new(1);
    let seeds = [
        None,
        Some(Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Eq,
            Term::Symbol(other),
        ))),
        Some(Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Ne,
            Term::Symbol(other),
        ))),
        Some(Fact::Relation(RelationalFact::Operation(
            OperationRelation::new(0x15, other, vec![Term::Symbol(Symbol::THIS)]).unwrap(),
        ))),
    ];
    for capacity in 0..=12 {
        for seed in &seeds {
            for first in &predicates {
                for second in &predicates {
                    for second_subject in [Symbol::THIS, other] {
                        let first = unary(first.clone());
                        let second = Fact::Unary(UnaryFact::new(second_subject, second.clone()));
                        let mut actual = FactLattice::new(capacity);
                        let mut original = actual.clone();
                        // Continuing after a refusal also tests the committed prefix.
                        for fact in seed.iter().chain([&first, &second, &first]) {
                            let before = actual.clone();
                            let expected = insert_full_transaction(&mut original, fact.clone());
                            let result = actual.insert(fact.clone());
                            assert_eq!(result, expected, "capacity={capacity}, fact={fact:?}");
                            assert_eq!(actual, original, "capacity={capacity}, fact={fact:?}");
                            assert!(actual.atoms() <= capacity);
                            if result.is_err() {
                                assert_eq!(actual, before, "a refusal must be atomic");
                            }
                        }
                    }
                }
            }
        }
    }
}

// The public transaction from a3c42493 retains full-table staging and bypasses
// optimized insert. Predicate, validation and normalization helpers are shared;
// the separate normalization assertions provide their independent coverage.
fn insert_full_transaction(lattice: &mut FactLattice, fact: Fact) -> Result<FactChange, FactError> {
    if let Fact::Unary(UnaryFact {
        predicate: UnaryPredicate::MemberOf(set),
        ..
    }) = &fact
        && set.values().len() > lattice.max_atoms
    {
        return Err(FactError::Capacity {
            max_atoms: lattice.max_atoms,
            required: set.values().len(),
        });
    }
    let mut next = lattice.clone();
    next.apply(fact)?;
    let required = next.atoms();
    if required > lattice.max_atoms {
        return Err(FactError::Capacity {
            max_atoms: lattice.max_atoms,
            required,
        });
    }
    if next == *lattice {
        return Ok(FactChange::Unchanged);
    }
    *lattice = next;
    Ok(FactChange::Strengthened)
}

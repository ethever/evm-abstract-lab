use super::{
    BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FactChange, FactError,
    FactLattice, FiniteSet, OperationRelation, RelationalFact, Symbol, Term, UnaryFact,
    UnaryPredicate, WordBounds,
};
use crate::domain::provenance::{Origin, OriginSet};
use alloy_primitives::U256;
use std::collections::BTreeSet;

fn unary(predicate: UnaryPredicate) -> Fact {
    Fact::Unary(UnaryFact::new(Symbol::THIS, predicate))
}

#[test]
fn repeated_feedback_is_deduplicated_and_strict_strengthening_is_reported() {
    let mut lattice = FactLattice::new(8);
    let clear = unary(UnaryPredicate::BitClear(
        BitIndex::new(2).expect("valid bit"),
    ));
    assert_eq!(lattice.insert(clear.clone()), Ok(FactChange::Strengthened));
    let size = lattice.atoms();
    for _ in 0..100 {
        assert_eq!(lattice.insert(clear.clone()), Ok(FactChange::Unchanged));
        assert_eq!(lattice.atoms(), size);
    }
    let one = unary(UnaryPredicate::BitSet(BitIndex::new(0).expect("valid bit")));
    assert_eq!(lattice.insert(one), Ok(FactChange::Strengthened));
    assert_eq!(lattice.atoms(), size, "bit facts share a compact mask slot");
}

#[test]
fn finite_membership_is_a_disjunction_and_intersects_other_constraints() {
    let mut lattice = FactLattice::new(16);
    let candidates = BTreeSet::from([U256::from(1), U256::from(2)]);
    lattice
        .insert(unary(UnaryPredicate::MemberOf(
            FiniteSet::new(candidates).expect("nonempty"),
        )))
        .expect("disjunctive membership");
    lattice
        .insert(unary(UnaryPredicate::BitClear(
            BitIndex::new(0).expect("valid bit"),
        )))
        .expect("two is even");
    assert_eq!(
        lattice.scalar(Symbol::THIS).expect("scalar").finite(),
        Some(&BTreeSet::from([U256::from(2)]))
    );
    assert!(
        lattice
            .scalar(Symbol::THIS)
            .expect("scalar")
            .contains(U256::from(2))
    );
}

#[test]
fn contradictory_numerical_and_origin_facts_have_distinct_errors_and_roll_back() {
    let mut lattice = FactLattice::new(16);
    lattice.insert(unary(UnaryPredicate::IsZero)).expect("zero");
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::NonZero)),
        Err(FactError::Contradiction {
            subject: Symbol::THIS
        })
    );
    assert_eq!(lattice, before);

    lattice
        .insert(unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
            Origin::Calldata,
        ))))
        .expect("known source");
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
            Origin::Storage
        )))),
        Err(FactError::OriginContradiction {
            subject: Symbol::THIS
        })
    );
    assert_eq!(lattice, before);
}

#[test]
fn capacity_is_a_resource_stop_and_keeps_the_old_complete_table() {
    let mut lattice = FactLattice::new(1);
    lattice
        .insert(unary(UnaryPredicate::Exact(U256::from(2))))
        .expect("one atom");
    let before = lattice.clone();
    assert!(matches!(
        lattice.insert(unary(UnaryPredicate::NonZero)),
        Err(FactError::Capacity {
            max_atoms: 1,
            required: 2
        })
    ));
    assert_eq!(lattice, before);

    let large = FiniteSet::new(BTreeSet::from([U256::from(1), U256::from(2)])).expect("nonempty");
    assert!(matches!(
        lattice.insert(unary(UnaryPredicate::MemberOf(large))),
        Err(FactError::Capacity { .. })
    ));
}

#[test]
fn raw_fact_constructors_reject_invalid_declarations() {
    assert_eq!(BitIndex::new(256), Err(FactError::InvalidBitIndex(256)));
    assert_eq!(
        BitConstraints::new(U256::from(1), U256::from(1)),
        Err(FactError::ConflictingBits)
    );
    assert_eq!(
        WordBounds::new(U256::from(2), U256::from(1)),
        Err(FactError::InvalidBounds)
    );
    assert_eq!(
        FiniteSet::new(BTreeSet::new()),
        Err(FactError::EmptyFiniteSet)
    );
    assert_eq!(
        UnaryPredicate::multiple_of(U256::ZERO),
        Err(FactError::InvalidModulus)
    );
}

#[test]
fn general_congruences_merge_into_one_crt_slot() {
    let mut lattice = FactLattice::new(8);
    lattice
        .insert(unary(
            UnaryPredicate::congruent(U256::from(3), U256::from(1)).expect("valid modulus"),
        ))
        .expect("mod three");
    lattice
        .insert(unary(
            UnaryPredicate::congruent(U256::from(5), U256::from(2)).expect("valid modulus"),
        ))
        .expect("mod five");
    assert_eq!(lattice.atoms(), 1);
    let congruence = lattice
        .scalar(Symbol::THIS)
        .expect("scalar")
        .congruence()
        .expect("congruence");
    assert!(congruence.contains(U256::from(7)));
    assert!(congruence.contains(U256::from(22)));
    assert!(!congruence.contains(U256::from(8)));
}

#[test]
fn signed_and_unsigned_bounds_use_different_coordinates() {
    let mut lattice = FactLattice::new(16);
    let sign = U256::from(1) << 255;
    lattice
        .insert(unary(UnaryPredicate::UnsignedBounds(
            WordBounds::new(sign, sign).expect("range"),
        )))
        .expect("unsigned exact minimum signed word");
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::SignedBounds(
            WordBounds::new(sign, sign).expect("range")
        ))),
        Err(FactError::Contradiction {
            subject: Symbol::THIS
        })
    );
    let compatible = WordBounds::new(U256::ZERO, U256::ZERO).expect("range");
    lattice
        .insert(unary(UnaryPredicate::SignedBounds(compatible)))
        .expect("the signed minimum maps to zero");
}

#[test]
fn explicit_equality_is_transitive_but_sources_and_numbers_do_not_create_identity() {
    let a = Symbol::new(1);
    let b = Symbol::new(2);
    let c = Symbol::new(3);
    let mut lattice = FactLattice::new(16);
    for (left, right) in [(a, b), (b, c)] {
        lattice
            .insert(Fact::Binary(BinaryFact::new(
                Term::Symbol(left),
                BinaryPredicate::Eq,
                Term::Symbol(right),
            )))
            .expect("trusted equality");
    }
    assert!(lattice.equivalent(a, c));
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(Fact::Binary(BinaryFact::new(
            Term::Symbol(a),
            BinaryPredicate::Ne,
            Term::Symbol(c),
        ))),
        Err(FactError::Contradiction { subject: a })
    );
    assert_eq!(lattice, before);
}

#[test]
fn equality_components_have_canonical_edges_and_share_scalar_facts() {
    let a = Symbol::new(1);
    let b = Symbol::new(2);
    let c = Symbol::new(3);
    let equality = |left, right| {
        Fact::Binary(BinaryFact::new(
            Term::Symbol(left),
            BinaryPredicate::Eq,
            Term::Symbol(right),
        ))
    };
    let mut forward = FactLattice::new(16);
    let mut backward = FactLattice::new(16);
    for fact in [equality(a, b), equality(b, c)] {
        forward.insert(fact).expect("trusted equality");
    }
    for fact in [equality(c, b), equality(b, a)] {
        backward.insert(fact).expect("trusted equality");
    }
    assert_eq!(forward, backward);
    assert_eq!(forward.insert(equality(a, c)), Ok(FactChange::Unchanged));
    forward
        .insert(Fact::Unary(UnaryFact::new(
            c,
            UnaryPredicate::Exact(U256::from(3)),
        )))
        .expect("a property of an alias");
    assert_eq!(forward.scalar(a), forward.scalar(c));
    let before = forward.clone();
    assert_eq!(
        forward.insert(Fact::Unary(UnaryFact::new(
            b,
            UnaryPredicate::Exact(U256::from(4))
        ))),
        Err(FactError::Contradiction { subject: b })
    );
    assert_eq!(forward, before);
}

#[test]
fn numerical_equality_does_not_merge_lineage_or_usage_roles() {
    let a = Symbol::new(1);
    let b = Symbol::new(2);
    let mut lattice = FactLattice::new(32);
    lattice
        .insert(Fact::Unary(UnaryFact::new(
            a,
            UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Calldata)),
        )))
        .expect("calldata lineage");
    lattice
        .insert(Fact::Unary(UnaryFact::new(
            b,
            UnaryPredicate::PossibleOrigins(OriginSet::source(Origin::Storage)),
        )))
        .expect("storage lineage");
    lattice
        .insert(Fact::Unary(UnaryFact::new(
            a,
            UnaryPredicate::IsCodeAddress,
        )))
        .expect("only a has this usage role");
    lattice
        .insert(Fact::Binary(BinaryFact::new(
            Term::Symbol(a),
            BinaryPredicate::Eq,
            Term::Symbol(b),
        )))
        .expect("equal contents can have different lineages");
    assert!(lattice.equivalent(a, b));
    assert!(lattice.scalar(a).expect("a").is_code_address());
    assert!(!lattice.scalar(b).expect("b").is_code_address());
    assert_ne!(
        lattice.scalar(a).expect("a").possible_origins(),
        lattice.scalar(b).expect("b").possible_origins()
    );
}

#[test]
fn comparisons_against_constants_refine_numeric_facts() {
    let mut lattice = FactLattice::new(32);
    lattice
        .insert(Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Ult,
            Term::Constant(U256::from(8)),
        )))
        .expect("less than eight");
    lattice
        .insert(Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Ne,
            Term::Constant(U256::ZERO),
        )))
        .expect("nonzero");
    let scalar = lattice.scalar(Symbol::THIS).expect("scalar");
    assert!(!scalar.contains(U256::ZERO));
    assert!(scalar.contains(U256::from(1)));
    assert!(!scalar.contains(U256::from(8)));
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::Exact(U256::from(8)))),
        Err(FactError::Contradiction {
            subject: Symbol::THIS
        })
    );
    assert_eq!(lattice, before);

    let mut signed = FactLattice::new(32);
    signed
        .insert(Fact::Binary(BinaryFact::new(
            Term::Symbol(Symbol::THIS),
            BinaryPredicate::Slt,
            Term::Constant(U256::ZERO),
        )))
        .expect("signed negative");
    let scalar = signed.scalar(Symbol::THIS).expect("scalar");
    assert!(scalar.contains(U256::MAX));
    assert!(!scalar.contains(U256::ZERO));
}

#[test]
fn operation_relations_are_validated_and_deduplicated() {
    let a = Symbol::new(1);
    let result = Symbol::new(2);
    assert!(matches!(
        OperationRelation::new(0x01, result, vec![Term::Symbol(a)]),
        Err(FactError::InvalidOperation {
            expected: 2,
            actual: 1,
            ..
        })
    ));
    assert_eq!(
        OperationRelation::new(0x54, result, vec![]),
        Err(FactError::UnsupportedOperation(0x54))
    );
    let relation = OperationRelation::new(0x15, result, vec![Term::Symbol(a)])
        .expect("iszero has one operand");
    let fact = Fact::Relation(RelationalFact::Operation(relation));
    let mut lattice = FactLattice::new(8);
    assert_eq!(lattice.insert(fact.clone()), Ok(FactChange::Strengthened));
    assert_eq!(lattice.insert(fact), Ok(FactChange::Unchanged));
    assert_eq!(lattice.atoms(), 2);
}

#[test]
fn conjunction_slot_updates_commute_and_idempotently_preserve_their_semantics() {
    let facts = [
        unary(UnaryPredicate::KnownBits(
            BitConstraints::new(U256::from(2), U256::from(1)).expect("bits"),
        )),
        unary(UnaryPredicate::UnsignedBounds(
            WordBounds::new(U256::from(1), U256::from(9)).expect("bounds"),
        )),
        unary(UnaryPredicate::congruent(U256::from(2), U256::from(1)).expect("congruence")),
    ];
    let mut forward = FactLattice::new(16);
    let mut backward = FactLattice::new(16);
    for fact in &facts {
        forward.insert(fact.clone()).expect("consistent");
    }
    for fact in facts.iter().rev() {
        backward.insert(fact.clone()).expect("consistent");
    }
    assert_eq!(forward, backward);
    for fact in facts {
        assert_eq!(forward.insert(fact), Ok(FactChange::Unchanged));
    }
}

#[test]
fn address_range_and_code_role_have_separate_guarantees() {
    let mut lattice = FactLattice::new(8);
    lattice
        .insert(unary(UnaryPredicate::IsCodeAddress))
        .expect("role");
    let role = lattice.scalar(Symbol::THIS).expect("scalar");
    assert!(role.is_code_address());
    assert!(!role.is_address());
    assert!(
        role.contains(U256::MAX),
        "role alone supplies no numeric proof"
    );
    lattice
        .insert(unary(UnaryPredicate::IsAddress))
        .expect("range");
    let address = lattice.scalar(Symbol::THIS).expect("scalar");
    assert!(address.contains((U256::from(1) << 160) - U256::from(1)));
    assert!(!address.contains(U256::from(1) << 160));
}

#[test]
fn tutorial_initial_facts_exchange_bounds_and_general_congruence() {
    let value = crate::domain::Domain::default()
        .from_facts(&[
            UnaryPredicate::UnsignedBounds(
                WordBounds::new(U256::from(1), U256::from(20)).expect("range"),
            ),
            UnaryPredicate::multiple_of(U256::from(8)).expect("modulus"),
        ])
        .expect("compatible initial constraints");
    assert_eq!(
        value.constants(),
        Some(&BTreeSet::from([U256::from(8), U256::from(16)]))
    );
    assert!(!value.contains(U256::from(7)));
}

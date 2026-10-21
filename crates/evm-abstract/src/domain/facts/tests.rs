use super::{
    BinaryFact, BinaryPredicate, BitConstraints, BitIndex, Fact, FactChange, FactError,
    FactLattice, FiniteSet, OperationRelation, RelationalFact, ScalarFacts, Symbol, Term,
    UnaryFact, UnaryPredicate, WordBounds,
};
use crate::{
    domain::provenance::{Origin, OriginSet},
    test_instrumentation::Counter,
};
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

pub(super) static SCALAR_MEETS: Counter<usize> = Counter::new(0);
pub(super) static SCALAR_VALIDATIONS: Counter<usize> = Counter::new(0);

fn reset_scalar_work() {
    SCALAR_MEETS.set(0);
    SCALAR_VALIDATIONS.set(0);
}

fn scalar_work() -> (usize, usize) {
    (SCALAR_MEETS.get(), SCALAR_VALIDATIONS.get())
}

fn unary(predicate: UnaryPredicate) -> Fact {
    Fact::Unary(UnaryFact::new(Symbol::THIS, predicate))
}

fn assert_unary_matches_general(lattice: &mut FactLattice, fact: UnaryFact, expected: FactChange) {
    assert!(lattice.binary.is_empty() && lattice.relations.is_empty());
    let mut general = lattice.clone();
    general.apply_unary(fact.clone()).expect("valid unary fact");
    general
        .normalize_equalities_general()
        .expect("general normalization");
    general.validate_relations().expect("valid relations");
    assert!(general.atoms() <= general.max_atoms);
    assert_eq!(lattice.insert(Fact::Unary(fact)), Ok(expected));
    assert_eq!(
        *lattice, general,
        "compare the complete lattice, including representatives"
    );
}

#[test]
fn unary_numeric_updates_preserve_general_normalization_exactly() {
    let mut lattice = FactLattice::new(32);
    let candidates = BTreeSet::from([
        U256::ZERO,
        U256::from(1),
        U256::from(3),
        U256::from(5),
        U256::from(6),
        U256::from(9),
        U256::MAX,
    ]);
    assert_unary_matches_general(
        &mut lattice,
        UnaryFact::new(
            Symbol::THIS,
            UnaryPredicate::MemberOf(FiniteSet::new(candidates).expect("nonempty")),
        ),
        FactChange::Strengthened,
    );
    let bias = U256::from(1) << 255;
    let bits = BitConstraints::new(U256::from(2), U256::from(1)).expect("bits");
    let unsigned = WordBounds::new(U256::from(5), U256::from(9)).expect("bounds");
    let signed = WordBounds::new(bias + U256::from(6), bias + U256::from(9)).expect("bounds");
    for (predicate, expected) in [
        (UnaryPredicate::KnownBits(bits), vec![1u64, 5, 9]),
        (UnaryPredicate::UnsignedBounds(unsigned), vec![5, 9]),
        (UnaryPredicate::SignedBounds(signed), vec![9]),
        (
            UnaryPredicate::congruent(U256::from(4), U256::from(1)).expect("modulus"),
            vec![9],
        ),
        (UnaryPredicate::NonZero, vec![9]),
    ] {
        assert_unary_matches_general(
            &mut lattice,
            UnaryFact::new(Symbol::THIS, predicate),
            FactChange::Strengthened,
        );
        assert_eq!(
            lattice.scalar(Symbol::THIS).expect("scalar").finite(),
            Some(
                &expected
                    .into_iter()
                    .map(U256::from)
                    .collect::<BTreeSet<_>>()
            ),
        );
    }
    let scalar = lattice.scalar(Symbol::THIS).expect("scalar");
    assert_eq!(scalar.known_bits(), Some(bits));
    assert_eq!(scalar.unsigned_bounds(), Some(unsigned));
    assert_eq!(scalar.signed_bounds(), Some(signed));
    assert_eq!(
        scalar.congruence().expect("congruence").modulus_residue(),
        Some((U256::from(4), U256::from(1)))
    );
    assert!(scalar.nonzero());
    assert!(scalar.contains(U256::from(9)));
    assert!(!scalar.contains(U256::from(5)));
    assert_unary_matches_general(
        &mut lattice,
        UnaryFact::new(Symbol::THIS, UnaryPredicate::Exact(U256::from(9))),
        FactChange::Unchanged,
    );

    let mut zero = FactLattice::new(2);
    for predicate in [
        UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::ZERO, U256::from(1)])).expect("nonempty"),
        ),
        UnaryPredicate::IsZero,
    ] {
        assert_unary_matches_general(
            &mut zero,
            UnaryFact::new(Symbol::THIS, predicate),
            FactChange::Strengthened,
        );
    }
    assert_eq!(
        zero.scalar(Symbol::THIS).expect("zero").finite(),
        Some(&BTreeSet::from([U256::ZERO]))
    );
    assert!(!zero.scalar(Symbol::THIS).expect("zero").nonzero());
}

#[test]
fn unary_validation_keeps_singleton_contradictions_atomic_without_finite_candidates() {
    let bias = U256::from(1) << 255;
    for predicate in [
        UnaryPredicate::KnownBits(BitConstraints::new(U256::MAX, U256::ZERO).expect("zero bits")),
        UnaryPredicate::UnsignedBounds(
            WordBounds::new(U256::ZERO, U256::ZERO).expect("zero bounds"),
        ),
        UnaryPredicate::SignedBounds(WordBounds::new(bias, bias).expect("signed zero bounds")),
        UnaryPredicate::Congruent(super::Congruence::exact(U256::ZERO)),
        UnaryPredicate::IsZero,
    ] {
        let mut lattice = FactLattice::new(8);
        lattice
            .insert(unary(UnaryPredicate::NonZero))
            .expect("nonzero");
        let before = lattice.clone();
        assert_eq!(
            lattice.insert(unary(predicate)),
            Err(FactError::Contradiction {
                subject: Symbol::THIS
            })
        );
        assert_eq!(lattice, before);
    }
}

#[test]
fn unary_top_and_redundant_facts_keep_the_original_change_results() {
    for predicate in [
        UnaryPredicate::KnownBits(
            BitConstraints::new(U256::ZERO, U256::ZERO).expect("unknown bits"),
        ),
        UnaryPredicate::UnsignedBounds(
            WordBounds::new(U256::ZERO, U256::MAX).expect("full bounds"),
        ),
        UnaryPredicate::SignedBounds(WordBounds::new(U256::ZERO, U256::MAX).expect("full bounds")),
        UnaryPredicate::Congruent(super::Congruence::top()),
        UnaryPredicate::PossibleOrigins(OriginSet::top()),
    ] {
        let mut lattice = FactLattice::new(1);
        let fact = UnaryFact::new(Symbol::THIS, predicate);
        // Some(Top) remains a stored slot, as in the existing general path.
        assert_unary_matches_general(&mut lattice, fact.clone(), FactChange::Strengthened);
        assert_eq!(lattice.atoms(), 1);
        assert_eq!(lattice.scalar(Symbol::THIS).expect("scalar").finite(), None);
        assert_unary_matches_general(&mut lattice, fact, FactChange::Unchanged);
    }
    let mut lattice = FactLattice::new(1);
    for (predicate, change) in [
        (
            UnaryPredicate::BitSet(BitIndex::new(0).expect("bit")),
            FactChange::Strengthened,
        ),
        (
            UnaryPredicate::KnownBits(
                BitConstraints::new(U256::ZERO, U256::ZERO).expect("unknown bits"),
            ),
            FactChange::Unchanged,
        ),
        (
            UnaryPredicate::BitSet(BitIndex::new(0).expect("bit")),
            FactChange::Unchanged,
        ),
    ] {
        assert_unary_matches_general(
            &mut lattice,
            UnaryFact::new(Symbol::THIS, predicate),
            change,
        );
    }
}

#[test]
fn unary_default_cleanup_preserves_pre_cleanup_identity_representatives() {
    let removed = Symbol::new(1);
    let stale = Symbol::new(2);
    let mut lattice = FactLattice::new(1);
    lattice
        .insert(unary(UnaryPredicate::NonZero))
        .expect("nonzero");
    lattice.scalars.insert(removed, ScalarFacts::default());
    lattice.representatives.insert(stale, Symbol::THIS);
    let mut general = lattice.clone();
    general
        .normalize_equalities_general()
        .expect("general normalization");
    lattice.normalize_equalities().expect("unary normalization");
    assert_eq!(lattice, general);
    assert_eq!(lattice.scalar(removed), None);
    assert_eq!(
        lattice.representatives,
        BTreeMap::from([(Symbol::THIS, Symbol::THIS), (removed, removed)])
    );
    assert!(!lattice.equivalent(removed, Symbol::THIS));
    assert!(!lattice.equivalent(stale, Symbol::THIS));
    // The next pass drops the removed slot's identity entry, just as before.
    assert_unary_matches_general(
        &mut lattice,
        UnaryFact::new(Symbol::THIS, UnaryPredicate::NonZero),
        FactChange::Strengthened,
    );
    assert_unary_matches_general(
        &mut lattice,
        UnaryFact::new(Symbol::THIS, UnaryPredicate::NonZero),
        FactChange::Unchanged,
    );
}

#[test]
fn unary_normalization_skips_meets_and_revalidation_but_keeps_apply_validation() {
    let _meets = SCALAR_MEETS.scope();
    let _validations = SCALAR_VALIDATIONS.scope();
    let mut lattice = FactLattice::new(8);
    lattice
        .insert(unary(UnaryPredicate::Exact(U256::from(4))))
        .expect("finite");
    lattice
        .insert(Fact::Unary(UnaryFact::new(
            Symbol::new(1),
            UnaryPredicate::NonZero,
        )))
        .expect("second scalar");
    reset_scalar_work();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::BitClear(
            BitIndex::new(0).expect("bit")
        ))),
        Ok(FactChange::Strengthened)
    );
    assert_eq!(scalar_work(), (0, 1), "only apply_unary validates");
    let mut general = lattice.clone();
    reset_scalar_work();
    general
        .normalize_equalities_general()
        .expect("general normalization");
    assert_eq!(
        scalar_work(),
        (2, 2),
        "the old path meets and revalidates both scalars"
    );
    assert_eq!(lattice, general);
}

#[test]
fn binary_and_operation_relations_keep_general_normalization_without_equality_edges() {
    let _meets = SCALAR_MEETS.scope();
    let _validations = SCALAR_VALIDATIONS.scope();
    let a = Symbol::THIS;
    let b = Symbol::new(1);
    for fact in [
        Fact::Binary(BinaryFact::new(
            Term::Symbol(a),
            BinaryPredicate::Ne,
            Term::Symbol(b),
        )),
        Fact::Binary(BinaryFact::new(
            Term::Symbol(a),
            BinaryPredicate::Ule,
            Term::Symbol(b),
        )),
        Fact::Relation(RelationalFact::Operation(
            OperationRelation::new(0x15, b, vec![Term::Symbol(a)]).expect("iszero"),
        )),
    ] {
        let mut lattice = FactLattice::new(8);
        lattice
            .insert(unary(UnaryPredicate::NonZero))
            .expect("nonzero");
        reset_scalar_work();
        assert_eq!(lattice.insert(fact.clone()), Ok(FactChange::Strengthened));
        assert_eq!(scalar_work(), (1, 1), "relations retain the general path");
        assert!(lattice.facts().contains(&fact));
        let mut general = lattice.clone();
        general
            .normalize_equalities_general()
            .expect("general normalization");
        assert_eq!(lattice, general);
    }
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
fn fact_candidates_use_atom_capacity_instead_of_stored_domain_capacity() {
    let mut open = FactLattice::new(1);
    open.insert(unary(UnaryPredicate::NonZero))
        .expect("one atom without finite candidates");
    assert_eq!(open.scalar(Symbol::THIS).expect("scalar").finite(), None);
    assert_eq!(open.atoms(), 1, "finite Top costs no fact atoms");

    let candidates: BTreeSet<_> = (0u64..9).map(U256::from).collect();
    assert!(candidates.len() > crate::domain::DomainSpec::default().capacity());
    let mut lattice = FactLattice::new(9);
    assert_eq!(lattice.atoms(), 0, "unconstrained components cost no atoms");
    lattice
        .insert(unary(UnaryPredicate::MemberOf(
            FiniteSet::new(candidates.clone()).expect("nonempty"),
        )))
        .expect("nine candidates fit the fact capacity");
    assert_eq!(lattice.atoms(), candidates.len());
    assert_eq!(
        lattice.scalar(Symbol::THIS).expect("scalar").finite(),
        Some(&candidates)
    );
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(Fact::Unary(UnaryFact::new(
            Symbol::new(1),
            UnaryPredicate::Exact(U256::from(10)),
        ))),
        Err(FactError::Capacity {
            max_atoms: 9,
            required: 10,
        })
    );
    assert_eq!(lattice, before);
}

#[test]
fn finite_membership_keeps_the_complete_native_candidate_payload() {
    let candidates = BTreeSet::from([U256::from(1), U256::from(2)]);
    let predicate = UnaryPredicate::MemberOf(FiniteSet::new(candidates.clone()).expect("nonempty"));
    let expected = serde_json::json!({
        "MemberOf": {"constants": serde_json::to_value(&candidates).expect("candidate array")},
    });
    assert_eq!(
        serde_json::to_value(&predicate).expect("serialized membership"),
        expected
    );
    let mut lattice = FactLattice::new(2);
    lattice.insert(unary(predicate)).expect("membership");
    let facts = lattice.facts();
    let [Fact::Unary(fact)] = facts.as_slice() else {
        panic!("one complete membership fact must be exported");
    };
    assert_eq!(
        serde_json::to_value(&fact.predicate).expect("exported membership"),
        expected
    );
}

#[test]
fn empty_finite_meet_and_filter_return_contradiction_and_preserve_the_table() {
    let mut lattice = FactLattice::new(16);
    lattice
        .insert(unary(UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::from(1), U256::from(3)])).expect("nonempty"),
        )))
        .expect("initial membership");
    let before = lattice.clone();
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::MemberOf(
            FiniteSet::new(BTreeSet::from([U256::from(2), U256::from(4)])).expect("nonempty"),
        ))),
        Err(FactError::Contradiction {
            subject: Symbol::THIS,
        })
    );
    assert_eq!(lattice, before);
    assert_eq!(
        lattice.insert(unary(UnaryPredicate::BitClear(
            BitIndex::new(0).expect("valid bit"),
        ))),
        Err(FactError::Contradiction {
            subject: Symbol::THIS,
        })
    );
    assert_eq!(lattice, before);
}

#[test]
fn excluding_the_last_candidate_preserves_typed_contradiction_in_both_orders() {
    let excluded = U256::from(7);
    let exact = unary(UnaryPredicate::Exact(excluded));
    let different = Fact::Binary(BinaryFact::new(
        Term::Symbol(Symbol::THIS),
        BinaryPredicate::Ne,
        Term::Constant(excluded),
    ));
    for (first, second) in [(exact.clone(), different.clone()), (different, exact)] {
        let mut lattice = FactLattice::new(16);
        lattice.insert(first).expect("first fact is consistent");
        let before = lattice.clone();
        assert_eq!(
            lattice.insert(second),
            Err(FactError::Contradiction {
                subject: Symbol::THIS,
            })
        );
        assert_eq!(lattice, before);
    }
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
fn scalar_meet_checks_origin_conflict_before_empty_finite_validation() {
    let scalar = |value, origin| {
        let mut lattice = FactLattice::new(16);
        lattice
            .insert(unary(UnaryPredicate::Exact(U256::from(value))))
            .expect("exact value");
        lattice
            .insert(unary(UnaryPredicate::PossibleOrigins(OriginSet::source(
                origin,
            ))))
            .expect("known source");
        lattice.scalar(Symbol::THIS).expect("scalar").clone()
    };
    let mut left = scalar(1u64, Origin::Calldata);
    let right = scalar(2u64, Origin::Storage);
    assert_eq!(
        left.meet(&right, Symbol::THIS),
        Err(FactError::OriginContradiction {
            subject: Symbol::THIS,
        })
    );

    let mut left = scalar(1u64, Origin::Calldata);
    let right = scalar(2u64, Origin::Calldata);
    assert_eq!(
        left.meet(&right, Symbol::THIS),
        Err(FactError::Contradiction {
            subject: Symbol::THIS,
        })
    );
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
    assert_eq!(lattice, before);
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
    let relation = Fact::Relation(RelationalFact::Operation(
        OperationRelation::new(0x15, Symbol::new(3), vec![Term::Symbol(b)]).expect("iszero"),
    ));
    lattice
        .insert(relation.clone())
        .expect("operation relation");
    lattice
        .insert(Fact::Unary(UnaryFact::new(
            b,
            UnaryPredicate::Exact(U256::from(3)),
        )))
        .expect("numeric alias propagation");
    for symbol in [a, b] {
        assert_eq!(
            lattice.scalar(symbol).expect("alias").finite(),
            Some(&BTreeSet::from([U256::from(3)]))
        );
    }
    assert_eq!(
        lattice.scalar(a).expect("a").possible_origins(),
        Some(&OriginSet::source(Origin::Calldata))
    );
    assert_eq!(
        lattice.scalar(b).expect("b").possible_origins(),
        Some(&OriginSet::source(Origin::Storage))
    );
    assert!(lattice.scalar(a).expect("a").is_code_address());
    assert!(!lattice.scalar(b).expect("b").is_code_address());
    assert!(lattice.facts().contains(&relation));
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
    let value = crate::domain::ValueAlgebra::new(crate::domain::ProductAlgebra::default())
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

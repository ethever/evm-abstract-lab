//! Finite observed cells retain the intersection of all numeric components.

use super::{Encoder, word};
use crate::domain::{
    AbstractValue, Domain, FiniteConstantSet, NumericValue,
    congruence::Congruence,
    interval::Interval,
    known_bits::KnownBits,
    relational::{CheckResult, RelationLimits, RelationState, SmtProvider, solver::checked},
    symbolic::ExprId,
};
use alloy_primitives::U256;
use embedded_smt::{Bool, Context};

#[test]
fn finite_cell_membership_filters_masks_bounds_congruences_and_nonzero() {
    let mut numeric = NumericValue::top();
    numeric.finite = FiniteConstantSet::try_from_values(
        [0, 1, 2, 3, 4, 6].map(U256::from).into_iter().collect(),
    )
    .unwrap();
    numeric.bits = KnownBits::new(U256::ZERO, U256::from(2)).unwrap();
    numeric.interval = Interval::new_unsigned(U256::ZERO, U256::from(4)).unwrap();
    numeric.congruence = Congruence::new(U256::from(2), U256::ZERO).unwrap();
    numeric.nonzero = true;
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        let symbols = Context::default();
        let value = symbols.fresh_bv("observed_cell", 256);
        let mut encoder = Encoder::new(&limits);
        let mut facts = encoder.numeric(&value, &numeric).unwrap();
        assert_eq!(checked(&facts, &limits), CheckResult::Sat);
        facts.push(value.eq(word(U256::from(2))).not());
        assert_eq!(checked(&facts, &limits), CheckResult::Unsat);
        numeric.bits = KnownBits::new(U256::from(2), U256::ZERO).unwrap();
        numeric.nonzero = true;
        // The remaining candidates are zero and four; nonzero removes zero.
        let mut facts = encoder.numeric(&value, &numeric).unwrap();
        facts.push(value.eq(word(U256::from(4))).not());
        assert_eq!(checked(&facts, &limits), CheckResult::Unsat);
        numeric.bits = KnownBits::new(U256::ZERO, U256::from(8)).unwrap();
        let facts = encoder.numeric(&value, &numeric).unwrap();
        assert_eq!(checked(&facts, &limits), CheckResult::Unsat);
        numeric.bits = KnownBits::new(U256::ZERO, U256::from(2)).unwrap();
    }
}

#[test]
fn finite_cell_with_large_stride_keeps_both_values_and_excludes_every_other_word() {
    let a = U256::from(0x101);
    let b = (U256::ONE << 200) + U256::from(0x333);
    let domain = Domain::default();
    let numeric = domain.join(&AbstractValue::constant(a), &AbstractValue::constant(b));
    assert_eq!(numeric.constants().unwrap().len(), 2);
    assert!(numeric.numeric().congruence().modulus_residue().unwrap().0 > U256::from(1));
    for provider in [SmtProvider::Z3, SmtProvider::Bitwuzla, SmtProvider::Cvc5] {
        let limits = RelationLimits {
            provider,
            ..RelationLimits::default()
        };
        let symbols = Context::default();
        let value = symbols.fresh_bv("observed_cell", 256);
        let mut encoder = Encoder::new(&limits);
        let facts = encoder.numeric(&value, numeric.numeric()).unwrap();
        for candidate in [a, b] {
            let mut selected = facts.clone();
            selected.push(value.eq(word(candidate)));
            assert_eq!(checked(&selected, &limits), CheckResult::Sat);
        }
        let mut outside = facts;
        outside.push(Bool::or(&[value.eq(word(a)), value.eq(word(b))]).not());
        assert_eq!(checked(&outside, &limits), CheckResult::Unsat);

        // The path observer uses the same complete-set contract. One retained
        // fact must suffice even for words with a large derived congruence.
        let path_limits = RelationLimits {
            max_constraints: 1,
            ..limits
        };
        let expression = ExprId::fresh().unwrap();
        let mut relations = RelationState::default();
        relations
            .observe(&expression, numeric.numeric(), &path_limits)
            .unwrap();
        for candidate in [a, b] {
            assert_eq!(
                relations.can_equal(&expression, candidate, &path_limits),
                CheckResult::Sat
            );
        }
        assert_eq!(
            relations.can_equal(&expression, U256::ZERO, &path_limits),
            CheckResult::Unsat
        );
    }
}

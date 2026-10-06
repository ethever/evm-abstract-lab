use super::Interval;
use alloy_primitives::U256;
use proptest::prelude::*;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

fn u(value: u64) -> U256 {
    U256::from(value)
}

#[test]
fn unsigned_and_signed_bounds_keep_both_halves() {
    let sign = u(1) << 255;
    let signed = Interval::new_signed(sign - u(2), sign + u(2)).unwrap();
    assert_eq!(
        signed.segments(),
        vec![(U256::ZERO, u(2)), (U256::MAX - u(1), U256::MAX)]
    );
    let only_positive = signed
        .meet(&Interval::new_unsigned(u(1), u(10)).unwrap())
        .unwrap();
    assert_eq!(only_positive.unsigned_bounds(), (u(1), u(2)));
    assert_eq!(only_positive.signed_bounds(), (sign + u(1), sign + u(2)));
    assert!(
        signed
            .meet(&Interval::new_unsigned(u(3), u(10)).unwrap())
            .is_none()
    );
}

#[test]
fn wrap_is_not_integer_interval_arithmetic() {
    let edge = Interval::new_unsigned(U256::MAX - u(1), U256::MAX).unwrap();
    let result = Interval::transfer(opcode::ADD, &[edge, Interval::exact(u(1))]);
    assert!(result.contains(U256::MAX));
    assert!(result.contains(U256::ZERO));
    let result = Interval::transfer(opcode::SUB, &[Interval::exact(U256::ZERO), edge]);
    assert!(result.contains(u(1)));
    assert!(result.contains(u(2)));
    let result = Interval::transfer(opcode::MUL, &[edge, Interval::exact(u(2))]);
    assert!(result.contains(U256::MAX - u(1)));
    assert!(result.contains(U256::MAX - u(3)));
}

#[test]
fn division_zero_modulus_and_shift_edges_are_evm_semantics() {
    let input = Interval::new_unsigned(u(4), u(16)).unwrap();
    assert_eq!(
        Interval::transfer(opcode::DIV, &[input, Interval::exact(U256::ZERO)]),
        Interval::exact(U256::ZERO)
    );
    assert_eq!(
        Interval::transfer(opcode::ADDMOD, &[input, input, Interval::exact(U256::ZERO)]),
        Interval::exact(U256::ZERO)
    );
    let shifted = Interval::transfer(
        opcode::SHR,
        &[
            Interval::new_unsigned(u(255), u(300)).unwrap(),
            Interval::top(),
        ],
    );
    assert_eq!(shifted.unsigned_bounds(), (U256::ZERO, u(1)));
    assert_eq!(
        Interval::transfer(opcode::SHL, &[Interval::exact(u(256)), input]),
        Interval::exact(U256::ZERO)
    );
    let signed_min = u(1) << 255;
    assert_eq!(
        Interval::transfer(
            opcode::SDIV,
            &[Interval::exact(signed_min), Interval::exact(U256::MAX)]
        ),
        Interval::exact(signed_min)
    );
}

#[test]
fn signed_comparisons_and_arithmetic_shift_use_signed_order() {
    let negatives = Interval::new_unsigned(U256::MAX - u(4), U256::MAX).unwrap();
    let positives = Interval::new_unsigned(u(2), u(10)).unwrap();
    assert_eq!(
        Interval::transfer(opcode::SLT, &[negatives, positives]),
        Interval::exact(u(1))
    );
    assert_eq!(
        Interval::transfer(opcode::LT, &[negatives, positives]),
        Interval::exact(U256::ZERO)
    );
    let result = Interval::transfer(
        opcode::SAR,
        &[Interval::new_unsigned(u(1), u(300)).unwrap(), negatives],
    );
    assert_eq!(result.unsigned_bounds(), (U256::MAX - u(2), U256::MAX));
}

#[test]
fn boolean_byte_and_clz_have_bounded_results() {
    assert_eq!(
        Interval::transfer(opcode::EQ, &[Interval::top(), Interval::top()]).unsigned_bounds(),
        (U256::ZERO, u(1))
    );
    assert_eq!(
        Interval::transfer(opcode::BYTE, &[Interval::top(), Interval::top()]).unsigned_bounds(),
        (U256::ZERO, u(255))
    );
    assert_eq!(
        Interval::transfer(opcode::CLZ, &[Interval::top()]).unsigned_bounds(),
        (U256::ZERO, u(256))
    );
    assert_eq!(
        Interval::transfer(
            opcode::ISZERO,
            &[Interval::new_unsigned(u(1), u(99)).unwrap()]
        ),
        Interval::exact(U256::ZERO)
    );
}

proptest! {
    #[test]
    fn full_width_signed_and_wrapping_transfers_cover_sampled_members(
        av in any::<[u64; 4]>(), bv in any::<[u64; 4]>(),
        cv in any::<[u64; 4]>(), dv in any::<[u64; 4]>(),
    ) {
        let av = U256::from_limbs(av);
        let bv = U256::from_limbs(bv);
        let cv = U256::from_limbs(cv);
        let dv = U256::from_limbs(dv);
        let a = Interval::from_values(&BTreeSet::from([av, bv]));
        let b = Interval::from_values(&BTreeSet::from([cv, dv]));
        for op in [opcode::ADD, opcode::SUB, opcode::MUL, opcode::DIV, opcode::MOD,
            opcode::SDIV, opcode::SMOD, opcode::LT, opcode::GT, opcode::SLT,
            opcode::SGT, opcode::EQ, opcode::AND, opcode::OR, opcode::XOR,
            opcode::BYTE, opcode::SHL, opcode::SHR, opcode::SAR, opcode::SIGNEXTEND] {
            let result = Interval::transfer(op, &[a, b]);
            for x in [av, bv] {
                for y in [cv, dv] {
                    let concrete = super::super::evaluate(op, x, y, U256::ZERO);
                    prop_assert!(result.contains(concrete), "op={op:#x}, a={x}, b={y}, result={result:?}");
                }
            }
        }
    }

    #[test]
    fn join_obeys_laws_for_words_across_sign_boundary(
        a in any::<u64>(), b in any::<u64>(), c in any::<u64>(),
        an in any::<bool>(), bn in any::<bool>(), cn in any::<bool>(),
    ) {
        let embed = |x: u64, negative: bool| if negative { U256::MAX - u(x) } else { u(x) };
        let a = Interval::exact(embed(a, an));
        let b = Interval::exact(embed(b, bn));
        let c = Interval::exact(embed(c, cn));
        prop_assert_eq!(a.join(&a), a);
        prop_assert_eq!(a.join(&b), b.join(&a));
        prop_assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
        prop_assert!(a.join(&b).contains(a.singleton().unwrap()));
        prop_assert!(a.join(&b).contains(b.singleton().unwrap()));
    }

    #[test]
    fn low_width_transfers_cover_all_concrete_pairs(
        alo in 0_u64..16, awidth in 0_u64..8,
        blo in 0_u64..16, bwidth in 0_u64..8,
    ) {
        let a = Interval::new_unsigned(u(alo), u(alo + awidth)).unwrap();
        let b = Interval::new_unsigned(u(blo), u(blo + bwidth)).unwrap();
        for op in [opcode::ADD, opcode::SUB, opcode::MUL, opcode::DIV, opcode::MOD,
            opcode::LT, opcode::GT, opcode::SLT, opcode::SGT, opcode::EQ,
            opcode::AND, opcode::OR, opcode::XOR, opcode::BYTE, opcode::SHL,
            opcode::SHR, opcode::SAR, opcode::SIGNEXTEND] {
            let result = Interval::transfer(op, &[a, b]);
            for av in alo..=alo + awidth {
                for bv in blo..=blo + bwidth {
                    let concrete = super::super::evaluate(op, u(av), u(bv), U256::ZERO);
                    prop_assert!(result.contains(concrete), "op={op:#x}, a={av}, b={bv}, result={result:?}");
                }
            }
        }
    }

    #[test]
    fn meet_matches_concrete_membership_in_small_universe(
        alo in 0_u64..32, ahi in 0_u64..32,
        blo in 0_u64..32, bhi in 0_u64..32,
    ) {
        let a = Interval::new_unsigned(u(alo.min(ahi)), u(alo.max(ahi))).unwrap();
        let b = Interval::new_unsigned(u(blo.min(bhi)), u(blo.max(bhi))).unwrap();
        let meet = a.meet(&b);
        for value in 0..32 {
            prop_assert_eq!(meet.as_ref().is_some_and(|m| m.contains(u(value))), a.contains(u(value)) && b.contains(u(value)));
        }
    }
}

#[test]
fn finite_hulls_and_normalization_are_canonical() {
    let sign = u(1) << 255;
    let values = BTreeSet::from([u(4), u(8), U256::MAX - u(1), U256::MAX]);
    let interval = Interval::from_values(&values);
    for value in values {
        assert!(interval.contains(value));
    }
    assert_eq!(interval.meet(&Interval::top()), Some(interval));
    assert!(Interval::new_unsigned(u(2), u(1)).is_none());
    assert!(Interval::new_signed(sign + u(2), sign + u(1)).is_none());
}

#[test]
fn widening_stabilizes_unsigned_counter_without_changing_join() {
    let initial = Interval::exact(U256::ZERO);
    let next = initial.join(&Interval::exact(u(1)));
    assert_eq!(next.unsigned_bounds(), (U256::ZERO, u(1)));
    let widened = initial.widen(&next);
    assert!(widened.contains(U256::ZERO));
    assert!(widened.contains(u(1)));
    assert_eq!(
        widened.widen(&widened.join(&Interval::exact(u(2)))),
        widened
    );
    // 后续跨过符号边界时仍扩大，不能把正数范围当作永久保证。
    let includes_negative = widened.widen(&widened.join(&Interval::exact(U256::MAX)));
    assert!(includes_negative.is_top());
    assert_eq!(includes_negative.widen(&Interval::top()), includes_negative);
}

proptest! {
    #[test]
    fn widening_covers_both_operands(
        a in any::<[u64; 4]>(), b in any::<[u64; 4]>(),
        c in any::<[u64; 4]>(), d in any::<[u64; 4]>(),
    ) {
        let a = U256::from_limbs(a);
        let b = U256::from_limbs(b);
        let c = U256::from_limbs(c);
        let d = U256::from_limbs(d);
        let old = Interval::from_values(&BTreeSet::from([a, b]));
        let next = Interval::from_values(&BTreeSet::from([c, d]));
        let widened = old.widen(&next);
        for member in [a, b, c, d] {
            prop_assert!(widened.contains(member));
        }
        prop_assert_eq!(old.widen(&old), old);
    }
}

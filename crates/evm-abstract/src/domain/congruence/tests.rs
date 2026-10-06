use super::Congruence;
use alloy_primitives::U256;
use proptest::prelude::*;
use revm_bytecode::opcode;
use std::collections::BTreeSet;

fn u(value: u64) -> U256 {
    U256::from(value)
}

#[test]
fn general_moduli_keep_odd_divisibility() {
    let multiples_of_three = Congruence::new(u(3), U256::ZERO).unwrap();
    assert!(multiples_of_three.contains(u(9)));
    assert!(!multiples_of_three.contains(u(8)));
    assert_eq!(
        Congruence::new(u(3), u(8)).unwrap().modulus_residue(),
        Some((u(3), u(2)))
    );
    assert!(Congruence::new(U256::ZERO, u(4)).is_none());
    assert!(Congruence::new(u(1), u(4)).unwrap().is_top());
    assert_eq!(
        Congruence::new(U256::MAX, u(1)).unwrap().singleton(),
        Some(u(1))
    );
}

#[test]
fn crt_handles_coprime_and_incompatible_moduli() {
    let left = Congruence::new(u(4), u(1)).unwrap();
    let right = Congruence::new(u(6), u(3)).unwrap();
    assert_eq!(
        left.meet(&right).unwrap().modulus_residue(),
        Some((u(12), u(9)))
    );
    assert!(left.meet(&Congruence::new(u(6), u(2)).unwrap()).is_none());
    assert_eq!(
        left.meet(&Congruence::exact(u(9))),
        Some(Congruence::exact(u(9)))
    );
    assert!(left.meet(&Congruence::exact(u(10))).is_none());
}

#[test]
fn crt_lcm_larger_than_word_can_be_singleton_or_empty() {
    let large = (u(1) << 200) + u(1);
    let other = (u(1) << 200) + u(3);
    let left = Congruence::new(large, u(17)).unwrap();
    let right = Congruence::new(other, u(17)).unwrap();
    assert_eq!(left.meet(&right), Some(Congruence::exact(u(17))));
    assert_eq!(right.meet(&left), Some(Congruence::exact(u(17))));
    // 解为约 2^399，超出 W；不能截断成低 256 bit 的假单点。
    let different = Congruence::new(other, u(18)).unwrap();
    assert!(left.meet(&different).is_none());
}

#[test]
fn lcm_beyond_word_size_is_not_truncated() {
    let half = u(1) << 255;
    let left = Congruence::new(half, U256::ZERO).unwrap();
    let right = Congruence::new(u(3), U256::ZERO).unwrap();
    assert_eq!(left.meet(&right), Some(Congruence::exact(U256::ZERO)));
    assert_eq!(
        left.meet(&Congruence::new(u(3), u(2)).unwrap()),
        Some(Congruence::exact(half))
    );
}

#[test]
fn word_wrap_discards_false_odd_modulus_but_preserves_low_bits() {
    let odd = Congruence::new(u(3), U256::ZERO).unwrap();
    let one = Congruence::exact(u(1));
    let result = Congruence::transfer(opcode::ADD, &[odd.clone(), one]);
    assert!(result.contains(U256::ZERO)); // MAX 是 3 的倍数；MAX+1 回到零。
    assert!(result.is_top());
    let even = Congruence::new(u(8), u(6)).unwrap();
    let result = Congruence::transfer(opcode::ADD, &[even, Congruence::exact(u(3))]);
    assert_eq!(result.modulus_residue(), Some((u(8), u(1))));
    assert!(result.contains(u(1)));
    let result = Congruence::transfer(opcode::MUL, &[odd, Congruence::exact(u(2))]);
    assert_eq!(result.modulus_residue(), Some((u(2), U256::ZERO)));
}

#[test]
fn explicit_modular_operations_keep_general_divisibility_without_word_wrap() {
    let multiples_of_three = Congruence::new(u(3), U256::ZERO).unwrap();
    let one_mod_three = Congruence::new(u(3), u(1)).unwrap();
    assert_eq!(
        Congruence::transfer(
            opcode::ADDMOD,
            &[
                multiples_of_three.clone(),
                one_mod_three,
                Congruence::exact(u(6))
            ]
        )
        .modulus_residue(),
        Some((u(3), u(1)))
    );
    assert_eq!(
        Congruence::transfer(
            opcode::MULMOD,
            &[
                multiples_of_three.clone(),
                Congruence::top(),
                Congruence::exact(u(3))
            ]
        ),
        Congruence::exact(U256::ZERO)
    );
    for op in [opcode::SHL, opcode::SHR, opcode::SAR] {
        assert_eq!(
            Congruence::transfer(
                op,
                &[Congruence::exact(U256::ZERO), multiples_of_three.clone()]
            ),
            multiples_of_three
        );
    }
}

#[test]
fn arithmetic_and_shift_identities_have_exact_evm_edges() {
    let class = Congruence::new(u(8), u(6)).unwrap();
    assert_eq!(
        Congruence::transfer(opcode::SHR, &[Congruence::exact(u(1)), class.clone()])
            .modulus_residue(),
        Some((u(4), u(3)))
    );
    assert_eq!(
        Congruence::transfer(opcode::SHL, &[Congruence::exact(u(2)), class.clone()])
            .modulus_residue(),
        Some((u(32), u(24)))
    );
    assert_eq!(
        Congruence::transfer(opcode::MOD, &[class.clone(), Congruence::exact(u(4))]),
        Congruence::exact(u(2))
    );
    assert_eq!(
        Congruence::transfer(opcode::DIV, &[class.clone(), Congruence::exact(U256::ZERO)]),
        Congruence::exact(U256::ZERO)
    );
    assert_eq!(
        Congruence::transfer(opcode::SHL, &[Congruence::exact(u(300)), class]),
        Congruence::exact(U256::ZERO)
    );
    let min = u(1) << 255;
    assert_eq!(
        Congruence::transfer(
            opcode::SDIV,
            &[Congruence::exact(min), Congruence::exact(U256::MAX)]
        ),
        Congruence::exact(min)
    );
}

#[test]
fn bounded_alignment_handles_upper_edge_without_wrapping() {
    let class = Congruence::new(u(8), u(3)).unwrap();
    assert_eq!(class.first_last(u(4), u(20)), Some((u(11), u(19))));
    assert!(class.first_last(u(4), u(10)).is_none());
    assert_eq!(
        class.first_last(U256::MAX - u(4), U256::MAX),
        Some((U256::MAX - u(4), U256::MAX - u(4)))
    );
    assert!(class.first_last(U256::MAX - u(3), U256::MAX).is_none());
}

proptest! {
    #[test]
    fn full_width_modulus_joins_obey_associativity(
        am in any::<[u64; 4]>(), bm in any::<[u64; 4]>(), cm in any::<[u64; 4]>(),
        ar in any::<[u64; 4]>(), br in any::<[u64; 4]>(), cr in any::<[u64; 4]>(),
    ) {
        let a = Congruence::new(U256::from_limbs(am).max(u(1)), U256::from_limbs(ar)).unwrap();
        let b = Congruence::new(U256::from_limbs(bm).max(u(1)), U256::from_limbs(br)).unwrap();
        let c = Congruence::new(U256::from_limbs(cm).max(u(1)), U256::from_limbs(cr)).unwrap();
        prop_assert_eq!(a.join(&a), a.clone());
        prop_assert_eq!(a.join(&b), b.join(&a));
        prop_assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
    }

    #[test]
    fn gcd_join_obeys_laws(a in 1_u64..48, b in 1_u64..48, c in 1_u64..48,
        ar in 0_u64..48, br in 0_u64..48, cr in 0_u64..48) {
        let a = Congruence::new(u(a), u(ar)).unwrap();
        let b = Congruence::new(u(b), u(br)).unwrap();
        let c = Congruence::new(u(c), u(cr)).unwrap();
        prop_assert_eq!(a.join(&a), a.clone());
        prop_assert_eq!(a.join(&b), b.join(&a));
        prop_assert_eq!(a.join(&b).join(&c), a.join(&b.join(&c)));
        let joined = a.join(&b);
        for value in 0..96 {
            prop_assert!(!(a.contains(u(value)) || b.contains(u(value))) || joined.contains(u(value)));
        }
    }

    #[test]
    fn crt_meet_matches_membership(a in 1_u64..32, b in 1_u64..32,
        ar in 0_u64..32, br in 0_u64..32) {
        let a = Congruence::new(u(a), u(ar)).unwrap();
        let b = Congruence::new(u(b), u(br)).unwrap();
        let meet = a.meet(&b);
        prop_assert_eq!(a.meet(&b), b.meet(&a));
        for value in 0..256 {
            prop_assert_eq!(meet.as_ref().is_some_and(|m| m.contains(u(value))), a.contains(u(value)) && b.contains(u(value)));
        }
    }

    #[test]
    fn wrapping_transfers_cover_low_and_high_word_members(
        am in 1_u64..32, bm in 1_u64..32,
        ar in 0_u64..32, br in 0_u64..32,
    ) {
        let a = Congruence::new(u(am), u(ar)).unwrap();
        let b = Congruence::new(u(bm), u(br)).unwrap();
        let low = (0..64).map(u);
        let high = (0..64).map(|x| U256::MAX - u(x));
        let a_values = low.chain(high).filter(|x| a.contains(*x)).collect::<Vec<_>>();
        let low = (0..64).map(u);
        let high = (0..64).map(|x| U256::MAX - u(x));
        let b_values = low.chain(high).filter(|x| b.contains(*x)).collect::<Vec<_>>();
        for op in [opcode::ADD, opcode::SUB, opcode::MUL, opcode::DIV, opcode::MOD, opcode::EQ, opcode::AND, opcode::OR, opcode::XOR] {
            let result = Congruence::transfer(op, &[a.clone(), b.clone()]);
            for av in &a_values {
                for bv in &b_values {
                    let concrete = super::super::evaluate(op, *av, *bv, U256::ZERO);
                    prop_assert!(result.contains(concrete), "op={op:#x}, a={av}, b={bv}, result={result:?}");
                }
            }
        }
        for modulus in [u(3), u(6), u(17)] {
            for op in [opcode::ADDMOD, opcode::MULMOD] {
                let result = Congruence::transfer(op, &[a.clone(), b.clone(), Congruence::exact(modulus)]);
                for av in &a_values {
                    for bv in &b_values {
                        let concrete = super::super::evaluate(op, *av, *bv, modulus);
                        prop_assert!(result.contains(concrete), "op={op:#x}, a={av}, b={bv}, modulus={modulus}, result={result:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn finite_set_hull_uses_spacing_not_only_power_of_two() {
    let class = Congruence::from_values(&BTreeSet::from([u(3), u(9), u(15)]));
    assert_eq!(class.modulus_residue(), Some((u(6), u(3))));
}

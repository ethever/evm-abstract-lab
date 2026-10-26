//! Independent concrete members are retained before building abstract inputs.
//! Existing fast scalar-helper/lattice laws remain complementary checks.
#[path = "support/arithmetic.rs"]
mod arithmetic;
#[path = "support/native.rs"]
mod native;

use alloy_primitives::U256;
use evm_abstract::{
    Fork,
    domain::{
        EvmValue, NumericValue,
        interval::Interval,
        known_bits::KnownBits,
        relational::{EvmValueQuery, RelationLimits, RelationState},
        symbolic::ExprId,
    },
};
use proptest::{arbitrary::any, proptest};
use revm_bytecode::opcode;
use std::collections::BTreeSet;

fn check_members(op: u8, members: &[Vec<U256>], fork: Fork) {
    let sets: Vec<BTreeSet<_>> = members
        .iter()
        .map(|values| values.iter().copied().collect())
        .collect();
    let bits: Vec<_> = sets.iter().map(KnownBits::from_values).collect();
    let intervals: Vec<_> = sets.iter().map(Interval::from_values).collect();
    let algebra = native::product();
    let abstract_args: Vec<_> = members
        .iter()
        .map(|values| {
            values
                .iter()
                .map(|value| EvmValue::<NumericValue>::constant(*value))
                .reduce(|left, right| algebra.join(&left, &right))
                .unwrap()
        })
        .collect();
    let actual_bits = KnownBits::transfer(op, &bits);
    let actual_interval = Interval::transfer(op, &intervals);
    let actual_product = algebra.apply(op, &abstract_args);
    // Enumerate the saved members, never filter them through contains/join.
    let mut tuples = vec![Vec::new()];
    for values in members {
        tuples = tuples
            .into_iter()
            .flat_map(|prefix| {
                values.iter().map(move |value| {
                    let mut tuple = prefix.clone();
                    tuple.push(*value);
                    tuple
                })
            })
            .collect();
    }
    for args in tuples {
        let expected = arithmetic::evaluate(op, &args, fork);
        assert!(
            actual_bits.contains(expected),
            "KnownBits {fork:?} op={op:#x} args={args:?} expected={expected:#x}"
        );
        assert!(
            actual_interval.contains(expected),
            "Interval {fork:?} op={op:#x} args={args:?} expected={expected:#x}"
        );
        assert!(
            actual_product.contains(expected),
            "Product {fork:?} op={op:#x} args={args:?} expected={expected:#x}"
        );
        // The relational scalar path is also checked against upstream execution.
        let limits = RelationLimits::default();
        let mut relations = RelationState::default();
        let inputs: Vec<_> = args
            .iter()
            .map(|value| {
                let input = ExprId::fresh().unwrap();
                relations.add_unsigned_bounds(&input, *value, *value, &limits, algebra.borrowed());
                input
            })
            .collect();
        let expression = ExprId::operation(op, &inputs, limits.expressions()).unwrap();
        assert_eq!(
            relations.local_value(&expression, &limits, algebra.borrowed()),
            EvmValueQuery::Exact(expected),
            "Relations {fork:?} op={op:#x} args={args:?}"
        );
    }
}

#[test]
fn multiple_members_cover_wrap_signed_division_zero_large_shifts_and_fork_clz() {
    let sign = U256::from(1) << 255;
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        for op in [
            opcode::ADD,
            opcode::SUB,
            opcode::MUL,
            opcode::SDIV,
            opcode::SMOD,
            opcode::DIV,
            opcode::MOD,
            opcode::SHL,
            opcode::SHR,
            opcode::SAR,
            opcode::BYTE,
            opcode::SIGNEXTEND,
            opcode::XOR,
            opcode::LT,
            opcode::SLT,
        ] {
            check_members(
                op,
                &[
                    vec![U256::ZERO, U256::from(256), U256::MAX],
                    vec![U256::ZERO, U256::from(1), sign],
                ],
                fork,
            );
        }
        for op in [opcode::ADDMOD, opcode::MULMOD] {
            check_members(
                op,
                &[
                    vec![U256::MAX, sign],
                    vec![U256::from(2), U256::MAX],
                    vec![U256::ZERO, U256::from(17)],
                ],
                fork,
            );
        }
        if fork == Fork::Osaka {
            check_members(
                opcode::CLZ,
                &[vec![U256::ZERO, U256::from(1), sign, U256::MAX]],
                fork,
            );
        }
    }
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(16))]
    #[test]
    fn saved_full_width_member_pairs_are_included(
        a in any::<[u64; 4]>(), b in any::<[u64; 4]>(),
        c in any::<[u64; 4]>(), d in any::<[u64; 4]>(), selected in 0usize..8,
    ) {
        let op = [opcode::ADD, opcode::SUB, opcode::MUL, opcode::SDIV, opcode::XOR, opcode::SHL, opcode::SHR, opcode::SAR][selected];
        check_members(op, &[vec![U256::from_limbs(a), U256::from_limbs(b)], vec![U256::from_limbs(c), U256::from_limbs(d)]], Fork::Osaka);
    }
}

//! Independent typed expression evaluator shared by normalized test machines.
use alloy_primitives::U512;
use evm_decompile::{
    EntityId, LiftedExpressionKind as Expr, LiftedProgram, LiftedRelation as Relation, LiftedType,
    LiftedValue, U256,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn word(value: &LiftedValue) -> U256 {
    let LiftedValue::Word(word) = value else {
        panic!("predicate used as a numeric word without an explicit cast: {value:?}");
    };
    *word
}

pub(super) fn predicate(value: &LiftedValue) -> bool {
    let LiftedValue::Predicate(predicate) = value else {
        panic!("numeric word used as a typed predicate: {value:?}");
    };
    *predicate
}

pub(crate) fn signed_less(left: U256, right: U256) -> bool {
    if left.bit(255) == right.bit(255) {
        left < right
    } else {
        left.bit(255)
    }
}

fn relation(relation: Relation, left: U256, right: U256) -> bool {
    match relation {
        Relation::Eq => left == right,
        Relation::Ne => left != right,
        Relation::Ult => left < right,
        Relation::Ule => left <= right,
        Relation::Ugt => left > right,
        Relation::Uge => left >= right,
        Relation::Slt => signed_less(left, right),
        Relation::Sle => !signed_less(right, left),
        Relation::Sgt => signed_less(right, left),
        Relation::Sge => !signed_less(left, right),
    }
}

fn magnitude(value: U256) -> U256 {
    if value.bit(255) {
        (!value).wrapping_add(U256::from(1))
    } else {
        value
    }
}

fn primitive_word(opcode: u8, args: &[U256]) -> U256 {
    let first = args[0];
    let second = args.get(1).copied().unwrap_or_default();
    match opcode {
        0x01 => first.wrapping_add(second),
        0x02 => first.wrapping_mul(second),
        0x03 => first.wrapping_sub(second),
        0x04 => {
            if second == U256::ZERO {
                U256::ZERO
            } else {
                first / second
            }
        }
        0x05 => {
            if second == U256::ZERO {
                return U256::ZERO;
            }
            let result = magnitude(first) / magnitude(second);
            if first.bit(255) != second.bit(255) {
                (!result).wrapping_add(U256::from(1))
            } else {
                result
            }
        }
        0x06 => {
            if second == U256::ZERO {
                U256::ZERO
            } else {
                first % second
            }
        }
        0x07 => {
            if second == U256::ZERO {
                return U256::ZERO;
            }
            let result = magnitude(first) % magnitude(second);
            if first.bit(255) {
                (!result).wrapping_add(U256::from(1))
            } else {
                result
            }
        }
        0x08 | 0x09 => {
            if args[2] == U256::ZERO {
                return U256::ZERO;
            }
            let left = U512::from(first);
            let right = U512::from(second);
            let wide = if opcode == 0x08 {
                left + right
            } else {
                left * right
            };
            U256::from(wide % U512::from(args[2]))
        }
        0x0a => {
            let mut result = U256::from(1);
            let mut base = first;
            let mut exponent = second;
            while exponent != U256::ZERO {
                if exponent.bit(0) {
                    result = result.wrapping_mul(base);
                }
                base = base.wrapping_mul(base);
                exponent >>= 1;
            }
            result
        }
        0x0b => {
            if first >= U256::from(31) {
                return second;
            }
            let bit = usize::try_from(first).unwrap() * 8 + 7;
            let mask = (U256::from(1) << bit) - U256::from(1);
            if second.bit(bit) {
                second | !mask
            } else {
                second & mask
            }
        }
        0x16 => first & second,
        0x17 => first | second,
        0x18 => first ^ second,
        0x19 => !first,
        0x1a => {
            if first >= U256::from(32) {
                U256::ZERO
            } else {
                U256::from(second.to_be_bytes::<32>()[usize::try_from(first).unwrap()])
            }
        }
        0x1b => {
            if first >= U256::from(256) {
                U256::ZERO
            } else {
                second << usize::try_from(first).unwrap()
            }
        }
        0x1c => {
            if first >= U256::from(256) {
                U256::ZERO
            } else {
                second >> usize::try_from(first).unwrap()
            }
        }
        0x1d => {
            let negative = second.bit(255);
            if first >= U256::from(256) {
                return if negative { U256::MAX } else { U256::ZERO };
            }
            let shift = usize::try_from(first).unwrap();
            if shift == 0 {
                return second;
            }
            let shifted = second >> shift;
            if negative {
                shifted | (U256::MAX << (256 - shift))
            } else {
                shifted
            }
        }
        0x1e => U256::from(first.leading_zeros()),
        _ => panic!("fixture needs an independent reference for WordOp 0x{opcode:02x}"),
    }
}

fn dependencies(kind: &Expr) -> Vec<EntityId> {
    match kind {
        Expr::Word(_) | Expr::Predicate(_) | Expr::Value(_) => Vec::new(),
        Expr::WordFromPredicate(id) | Expr::NonZero(id) | Expr::Not(id) => vec![*id],
        Expr::Compare { left, right, .. } => vec![*left, *right],
        Expr::WordOp { arguments, .. } => arguments.clone(),
    }
}

/// Independent flat-DAG evaluator with an explicit current dynamic block.
pub(crate) fn evaluate(
    program: &LiftedProgram,
    root: EntityId,
    block: EntityId,
    bindings: &BTreeMap<EntityId, U256>,
) -> LiftedValue {
    let mut needed = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if needed.insert(id) {
            let expression = program.expression(id).expect("expression reference");
            assert_eq!(
                expression.block, block,
                "expression crossed its dynamic block scope"
            );
            pending.extend(dependencies(&expression.kind));
        }
    }
    let mut evaluated = BTreeMap::new();
    for expression in program
        .expressions
        .iter()
        .filter(|expression| needed.contains(&expression.id))
    {
        let operand = |id: &EntityId| evaluated.get(id).expect("topologically earlier operand");
        let result = match &expression.kind {
            Expr::Word(value) => LiftedValue::Word(*value),
            Expr::Predicate(value) => LiftedValue::Predicate(*value),
            Expr::Value(id) => LiftedValue::Word(*bindings.get(id).unwrap_or_else(|| {
                panic!("high projection has no capture/binding for {id:?} in block {block:?}")
            })),
            Expr::WordFromPredicate(id) => {
                LiftedValue::Word(U256::from(u64::from(predicate(operand(id)))))
            }
            Expr::NonZero(id) => LiftedValue::Predicate(word(operand(id)) != U256::ZERO),
            Expr::Not(id) => LiftedValue::Predicate(!predicate(operand(id))),
            Expr::Compare {
                relation: kind,
                left,
                right,
            } => LiftedValue::Predicate(relation(*kind, word(operand(left)), word(operand(right)))),
            Expr::WordOp { opcode, arguments } => LiftedValue::Word(primitive_word(
                *opcode,
                &arguments
                    .iter()
                    .map(|id| word(operand(id)))
                    .collect::<Vec<_>>(),
            )),
        };
        assert!(matches!(
            (&result, expression.ty),
            (LiftedValue::Word(_), LiftedType::Word256)
                | (LiftedValue::Predicate(_), LiftedType::Predicate)
        ));
        evaluated.insert(expression.id, result);
    }
    let result = evaluated.remove(&root).expect("root evaluated");
    assert_eq!(
        program.evaluate_expression(root, |id| bindings.get(&id).copied()),
        Some(result),
        "public evaluator disagrees with independent word/predicate semantics"
    );
    result
}

//! Bounded simultaneous assignments for selected symbolic witness tests.
//! This deliberately small evaluator rejects unsupported structure. It is not
//! an oracle for arbitrary expressions, byte stores, or unbounded EVM inputs.
use alloy_primitives::U256;
use evm_abstract::domain::symbolic::{ExprId, ExprKind};
use revm_bytecode::opcode;
use std::collections::BTreeMap;

#[derive(Debug, PartialEq, Eq)]
pub enum WitnessError {
    Unsupported,
    Unassigned,
    Bound,
    DuplicateInput,
}

fn evaluate(
    expr: &ExprId,
    words: &BTreeMap<ExprId, U256>,
    left: &mut usize,
) -> Result<U256, WitnessError> {
    *left = left.checked_sub(1).ok_or(WitnessError::Bound)?;
    match expr.kind() {
        ExprKind::Constant(value) => Ok(*value),
        ExprKind::Input(_) | ExprKind::Fresh(_) | ExprKind::SlotRead { .. } => {
            words.get(expr).copied().ok_or(WitnessError::Unassigned)
        }
        ExprKind::Operation { opcode: op, args } => {
            let args = args
                .iter()
                .map(|arg| evaluate(arg, words, left))
                .collect::<Result<Vec<_>, _>>()?;
            match (*op, args.as_slice()) {
                (opcode::ADD, [a, b]) => Ok(a.overflowing_add(*b).0),
                (opcode::SUB, [a, b]) => Ok(a.overflowing_sub(*b).0),
                (opcode::MUL, [a, b]) => Ok(a.overflowing_mul(*b).0),
                (opcode::XOR, [a, b]) => Ok(a ^ b),
                (opcode::AND, [a, b]) => Ok(a & b),
                (opcode::OR, [a, b]) => Ok(a | b),
                (opcode::EQ, [a, b]) => Ok(U256::from(a == b)),
                (opcode::LT, [a, b]) => Ok(U256::from(a < b)),
                (opcode::ISZERO, [a]) => Ok(U256::from(*a == U256::ZERO)),
                _ => Err(WitnessError::Unsupported),
            }
        }
        _ => Err(WitnessError::Unsupported),
    }
}

pub fn includes(
    inputs: &[(ExprId, Vec<U256>)],
    observations: &[(ExprId, U256)],
    guards: &[(ExprId, bool)],
) -> Result<bool, WitnessError> {
    let mut assignments = vec![BTreeMap::new()];
    for (input, values) in inputs {
        if !matches!(
            input.kind(),
            ExprKind::Input(_) | ExprKind::Fresh(_) | ExprKind::SlotRead { .. }
        ) {
            return Err(WitnessError::Unsupported);
        }
        if assignments.len().saturating_mul(values.len()) > 64 {
            return Err(WitnessError::Bound);
        }
        let mut next = Vec::new();
        for assignment in assignments {
            if assignment.contains_key(input) {
                return Err(WitnessError::DuplicateInput);
            }
            for value in values {
                let mut words = assignment.clone();
                words.insert(input.clone(), *value);
                next.push(words);
            }
        }
        assignments = next;
    }
    for words in assignments {
        let mut left = 256;
        let actual = observations
            .iter()
            .map(|(expr, expected)| {
                evaluate(expr, &words, &mut left).map(|actual| actual == *expected)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let feasible = guards
            .iter()
            .map(|(expr, expected)| {
                evaluate(expr, &words, &mut left).map(|actual| (actual != U256::ZERO) == *expected)
            })
            .collect::<Result<Vec<_>, _>>()?;
        if actual.into_iter().chain(feasible).all(|matches| matches) {
            return Ok(true);
        }
    }
    Ok(false)
}

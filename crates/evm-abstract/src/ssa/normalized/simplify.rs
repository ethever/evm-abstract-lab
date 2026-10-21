//! Unconditional EVM identities and exact, carrier-independent word evaluation.

use super::{
    FactSource, NormalizedInstruction, NormalizedOperation, ValueMapping, ValueOperand,
    ValueTransformation,
};
use crate::{analysis::InstructionProgress, ssa::ValueId};
use alloy_primitives::U256;
use revm_bytecode::opcode;

pub(super) struct Simplification {
    pub operation: NormalizedOperation,
    pub transformation: ValueTransformation,
    pub derived: Option<(U256, FactSource)>,
    pub equivalent: Option<ValueId>,
}

pub(super) fn instruction(
    step: &NormalizedInstruction,
    known: &[Option<U256>],
    mappings: &[ValueMapping],
) -> Simplification {
    let operands = step
        .origin
        .operands
        .iter()
        .map(|value| operand(*value, known, mappings))
        .collect::<Vec<_>>();
    let original = || Simplification {
        operation: NormalizedOperation::Opcode {
            opcode: step.origin.opcode,
            operands: operands.clone(),
        },
        transformation: ValueTransformation::Original,
        derived: None,
        equivalent: None,
    };
    // A reached or partially prepared instruction has no normal computation.
    if step.progress != InstructionProgress::Completed
        || step.origin.fault
        || step.origin.results.len() != 1
    {
        return original();
    }
    if let Some(value) = step.origin.immediate {
        return constant(
            value,
            ValueTransformation::Literal,
            Some(FactSource::Literal),
        );
    }
    let op = step.origin.opcode;
    if pure(op, operands.len()) {
        if let Some(constants) = operands
            .iter()
            .map(|value| match value {
                ValueOperand::Constant(value) => Some(*value),
                ValueOperand::Value(_) => None,
            })
            .collect::<Option<Vec<_>>>()
        {
            let value = crate::domain::concrete::evaluate(
                op,
                constants[0],
                constants.get(1).copied().unwrap_or(U256::ZERO),
                constants.get(2).copied().unwrap_or(U256::ZERO),
            );
            return constant(
                value,
                ValueTransformation::ConstantFold,
                Some(FactSource::ConstantFold),
            );
        }
        if let Some(operation) = algebraic(op, &operands) {
            return match operation {
                NormalizedOperation::Constant { value } => constant(
                    value,
                    ValueTransformation::AlgebraicIdentity,
                    Some(FactSource::AlgebraicIdentity),
                ),
                NormalizedOperation::Copy { value } => Simplification {
                    equivalent: match value {
                        ValueOperand::Value(value) => Some(value),
                        ValueOperand::Constant(_) => None,
                    },
                    operation: NormalizedOperation::Copy { value },
                    transformation: ValueTransformation::AlgebraicIdentity,
                    derived: None,
                },
                NormalizedOperation::Opcode { .. } => {
                    unreachable!("algebraic rules produce only constants and copies")
                }
            };
        }
    }
    if let Some(value) = known[step.origin.results[0]] {
        return constant(value, ValueTransformation::AnalyzedConstant, None);
    }
    original()
}

fn constant(
    value: U256,
    transformation: ValueTransformation,
    source: Option<FactSource>,
) -> Simplification {
    Simplification {
        operation: NormalizedOperation::Constant { value },
        transformation,
        derived: source.map(|source| (value, source)),
        equivalent: None,
    }
}

fn operand(mut value: ValueId, known: &[Option<U256>], mappings: &[ValueMapping]) -> ValueOperand {
    loop {
        if let Some(constant) = known[value] {
            return ValueOperand::Constant(constant);
        }
        if let Some(equivalent) = mappings[value].equivalent {
            value = equivalent;
        } else {
            return ValueOperand::Value(value);
        }
    }
}

fn pure(op: u8, arity: usize) -> bool {
    crate::domain::concrete::arity(op) == Some(arity)
}

fn algebraic(op: u8, operands: &[ValueOperand]) -> Option<NormalizedOperation> {
    let constant = |value| NormalizedOperation::Constant { value };
    let copy = |index: usize| NormalizedOperation::Copy {
        value: operands[index].clone(),
    };
    let a = operands.first()?;
    let b = operands.get(1);
    let is = |value: &ValueOperand, word| *value == ValueOperand::Constant(word);
    let zero_a = is(a, U256::ZERO);
    let zero_b = b.is_some_and(|b| is(b, U256::ZERO));
    let one_a = is(a, U256::from(1));
    let one_b = b.is_some_and(|b| is(b, U256::from(1)));
    let max_a = is(a, U256::MAX);
    let max_b = b.is_some_and(|b| is(b, U256::MAX));
    // Equality is identity of SSA names (including propagated unconditional
    // copies), never equality of an abstract interval or source category.
    let same = b.is_some_and(|b| a == b);
    match op {
        opcode::ADD | opcode::OR | opcode::XOR if zero_a => Some(copy(1)),
        opcode::ADD | opcode::SUB | opcode::OR | opcode::XOR if zero_b => Some(copy(0)),
        opcode::MUL | opcode::AND if zero_a || zero_b => Some(constant(U256::ZERO)),
        opcode::MUL if one_a => Some(copy(1)),
        opcode::MUL | opcode::DIV | opcode::SDIV if one_b => Some(copy(0)),
        opcode::DIV | opcode::SDIV | opcode::MOD | opcode::SMOD if zero_a || zero_b => {
            Some(constant(U256::ZERO))
        }
        opcode::MOD | opcode::SMOD if one_b || same => Some(constant(U256::ZERO)),
        opcode::ADDMOD | opcode::MULMOD
            if is(&operands[2], U256::ZERO) || is(&operands[2], U256::from(1)) =>
        {
            Some(constant(U256::ZERO))
        }
        opcode::MULMOD if zero_a || zero_b => Some(constant(U256::ZERO)),
        opcode::EXP if zero_b || one_a => Some(constant(U256::from(1))),
        opcode::EXP if one_b => Some(copy(0)),
        opcode::SUB | opcode::XOR | opcode::LT | opcode::GT | opcode::SLT | opcode::SGT if same => {
            Some(constant(U256::ZERO))
        }
        opcode::EQ if same => Some(constant(U256::from(1))),
        opcode::LT if zero_b => Some(constant(U256::ZERO)),
        opcode::GT if zero_a => Some(constant(U256::ZERO)),
        opcode::AND if max_a => Some(copy(1)),
        opcode::AND if max_b || same => Some(copy(0)),
        opcode::OR if max_a || max_b => Some(constant(U256::MAX)),
        opcode::OR if same => Some(copy(0)),
        opcode::SHL | opcode::SHR | opcode::SAR if zero_a => Some(copy(1)),
        opcode::SHL | opcode::SHR | opcode::SAR | opcode::BYTE if zero_b => {
            Some(constant(U256::ZERO))
        }
        opcode::SHL | opcode::SHR if matches!(a, ValueOperand::Constant(value) if *value >= U256::from(256)) => {
            Some(constant(U256::ZERO))
        }
        opcode::SAR if max_b => Some(constant(U256::MAX)),
        opcode::BYTE if matches!(a, ValueOperand::Constant(value) if *value >= U256::from(32)) => {
            Some(constant(U256::ZERO))
        }
        opcode::SIGNEXTEND if matches!(a, ValueOperand::Constant(value) if *value >= U256::from(31)) => {
            Some(copy(1))
        }
        _ => None,
    }
}

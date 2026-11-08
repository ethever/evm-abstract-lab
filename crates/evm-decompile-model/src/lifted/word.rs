//! Shared exact Word256 rules. The caller admits types, scopes and source
//! availability; this kernel sees only literal words, opaque identities and
//! existing AND operands. It never follows a primitive's definition or effects.

use crate::{EntityId, U256};

/// Minimal word semantics needed by the shared algebraic rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordExpr {
    /// An exact modulo-2^256 literal, not an approximate numeric fact.
    Literal(U256),
    /// An admitted word identity whose producer remains opaque.
    Opaque,
    /// An existing typed bitwise AND expression in the same evaluation scope.
    And {
        /// First word operand.
        left: EntityId,
        /// Second word operand.
        right: EntityId,
    },
}

/// A small immutable recipe; consumers retain their own arena and source IDs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordRewrite {
    /// No root rule applied; retain the original primitive presentation.
    Unchanged,
    /// Reuse an existing admitted word expression.
    Existing(EntityId),
    /// An exact constant result.
    Literal(U256),
    /// Replace nested masks with one AND, in value/mask operand order.
    Masked {
        /// Existing word expression retained once under the combined mask.
        value: EntityId,
        /// Exact combined Word256 mask.
        mask: U256,
    },
}

/// Local rule refusal, separate from a decompilation artifact's completeness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordSimplifyError {
    /// Caller refused the complete lookup or constant-evaluation work.
    Limit,
    /// Not an admitted non-predicate word opcode/arity.
    Unsupported,
    /// A referenced word expression is absent from the caller's admitted view.
    Missing(EntityId),
}
impl std::fmt::Display for WordSimplifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "word simplification refused: {self:?}")
    }
}
impl std::error::Error for WordSimplifyError {}

fn lookup(
    id: EntityId,
    view: &mut impl FnMut(EntityId) -> Option<WordExpr>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<WordExpr, WordSimplifyError> {
    if !pay(1) {
        return Err(WordSimplifyError::Limit);
    }
    view(id).ok_or(WordSimplifyError::Missing(id))
}
fn literal(value: WordExpr) -> Option<U256> {
    if let WordExpr::Literal(word) = value {
        Some(word)
    } else {
        None
    }
}

/// Apply exact local arithmetic rules once at the root, without allocating DAG
/// nodes. Comparisons and ISZERO stay in the owner's predicate conversion path.
/// Each repeated child contributes its identity independently; no storage read,
/// SSA definition, range fact or spelling is inspected. Work refusal publishes
/// no partial recipe. Constant EXP/modular arithmetic pays its actual evaluator
/// quote before executing, and every supported arity is at most three.
pub fn simplify_word(
    opcode: u8,
    arguments: &[EntityId],
    mut view: impl FnMut(EntityId) -> Option<WordExpr>,
    mut pay: impl FnMut(usize) -> bool,
) -> Result<WordRewrite, WordSimplifyError> {
    if !pay(1) {
        return Err(WordSimplifyError::Limit);
    }
    if matches!(opcode, 0x10..=0x15) || evm_bytecode::word::arity(opcode) != Some(arguments.len()) {
        return Err(WordSimplifyError::Unsupported);
    }
    let operands = arguments
        .iter()
        .map(|id| lookup(*id, &mut view, &mut pay))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some(words) = operands
        .iter()
        .copied()
        .map(literal)
        .collect::<Option<Vec<_>>>()
    {
        if !pay(evm_bytecode::word::evaluation_work(opcode)) {
            return Err(WordSimplifyError::Limit);
        }
        return evm_bytecode::word::evaluate(opcode, &words)
            .map(WordRewrite::Literal)
            .map_err(|_| WordSimplifyError::Unsupported);
    }
    if arguments.len() != 2 {
        return Ok(WordRewrite::Unchanged);
    }
    let (a, b) = (arguments[0], arguments[1]);
    let (left, right) = (literal(operands[0]), literal(operands[1]));
    let zero = Some(U256::ZERO);
    let one = Some(U256::from(1));
    let all = Some(U256::MAX);
    let identity = match opcode {
        0x01 | 0x17 | 0x18 if left == zero => Some(b),
        0x01 | 0x03 | 0x17 | 0x18 if right == zero => Some(a),
        0x02 if left == one => Some(b),
        0x02 | 0x04 if right == one => Some(a),
        0x16 if left == all => Some(b),
        0x16 if right == all => Some(a),
        0x16 | 0x17 if a == b => Some(a),
        0x1b..=0x1d if left == zero => Some(b),
        _ => None,
    };
    if let Some(existing) = identity {
        return Ok(WordRewrite::Existing(existing));
    }
    let is_zero = match opcode {
        0x02 | 0x16 => left == zero || right == zero,
        0x03 | 0x18 => a == b,
        0x04..=0x07 => left == zero || right == zero,
        _ => false,
    };
    if is_zero {
        return Ok(WordRewrite::Literal(U256::ZERO));
    }
    if opcode == 0x16 {
        for (nested, other, nested_view, outer_mask) in
            [(a, b, operands[0], right), (b, a, operands[1], left)]
        {
            let WordExpr::And {
                left: inner_left,
                right: inner_right,
            } = nested_view
            else {
                continue;
            };
            if inner_left == other || inner_right == other {
                return Ok(WordRewrite::Existing(nested));
            }
            if let Some(outer) = outer_mask {
                for (inner, value) in [(inner_left, inner_right), (inner_right, inner_left)] {
                    if let Some(inner_mask) = literal(lookup(inner, &mut view, &mut pay)?) {
                        let mask = outer & inner_mask;
                        return Ok(if mask == U256::ZERO {
                            WordRewrite::Literal(U256::ZERO)
                        } else {
                            WordRewrite::Masked { value, mask }
                        });
                    }
                }
            }
        }
    }
    Ok(WordRewrite::Unchanged)
}

//! Predicate polarity belongs to the typed arena, never to rendered text.
//! Materialized word bindings stay opaque; this pass follows only arena operands.

use super::{LiftedExpressionKind as Kind, LiftedRelation as Relation, LiftedRenderError as Error};
use super::{LiftedRenderIndex, MAX_LIFTED_EXPRESSION_TEXT, primitive::append};
use crate::{EntityId, LiftedType};

impl LiftedRenderIndex<'_> {
    /// Render a predicate or its logical complement without redundant negations.
    /// Word truth tests remain explicit; signed comparisons retain signedness
    /// and use only the existing SLT/SGT primitives. Both traversal and text are
    /// bounded even when the caller supplies an unlimited work allowance.
    pub fn render_condition(
        &self,
        expression: EntityId,
        invert: bool,
        mut reserve: impl FnMut(usize) -> bool,
    ) -> Result<String, Error> {
        let mut remaining = MAX_LIFTED_EXPRESSION_TEXT;
        let mut pay = |amount| {
            if amount > remaining || !reserve(amount) {
                return false;
            }
            remaining -= amount;
            true
        };
        let mut current = expression;
        let mut invert = invert;
        let mut scope = None;
        loop {
            if !pay(1) {
                return Err(Error::Limit);
            }
            let predicate = self.condition_operand(current, LiftedType::Predicate)?;
            if scope.is_some_and(|block| block != predicate.block) {
                return Err(Error::MissingWitness(current));
            }
            scope = Some(predicate.block);
            match &predicate.kind {
                Kind::Not(child) => {
                    current = *child;
                    invert = !invert;
                }
                Kind::Predicate(value) => {
                    let mut out = String::new();
                    append(
                        &mut out,
                        if *value != invert { "true" } else { "false" },
                        &mut pay,
                    )?;
                    return Ok(out);
                }
                Kind::NonZero(word) => {
                    if !pay(1) {
                        return Err(Error::Limit);
                    }
                    let operand = self.condition_operand(*word, LiftedType::Word256)?;
                    if operand.block != predicate.block {
                        return Err(Error::MissingWitness(*word));
                    }
                    // This is the explicit 0/1 encoding of this exact predicate,
                    // not an inferred property of an opaque SSA word binding.
                    if let Kind::WordFromPredicate(child) = operand.kind {
                        current = child;
                        continue;
                    }
                    let word = self.render_expression(*word, &mut pay)?;
                    let mut out = String::new();
                    append(&mut out, "(", &mut pay)?;
                    append(&mut out, &word, &mut pay)?;
                    append(&mut out, if invert { " == 0)" } else { " != 0)" }, &mut pay)?;
                    return Ok(out);
                }
                Kind::Compare {
                    relation,
                    left,
                    right,
                } => {
                    for operand in [left, right] {
                        if self.condition_operand(*operand, LiftedType::Word256)?.block
                            != predicate.block
                        {
                            return Err(Error::MissingWitness(*operand));
                        }
                    }
                    let relation = if invert {
                        relation.complement()
                    } else {
                        *relation
                    };
                    let left = self.render_expression(*left, &mut pay)?;
                    let right = self.render_expression(*right, &mut pay)?;
                    return comparison(relation, &left, &right, &mut pay);
                }
                // A declaration that disagrees with its kind has no typed
                // predicate witness and must not be silently coerced to truth.
                _ => return Err(Error::MissingWitness(current)),
            }
        }
    }

    fn condition_operand(
        &self,
        expression: EntityId,
        ty: LiftedType,
    ) -> Result<&crate::LiftedExpression, Error> {
        let operand = self
            .expressions
            .get(&expression)
            .copied()
            .ok_or(Error::MissingExpression(expression))?;
        if operand.ty != ty || operand.kind.ty() != ty {
            return Err(Error::MissingWitness(expression));
        }
        Ok(operand)
    }
}

fn comparison(
    relation: Relation,
    left: &str,
    right: &str,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<String, Error> {
    let (symbol, signed, inverted) = match relation {
        Relation::Eq => ("==", false, false),
        Relation::Ne => ("!=", false, false),
        Relation::Ult => ("<", false, false),
        Relation::Ule => ("<=", false, false),
        Relation::Ugt => (">", false, false),
        Relation::Uge => (">=", false, false),
        Relation::Slt => ("slt", true, false),
        Relation::Sle => ("sgt", true, true),
        Relation::Sgt => ("sgt", true, false),
        Relation::Sge => ("slt", true, true),
    };
    let mut out = String::new();
    if signed {
        if inverted {
            append(&mut out, "!(", pay)?;
        }
        append(&mut out, "evm.", pay)?;
        append(&mut out, symbol, pay)?;
        append(&mut out, "(left = ", pay)?;
        append(&mut out, left, pay)?;
        append(&mut out, ", right = ", pay)?;
        append(&mut out, right, pay)?;
        append(&mut out, if inverted { "))" } else { ")" }, pay)?;
    } else {
        append(&mut out, "(", pay)?;
        append(&mut out, left, pay)?;
        append(&mut out, " ", pay)?;
        append(&mut out, symbol, pay)?;
        append(&mut out, " ", pay)?;
        append(&mut out, right, pay)?;
        append(&mut out, ")", pay)?;
    }
    Ok(out)
}

//! Direct conjunction facts for reducing demanded reads after their assertions.
//!
//! Facts come only from the already encoded unconditional constraints. A path
//! alternative's atoms never enter this table. Selection cannot replace the
//! original predicate assertion, even when an inconsistent conjunction exists.

use super::{BV, Bool, Constraint, Encoder, ExprId, QueryReason, word};
use crate::domain::symbolic::ExprKind;
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct Guarantees {
    truths: BTreeMap<ExprId, bool>,
    equalities: BTreeMap<(ExprId, ExprId), bool>,
    bounds: BTreeMap<ExprId, (U256, U256)>,
}
impl Guarantees {
    pub(super) fn minimum(&self, expression: &ExprId) -> Option<U256> {
        expression
            .as_constant()
            .or_else(|| self.bounds.get(expression).map(|(lower, _)| *lower))
    }
    pub(super) fn truth(&self, expression: &ExprId) -> Option<bool> {
        expression
            .as_constant()
            .map(|value| value != U256::ZERO)
            .or_else(|| self.truths.get(expression).copied())
    }
    pub(super) fn equality(&self, left: &ExprId, right: &ExprId) -> Option<bool> {
        if left == right {
            return Some(true);
        }
        self.equalities.get(&ordered(left, right)).copied()
    }
}
fn ordered(left: &ExprId, right: &ExprId) -> (ExprId, ExprId) {
    if left <= right {
        (left.clone(), right.clone())
    } else {
        (right.clone(), left.clone())
    }
}
impl Encoder<'_> {
    pub(super) fn remember_guarantee(
        &mut self,
        constraint: &Constraint,
    ) -> Result<(), QueryReason> {
        self.charge(1)?;
        match constraint {
            Constraint::Truth {
                expression,
                nonzero,
            } => {
                let mut expression = expression;
                let mut nonzero = *nonzero;
                loop {
                    self.charge(1)?;
                    self.guarantees
                        .truths
                        .entry(expression.clone())
                        .or_insert(nonzero);
                    match expression.kind() {
                        ExprKind::Operation {
                            opcode: opcode::ISZERO,
                            args,
                        } => {
                            expression = &args[0];
                            nonzero = !nonzero;
                        }
                        ExprKind::Operation {
                            opcode: opcode::EQ,
                            args,
                        } => {
                            self.guarantees
                                .equalities
                                .entry(ordered(&args[0], &args[1]))
                                .or_insert(nonzero);
                            break;
                        }
                        _ => break,
                    }
                }
            }
            Constraint::Equal { left, right } => {
                self.guarantees
                    .equalities
                    .entry(ordered(left, right))
                    .or_insert(true);
            }
            Constraint::Range {
                expression,
                lower,
                upper,
            } => {
                self.guarantees
                    .bounds
                    .entry(expression.clone())
                    .or_insert((*lower, *upper));
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn unsigned_le(
        &mut self,
        expression: &ExprId,
        value: &BV,
        upper: U256,
    ) -> Result<Bool, QueryReason> {
        let mut expression = expression;
        loop {
            self.charge(1)?;
            if let Some(exact) = expression.as_constant() {
                return Ok(Bool::from_bool(exact <= upper));
            }
            if let Some((lower, maximum)) = self.guarantees.bounds.get(expression) {
                if *maximum <= upper {
                    return Ok(Bool::from_bool(true));
                }
                if *lower > upper {
                    return Ok(Bool::from_bool(false));
                }
                break;
            }
            let ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } = expression.kind()
            else {
                break;
            };
            let Some(nonzero) = self.guarantees.truth(condition) else {
                break;
            };
            expression = if nonzero { then_value } else { else_value };
        }
        Ok(value.bvule(word(upper)))
    }
}

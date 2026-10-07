//! Cheap scalar consequences of already retained direct branch guarantees.
//!
//! No native query occurs here. Existing fact rules implement strict unsigned
//! and signed boundaries; the imported scalar is intersected with every current
//! numeric component rather than replacing its approximation.

use super::{Constraint, QueryReason, RelationLimits, RelationState};
use crate::domain::{
    AbstractValue, Domain, NumericValue, ReductionStatus,
    facts::{
        BinaryFact, BinaryPredicate, BitConstraints, Fact, FactError, FactLattice, FiniteSet,
        Symbol, Term, UnaryFact, UnaryPredicate, WordBounds,
    },
    reduce,
    symbolic::{ExprId, ExprKind},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;

#[cfg(test)]
mod tests;

/// Result of direct-guarantee scalar projection; this is not a uniqueness claim.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ScalarQuery {
    /// Direct consequences were imported, possibly retaining the same bounds.
    Refined {
        /// Safe intersection of the old scalar and direct guarantees.
        numeric: Box<NumericValue>,
        /// All related constraints had a direct scalar translation. This does
        /// not assert an exact value or an exact union of possible executions.
        complete: bool,
    },
    /// No direct guard for this expression was available.
    Unchanged,
    /// A numerical contradiction was established by exact local fact rules.
    Infeasible,
    /// No bottom claim; keep the old scalar or any already returned refinement.
    Unknown(QueryReason),
}

fn binary(left: Term, predicate: BinaryPredicate, right: Term) -> Fact {
    Fact::Binary(BinaryFact::new(left, predicate, right))
}
fn guard(expression: &ExprId, truth: bool, target: &ExprId) -> Option<Fact> {
    let subject = Term::Symbol(Symbol::THIS);
    if expression == target {
        return Some(binary(
            subject,
            if truth {
                BinaryPredicate::Ne
            } else {
                BinaryPredicate::Eq
            },
            Term::Constant(U256::ZERO),
        ));
    }
    let ExprKind::Operation { opcode: op, args } = expression.kind() else {
        return None;
    };
    if *op == opcode::ISZERO {
        return guard(&args[0], !truth, target);
    }
    let (left, right) = if &args[0] == target {
        (subject, Term::Constant(args.get(1)?.as_constant()?))
    } else if args.get(1) == Some(target) {
        (Term::Constant(args[0].as_constant()?), subject)
    } else {
        return None;
    };
    Some(match *op {
        opcode::EQ => binary(
            left,
            if truth {
                BinaryPredicate::Eq
            } else {
                BinaryPredicate::Ne
            },
            right,
        ),
        opcode::LT => {
            if truth {
                binary(left, BinaryPredicate::Ult, right)
            } else {
                binary(right, BinaryPredicate::Ule, left)
            }
        }
        opcode::GT => {
            if truth {
                binary(right, BinaryPredicate::Ult, left)
            } else {
                binary(left, BinaryPredicate::Ule, right)
            }
        }
        opcode::SLT => {
            if truth {
                binary(left, BinaryPredicate::Slt, right)
            } else {
                binary(right, BinaryPredicate::Sle, left)
            }
        }
        opcode::SGT => {
            if truth {
                binary(right, BinaryPredicate::Slt, left)
            } else {
                binary(left, BinaryPredicate::Sle, right)
            }
        }
        _ => return None,
    })
}
fn direct(constraint: &Constraint, target: &ExprId) -> Result<Option<Fact>, FactError> {
    let unary = |predicate| Some(Fact::Unary(UnaryFact::new(Symbol::THIS, predicate)));
    Ok(match constraint {
        Constraint::Truth {
            expression,
            nonzero,
        } => guard(expression, *nonzero, target),
        Constraint::Range {
            expression,
            lower,
            upper,
        } if expression == target => unary(UnaryPredicate::UnsignedBounds(WordBounds::new(
            *lower, *upper,
        )?)),
        Constraint::Bits {
            expression,
            zero,
            one,
        } if expression == target => {
            unary(UnaryPredicate::KnownBits(BitConstraints::new(*zero, *one)?))
        }
        Constraint::Members { expression, values } if expression == target => {
            unary(UnaryPredicate::MemberOf(FiniteSet::new(values.clone())?))
        }
        Constraint::Equal { left, right } if left == target && right.as_constant().is_some() => {
            Some(binary(
                Term::Symbol(Symbol::THIS),
                BinaryPredicate::Eq,
                Term::Constant(right.as_constant().unwrap()),
            ))
        }
        Constraint::Equal { left, right } if right == target && left.as_constant().is_some() => {
            Some(binary(
                Term::Symbol(Symbol::THIS),
                BinaryPredicate::Eq,
                Term::Constant(left.as_constant().unwrap()),
            ))
        }
        _ => None,
    })
}
fn failed(error: FactError) -> ScalarQuery {
    match error {
        FactError::Capacity { .. } => ScalarQuery::Unknown(QueryReason::ScalarFactLimit),
        FactError::Contradiction { .. }
        | FactError::EmptyFiniteSet
        | FactError::ConflictingBits
        | FactError::InvalidBounds => ScalarQuery::Infeasible,
        other => ScalarQuery::Unknown(QueryReason::ScalarFactError(other.to_string())),
    }
}
impl RelationState {
    /// Project direct EQ/ISZERO/ordered constant guards through the existing
    /// scalar fact language. A useful range can avoid an unnecessary SMT unique
    /// query; nonlocal inverse constraints remain explicitly incomplete here.
    pub fn refine_numeric(
        &self,
        expression: &ExprId,
        numeric: &NumericValue,
        domain: Domain,
        limits: &RelationLimits,
    ) -> ScalarQuery {
        if self.bottom {
            return ScalarQuery::Infeasible;
        }
        if !limits.enabled {
            return ScalarQuery::Unknown(QueryReason::Disabled);
        }
        if !limits.valid() {
            return ScalarQuery::Unknown(QueryReason::Configuration);
        }
        if expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth {
            return ScalarQuery::Unknown(QueryReason::ExpressionLimit);
        }
        let variables = expression.variables();
        let mut lattice = FactLattice::new(domain.spec().fact_limit());
        if let Some(values) = numeric.constants() {
            match FiniteSet::new(
                values
                    .iter()
                    .copied()
                    .filter(|value| numeric.contains(*value))
                    .collect(),
            ) {
                Ok(values) => {
                    if let Err(error) = lattice.insert(Fact::Unary(UnaryFact::new(
                        Symbol::THIS,
                        UnaryPredicate::MemberOf(values),
                    ))) {
                        return failed(error);
                    }
                }
                Err(error) => return failed(error),
            }
        }
        let mut found = false;
        let mut complete = true;
        let mut work = expression.nodes();
        for constraint in &self.constraints {
            work = constraint
                .expressions()
                .iter()
                .fold(work, |total, expr| total.saturating_add(expr.nodes()));
            if work > limits.max_nodes {
                return ScalarQuery::Unknown(QueryReason::ExpressionLimit);
            }
            match direct(constraint, expression) {
                Ok(Some(fact)) => {
                    found = true;
                    if let Err(error) = lattice.insert(fact) {
                        return failed(error);
                    }
                }
                Ok(None) => {
                    if constraint
                        .expressions()
                        .iter()
                        .any(|expr| expr.variables().iter().any(|leaf| variables.contains(leaf)))
                    {
                        complete = false;
                    }
                }
                Err(error) => return failed(error),
            }
        }
        if !found {
            return ScalarQuery::Unchanged;
        }
        let value = AbstractValue::from_numeric(numeric.clone());
        let value = match reduce::import(&value, &lattice) {
            Ok(value) => value,
            Err(error) => return failed(error),
        };
        let reduced = domain.reduce(&value);
        match reduced.status {
            ReductionStatus::Empty => ScalarQuery::Infeasible,
            // These limits only stop further reductions. The already imported
            // conjunction remains sound and must not be mistaken for bottom.
            ReductionStatus::FactLimit | ReductionStatus::RoundLimit => ScalarQuery::Refined {
                numeric: Box::new(domain.project(&reduced.value).numeric().clone()),
                complete: false,
            },
            _ => ScalarQuery::Refined {
                numeric: Box::new(domain.project(&reduced.value).numeric().clone()),
                complete,
            },
        }
    }
}

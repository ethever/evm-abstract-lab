//! Cheap scalar consequences of already retained direct branch guarantees.
//!
//! No native query occurs here. Existing fact rules implement strict unsigned
//! and signed boundaries; the imported scalar is intersected with every current
//! numeric component rather than replacing its approximation.

use super::{Constraint, QueryReason, RelationLimits, RelationState};
use crate::domain::{
    AlgebraRef, Domain, EvmNumericAlgebra, EvmValue, NumericValue, ReductionStatus, ValueAlgebra,
    WordElement,
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
pub enum EvmScalarQuery<N: WordElement> {
    /// Direct consequences were imported, possibly retaining the same bounds.
    Refined {
        /// Safe intersection of the old scalar and direct guarantees.
        numeric: Box<N>,
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

/// Default-domain compatibility scalar query.
pub type ScalarQuery = EvmScalarQuery<NumericValue>;

fn binary(left: Term, predicate: BinaryPredicate, right: Term) -> Fact {
    Fact::Binary(BinaryFact::new(left, predicate, right))
}

fn masked_equal(expression: &ExprId, constant: U256, target: &ExprId) -> Option<Fact> {
    let ExprKind::Operation { opcode: op, args } = expression.kind() else {
        return None;
    };
    if *op != opcode::AND {
        return None;
    }
    let mask = if args.first() == Some(target) {
        args.get(1)?.as_constant()?
    } else if args.get(1) == Some(target) {
        args.first()?.as_constant()?
    } else {
        return None;
    };
    // Equality of an address projection determines only the masked bits. In
    // particular, its high 96 bits are not proved zero by target dispatch.
    if constant & !mask != U256::ZERO {
        return None;
    }
    Some(Fact::Unary(UnaryFact::new(
        Symbol::THIS,
        UnaryPredicate::KnownBits(BitConstraints::new(mask & !constant, mask & constant).ok()?),
    )))
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
    if *op == opcode::EQ && truth {
        if let Some(constant) = args.get(1)?.as_constant()
            && let Some(fact) = masked_equal(&args[0], constant, target)
        {
            return Some(fact);
        }
        if let Some(constant) = args[0].as_constant()
            && let Some(fact) = masked_equal(&args[1], constant, target)
        {
            return Some(fact);
        }
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
pub(super) fn direct(constraint: &Constraint, target: &ExprId) -> Result<Option<Fact>, FactError> {
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
        Constraint::SignedRange {
            expression,
            lower,
            upper,
        } if expression == target => unary(UnaryPredicate::SignedBounds(WordBounds::new(
            *lower, *upper,
        )?)),
        Constraint::Modulo {
            expression,
            modulus,
            residue,
        } if expression == target => unary(UnaryPredicate::Congruent(
            super::super::congruence::Congruence::new(*modulus, *residue)
                .expect("observed nonzero modulus"),
        )),
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
        Constraint::Equal { left, right } => right
            .as_constant()
            .and_then(|constant| masked_equal(left, constant, target))
            .or_else(|| {
                left.as_constant()
                    .and_then(|constant| masked_equal(right, constant, target))
            }),
        _ => None,
    })
}
fn failed<N: WordElement>(error: FactError) -> EvmScalarQuery<N> {
    match error {
        FactError::Capacity { .. } => EvmScalarQuery::Unknown(QueryReason::ScalarFactLimit),
        FactError::Contradiction { .. }
        | FactError::EmptyFiniteSet
        | FactError::ConflictingBits
        | FactError::InvalidBounds => EvmScalarQuery::Infeasible,
        other => EvmScalarQuery::Unknown(QueryReason::ScalarFactError(other)),
    }
}
struct PreparedScalar<N: WordElement> {
    value: EvmValue<N>,
    complete: bool,
}

impl RelationState {
    /// Whether this exact expression has a consequence in the local scalar
    /// proof language. Callers can skip reduction for unrelated cached reads.
    pub(crate) fn has_direct_numeric_guarantee(&self, expression: &ExprId) -> bool {
        self.constraints
            .iter()
            .any(|constraint| !matches!(direct(constraint, expression), Ok(None)))
    }

    /// Project direct EQ/ISZERO/ordered constant guards through the existing
    /// scalar fact language. A useful range can avoid a larger local traversal; nonlocal inverse
    /// constraints remain explicitly incomplete here.
    pub fn refine_numeric_with<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        numeric: &D::Element,
        domain: AlgebraRef<'_, D>,
        limits: &RelationLimits,
    ) -> EvmScalarQuery<D::Element> {
        match self.prepare_scalar(expression, numeric, domain, limits) {
            Ok(prepared) => Self::finish_scalar(prepared, domain),
            Err(answer) => answer,
        }
    }

    /// Reserve detached fact import, then the actual imported numeric closure.
    /// Custom policies retain their whole-operation quote unless they opt in.
    pub fn refine_numeric_budgeted_with<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        value: &EvmValue<D::Element>,
        domain: AlgebraRef<'_, D>,
        limits: &RelationLimits,
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<EvmScalarQuery<D::Element>, crate::domain::WorkExhausted> {
        let (facts, finite, structural) = self.scalar_import_shape(expression, value.numeric());
        let staged = domain.numeric_algebra().refinement_import_work(
            value.numeric(),
            facts,
            finite,
            structural,
        );
        let work = staged.map_or_else(
            || domain.refinement_work(value, self.len()),
            |work| {
                work.saturating_add(
                    value
                        .work_size()
                        .saturating_sub(value.numeric().work_size()),
                )
            },
        );
        if budget.exhausted() || !budget.charge(work) {
            return Err(crate::domain::WorkExhausted);
        }
        let prepared = match self.prepare_scalar(expression, value.numeric(), domain, limits) {
            Ok(prepared) => prepared,
            Err(answer) => return Ok(answer),
        };
        if staged.is_some()
            && !budget.charge(domain.numeric_reduction_work(prepared.value.numeric()))
        {
            return Err(crate::domain::WorkExhausted);
        }
        Ok(Self::finish_scalar(prepared, domain))
    }

    fn scalar_import_shape<N: WordElement>(
        &self,
        target: &ExprId,
        numeric: &N,
    ) -> (usize, usize, usize) {
        let mut finite = numeric.finite_constants().cardinality().unwrap_or(0);
        let mut structural = target.work_size().saturating_add(self.path.nodes());
        let target_levels = target.nodes().saturating_add(1).ilog2() as usize + 2;
        let mut expression = |expr: &ExprId| {
            let levels = expr.nodes().saturating_add(1).ilog2() as usize + 2;
            // Variable walks and leaf-set lookups, plus at most four guard/
            // masked-target comparisons at each nested unary level. All sizes
            // are cached; quotation allocates/clones/enumerates no fact payload.
            structural = structural
                .saturating_add(
                    expr.work_size()
                        .saturating_mul(levels.saturating_add(target_levels).saturating_add(8)),
                )
                .saturating_add(
                    expr.work_size()
                        .saturating_add(target.work_size())
                        .saturating_mul(expr.depth().saturating_add(1))
                        .saturating_mul(4),
                );
        };
        for constraint in &self.constraints {
            match constraint {
                Constraint::Equal { left, right } => {
                    expression(left);
                    expression(right);
                }
                Constraint::Members {
                    expression: expr,
                    values,
                } => {
                    finite = finite.saturating_add(values.len());
                    expression(expr);
                }
                Constraint::Truth {
                    expression: expr, ..
                }
                | Constraint::Range {
                    expression: expr, ..
                }
                | Constraint::SignedRange {
                    expression: expr, ..
                }
                | Constraint::Modulo {
                    expression: expr, ..
                }
                | Constraint::Bits {
                    expression: expr, ..
                } => expression(expr),
            }
        }
        (self.constraints.len().saturating_add(1), finite, structural)
    }

    fn prepare_scalar<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        numeric: &D::Element,
        domain: AlgebraRef<'_, D>,
        limits: &RelationLimits,
    ) -> Result<PreparedScalar<D::Element>, EvmScalarQuery<D::Element>> {
        if self.bottom {
            return Err(EvmScalarQuery::Infeasible);
        }
        if !limits.enabled {
            return Err(EvmScalarQuery::Unknown(QueryReason::Disabled));
        }
        if !limits.valid() {
            return Err(EvmScalarQuery::Unknown(QueryReason::Configuration));
        }
        if expression.nodes() > limits.max_nodes || expression.depth() > limits.max_depth {
            return Err(EvmScalarQuery::Unknown(QueryReason::ExpressionLimit));
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
                        return Err(failed(error));
                    }
                }
                Err(error) => return Err(failed(error)),
            }
        }
        let mut found = false;
        let mut complete = !expression.has_state_semantics()
            && !matches!(expression.kind(), ExprKind::Ite { .. })
            && matches!(self.path.kind(), super::RelationPathKind::True);
        let mut work = expression.nodes().saturating_add(self.path.nodes());
        for constraint in &self.constraints {
            work = constraint
                .expressions()
                .iter()
                .fold(work, |total, expr| total.saturating_add(expr.nodes()));
            if work > limits.max_nodes {
                return Err(EvmScalarQuery::Unknown(QueryReason::ExpressionLimit));
            }
            match direct(constraint, expression) {
                Ok(Some(fact)) => {
                    found = true;
                    if let Err(error) = lattice.insert(fact) {
                        return Err(failed(error));
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
                Err(error) => return Err(failed(error)),
            }
        }
        if !found {
            return Err(EvmScalarQuery::Unchanged);
        }
        let value = EvmValue::from_numeric(numeric.clone());
        let value = match reduce::import(&value, &lattice) {
            Ok(value) => value,
            Err(error) => return Err(failed(error)),
        };
        Ok(PreparedScalar { value, complete })
    }

    fn finish_scalar<D: EvmNumericAlgebra>(
        prepared: PreparedScalar<D::Element>,
        domain: AlgebraRef<'_, D>,
    ) -> EvmScalarQuery<D::Element> {
        let reduced = domain.reduce(&prepared.value);
        let complete = prepared.complete;
        match reduced.status {
            ReductionStatus::Empty => EvmScalarQuery::Infeasible,
            // These limits only stop further reductions. The already imported
            // conjunction remains sound and must not be mistaken for bottom.
            ReductionStatus::FactLimit | ReductionStatus::RoundLimit => EvmScalarQuery::Refined {
                numeric: Box::new(domain.project(&reduced.value).numeric().clone()),
                complete: false,
            },
            _ => EvmScalarQuery::Refined {
                numeric: Box::new(domain.project(&reduced.value).numeric().clone()),
                complete,
            },
        }
    }
}

impl RelationState {
    /// Default-domain compatibility entrypoint. Generic execution uses refine_numeric_with.
    pub fn refine_numeric(
        &self,
        expression: &ExprId,
        numeric: &NumericValue,
        domain: Domain,
        limits: &RelationLimits,
    ) -> ScalarQuery {
        self.refine_numeric_with(
            expression,
            numeric,
            ValueAlgebra::new(domain).borrowed(),
            limits,
        )
    }
}

//! Bounded cross-value constraints, queried by an in-process bit-vector solver.
//!
//! Constraints are guarantees: join retains common guarantees, never conjoins
//! unrelated paths. Only a proved UNSAT prunes; budgets/unsupported encodings
//! and solver unknown preserve the path and its sound retained assumptions.

use super::{
    NumericValue,
    symbolic::{ExprError, ExprId, ExprKind, ExprLimits},
};
use alloy_primitives::U256;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

mod scalar;
mod solver;
pub use scalar::ScalarQuery;
#[cfg(test)]
mod tests;

/// Explicit bounded policy for expression construction and SMT queries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RelationLimits {
    /// Enable relational queries; scalar/value identity precision is independent.
    pub enabled: bool,
    /// Maximum retained conjunction atoms.
    pub max_constraints: usize,
    /// Maximum expression/encoding traversal work in one query.
    pub max_nodes: usize,
    /// Maximum expression nesting.
    pub max_depth: usize,
    /// Native solver deterministic resource bound.
    pub rlimit: u32,
}
impl Default for RelationLimits {
    fn default() -> Self {
        Self {
            enabled: true,
            max_constraints: 128,
            max_nodes: 1024,
            max_depth: 64,
            rlimit: 10_000,
        }
    }
}
impl RelationLimits {
    /// Derive expression retention bounds from the same policy.
    pub fn expressions(self) -> ExprLimits {
        ExprLimits {
            max_nodes: self.max_nodes,
            max_depth: self.max_depth,
        }
    }
    /// Zero limits are configuration errors, never unbounded solver settings.
    pub fn valid(self) -> bool {
        self.max_constraints > 0 && self.max_nodes > 0 && self.max_depth > 0 && self.rlimit > 0
    }
}

/// A query did not prove either satisfiability or contradiction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum QueryReason {
    /// Relational queries were explicitly disabled.
    Disabled,
    /// Invalid zero-sized query policy.
    Configuration,
    /// The next assumption cannot fit the retained conjunction.
    ConstraintLimit,
    /// The bounded local scalar fact table could not complete the projection.
    ScalarFactLimit,
    /// Local fact interpretation failed without proving numerical bottom.
    ScalarFactError(String),
    /// Expression size or encoded work exceeds the query policy.
    ExpressionLimit,
    /// A pure operation is conservatively opaque in the current encoder.
    Unsupported(u8),
    /// Native solver exhausted its deterministic resource allowance.
    ResourceLimit,
    /// Native solver could not complete its bounded query.
    SolverUnknown(String),
    /// A model or its word value was unavailable.
    ModelUnavailable,
}

/// Native check result; only Unsat justifies eliminating a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum CheckResult {
    /// Satisfiable in the supported exact bit-vector encoding.
    Sat,
    /// No valuation satisfies the retained guarantees.
    Unsat,
    /// Preserve the path; no contradiction was established.
    Unknown(QueryReason),
}

/// A bounded value-refinement query.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ValueQuery {
    /// All valuations of the retained constraints agree on this exact word.
    Unique(U256),
    /// At least two satisfying values remain.
    Multiple,
    /// The state is proved impossible.
    Infeasible,
    /// Leave the numerical approximation unchanged.
    Unknown(QueryReason),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) enum Constraint {
    Truth {
        expression: ExprId,
        nonzero: bool,
    },
    Range {
        expression: ExprId,
        lower: U256,
        upper: U256,
    },
    Bits {
        expression: ExprId,
        zero: U256,
        one: U256,
    },
    Equal {
        left: ExprId,
        right: ExprId,
    },
    Members {
        expression: ExprId,
        values: BTreeSet<U256>,
    },
}
impl Constraint {
    pub(crate) fn expressions(&self) -> Vec<&ExprId> {
        match self {
            Self::Truth { expression, .. }
            | Self::Range { expression, .. }
            | Self::Bits { expression, .. }
            | Self::Members { expression, .. } => vec![expression],
            Self::Equal { left, right } => vec![left, right],
        }
    }
    fn rename(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        Ok(match self {
            Self::Truth {
                expression,
                nonzero,
            } => Self::Truth {
                expression: expression.rename_fresh(map, limits)?,
                nonzero: *nonzero,
            },
            Self::Range {
                expression,
                lower,
                upper,
            } => Self::Range {
                expression: expression.rename_fresh(map, limits)?,
                lower: *lower,
                upper: *upper,
            },
            Self::Bits {
                expression,
                zero,
                one,
            } => Self::Bits {
                expression: expression.rename_fresh(map, limits)?,
                zero: *zero,
                one: *one,
            },
            Self::Members { expression, values } => Self::Members {
                expression: expression.rename_fresh(map, limits)?,
                values: values.clone(),
            },
            Self::Equal { left, right } => Self::Equal {
                left: left.rename_fresh(map, limits)?,
                right: right.rename_fresh(map, limits)?,
            },
        })
    }
}

/// Conjunction of proved branch and scalar guarantees over scoped expressions.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct RelationState {
    constraints: BTreeSet<Constraint>,
    bottom: bool,
}
impl RelationState {
    /// Whether this state has been proved contradictory.
    pub fn is_bottom(&self) -> bool {
        self.bottom
    }
    /// Number of retained conjunction atoms.
    pub fn len(&self) -> usize {
        self.constraints.len()
    }
    /// No retained guarantee and no known contradiction.
    pub fn is_empty(&self) -> bool {
        !self.bottom && self.constraints.is_empty()
    }
    /// Conservative work estimate for retention/traversal.
    pub fn work_size(&self) -> usize {
        self.constraints.iter().fold(1usize, |n, c| {
            c.expressions()
                .iter()
                .fold(n.saturating_add(1), |n, e| n.saturating_add(e.work_size()))
        })
    }
    fn insert(&mut self, constraint: Constraint, limits: &RelationLimits) -> CheckResult {
        if self.bottom {
            return CheckResult::Unsat;
        }
        if !limits.enabled {
            return CheckResult::Unknown(QueryReason::Disabled);
        }
        if !limits.valid() {
            return CheckResult::Unknown(QueryReason::Configuration);
        }
        if constraint
            .expressions()
            .iter()
            .any(|e| e.nodes() > limits.max_nodes || e.depth() > limits.max_depth)
        {
            return CheckResult::Unknown(QueryReason::ExpressionLimit);
        }
        if !self.constraints.contains(&constraint)
            && self.constraints.len() >= limits.max_constraints
        {
            return CheckResult::Unknown(QueryReason::ConstraintLimit);
        }
        self.constraints.insert(constraint);
        let checked = self.check(limits);
        if checked == CheckResult::Unsat {
            self.bottom = true;
        }
        checked
    }
    fn retain(
        &mut self,
        constraint: Constraint,
        limits: &RelationLimits,
    ) -> Result<(), QueryReason> {
        if !limits.enabled {
            return Err(QueryReason::Disabled);
        }
        if !limits.valid() {
            return Err(QueryReason::Configuration);
        }
        if constraint
            .expressions()
            .iter()
            .any(|e| e.nodes() > limits.max_nodes || e.depth() > limits.max_depth)
        {
            return Err(QueryReason::ExpressionLimit);
        }
        if !self.constraints.contains(&constraint)
            && self.constraints.len() >= limits.max_constraints
        {
            return Err(QueryReason::ConstraintLimit);
        }
        self.constraints.insert(constraint);
        Ok(())
    }
    /// Import sound scalar guarantees without invoking a solver per component.
    /// Budget failure may leave a sound subset installed; it never creates bottom.
    pub fn observe(
        &mut self,
        expression: &ExprId,
        numeric: &NumericValue,
        limits: &RelationLimits,
    ) -> Result<(), QueryReason> {
        if !limits.enabled {
            return Err(QueryReason::Disabled);
        }
        if !limits.valid() {
            return Err(QueryReason::Configuration);
        }
        if let Some(value) = numeric.singleton() {
            return self.retain(
                Constraint::Equal {
                    left: expression.clone(),
                    right: ExprId::constant(value),
                },
                limits,
            );
        }
        // Comparison expressions are already exact 0/1 words in the encoder.
        // Reasserting their range, 255 zero bits and {0,1} membership expands a
        // trivial branch into redundant BV circuits and wastes deterministic fuel.
        if numeric.contains(U256::ZERO)
            && numeric.contains(U256::from(1))
            && matches!(expression.kind(), ExprKind::Operation { opcode, .. } if matches!(*opcode,
                revm_bytecode::opcode::EQ | revm_bytecode::opcode::LT | revm_bytecode::opcode::GT |
                revm_bytecode::opcode::SLT | revm_bytecode::opcode::SGT | revm_bytecode::opcode::ISZERO))
        {
            return Ok(());
        }
        let (lower, upper) = numeric.interval().unsigned_bounds();
        let (bits_lower, bits_upper) = numeric.known_bits().unsigned_bounds();
        if (lower != U256::ZERO || upper != U256::MAX) && (bits_lower < lower || bits_upper > upper)
        {
            self.retain(
                Constraint::Range {
                    expression: expression.clone(),
                    lower,
                    upper,
                },
                limits,
            )?;
        }
        let zero = numeric.known_bits().zero();
        let one = numeric.known_bits().one();
        if zero != U256::ZERO || one != U256::ZERO {
            self.retain(
                Constraint::Bits {
                    expression: expression.clone(),
                    zero,
                    one,
                },
                limits,
            )?;
        }
        if let Some(values) = numeric.constants() {
            if values.len() > limits.max_nodes {
                return Err(QueryReason::ExpressionLimit);
            }
            self.retain(
                Constraint::Members {
                    expression: expression.clone(),
                    values: values
                        .iter()
                        .copied()
                        .filter(|v| numeric.contains(*v))
                        .collect(),
                },
                limits,
            )?;
        }
        if !numeric.may_be_zero() {
            self.retain(
                Constraint::Truth {
                    expression: expression.clone(),
                    nonzero: true,
                },
                limits,
            )?;
        }
        Ok(())
    }
    /// Intersect this state with an EVM condition (zero or nonzero).
    /// A solver Unknown retains the new sound assumption; a capacity failure
    /// preserves the prior, weaker state instead of pruning the path.
    pub fn assume(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
    ) -> CheckResult {
        self.insert(
            Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            },
            limits,
        )
    }
    /// Add sound unsigned numeric bounds for one value.
    pub fn add_unsigned_bounds(
        &mut self,
        expression: &ExprId,
        lower: U256,
        upper: U256,
        limits: &RelationLimits,
    ) -> CheckResult {
        self.insert(
            Constraint::Range {
                expression: expression.clone(),
                lower,
                upper,
            },
            limits,
        )
    }
    /// Add sound bit guarantees; contradictory bits may prove bottom.
    pub fn add_known_bits(
        &mut self,
        expression: &ExprId,
        zero: U256,
        one: U256,
        limits: &RelationLimits,
    ) -> CheckResult {
        self.insert(
            Constraint::Bits {
                expression: expression.clone(),
                zero,
                one,
            },
            limits,
        )
    }
    /// Add equality between two value expressions, for state/call substitutions.
    pub fn add_equal(
        &mut self,
        left: &ExprId,
        right: &ExprId,
        limits: &RelationLimits,
    ) -> CheckResult {
        self.insert(
            Constraint::Equal {
                left: left.clone(),
                right: right.clone(),
            },
            limits,
        )
    }
    /// Whether any retained guarantee depends on a variable in this value.
    /// Names alone are insufficient: leaf equality includes private input scope
    /// or the globally fresh runtime token. Disjoint values need no projection.
    pub fn relevant(&self, expression: &ExprId) -> bool {
        if self.bottom {
            return true;
        }
        let variables = expression.variables();
        self.constraints
            .iter()
            .flat_map(Constraint::expressions)
            .any(|constraint| {
                constraint
                    .variables()
                    .iter()
                    .any(|leaf| variables.contains(leaf))
            })
    }
    /// Alias for dependency-aware caller code.
    pub fn mentions(&self, expression: &ExprId) -> bool {
        self.relevant(expression)
    }
    /// Check the current conjunction with a fresh, privately owned native solver.
    pub fn check(&self, limits: &RelationLimits) -> CheckResult {
        solver::check(self, None, limits)
    }
    /// Check whether an exact candidate remains possible; Unknown keeps it.
    pub fn can_equal(
        &self,
        expression: &ExprId,
        value: U256,
        limits: &RelationLimits,
    ) -> CheckResult {
        solver::check(
            self,
            Some(Constraint::Equal {
                left: expression.clone(),
                right: ExprId::constant(value),
            }),
            limits,
        )
    }
    /// Ask whether every satisfying valuation agrees on this expression.
    pub fn unique_value(&self, expression: &ExprId, limits: &RelationLimits) -> ValueQuery {
        solver::unique(self, expression, limits)
    }
    /// Cover both paths by keeping only guarantees common to both.
    /// This intentionally loses disjunction precision, never intersects paths.
    pub fn join(&self, other: &Self) -> Self {
        if self.bottom {
            return other.clone();
        }
        if other.bottom {
            return self.clone();
        }
        Self {
            constraints: self
                .constraints
                .intersection(&other.constraints)
                .cloned()
                .collect(),
            bottom: false,
        }
    }
    /// Widen by forgetting any guarantee absent from the previous state.
    pub fn widen(&self, previous: &Self) -> Self {
        self.join(previous)
    }
    /// Forget guarantees, preserving reachability rather than fabricating bottom.
    pub fn forget(&mut self) {
        self.constraints.clear();
        self.bottom = false;
    }
    /// Runtime leaves needing capture-avoiding renaming during summary replay.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        self.constraints
            .iter()
            .flat_map(|c| c.expressions())
            .flat_map(ExprId::fresh_leaves)
            .collect()
    }
    /// Rename internal runtime leaves while preserving external scoped inputs.
    pub fn rename_fresh(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        Ok(Self {
            constraints: self
                .constraints
                .iter()
                .map(|c| c.rename(map, limits))
                .collect::<Result<BTreeSet<_>, _>>()?,
            bottom: self.bottom,
        })
    }
}

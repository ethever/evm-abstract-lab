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

mod path;
mod scalar;
mod solver;
pub use embedded_smt::{Error as SmtError, Provider as SmtProvider};
pub use path::{RelationPath, RelationPathKind};
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
    /// Selected in-process provider; it never falls back to a different provider.
    pub provider: SmtProvider,
    /// Per-check resource allowance in provider-specific, non-wall-clock units.
    pub rlimit: u32,
}
impl Default for RelationLimits {
    fn default() -> Self {
        Self {
            enabled: true,
            max_constraints: 128,
            max_nodes: 1024,
            max_depth: 64,
            provider: SmtProvider::default(),
            rlimit: embedded_smt::DEFAULT_RLIMIT,
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
    pub(crate) fn cache_scope(self) -> embedded_smt::CacheScope {
        embedded_smt::CacheScope {
            enabled: self.enabled,
            constraints: self.max_constraints,
            nodes: self.max_nodes,
            depth: self.max_depth,
        }
    }
}

/// A query did not prove either satisfiability or contradiction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum QueryReason {
    /// Explicit caller cancellation; the enclosing controlled analysis stops.
    Cancelled,
    /// Relational queries were explicitly disabled.
    Disabled,
    /// Invalid zero-sized query policy.
    Configuration,
    /// The next assumption cannot fit the retained conjunction.
    ConstraintLimit,
    /// The bounded local scalar fact table could not complete the projection.
    ScalarFactLimit,
    /// Local fact interpretation failed without proving numerical bottom.
    ScalarFactError(super::facts::FactError),
    /// Expression size or encoded work exceeds the query policy.
    ExpressionLimit,
    /// A pure operation is conservatively opaque in the current encoder.
    Unsupported(u8),
    /// Native solver exhausted its deterministic resource allowance.
    ResourceLimit,
    /// Native solver returned UNKNOWN for a provider-specific reason.
    SolverUnknown {
        /// Provider that returned UNKNOWN.
        provider: SmtProvider,
        /// Original native explanation; it is not an adapter error.
        reason: String,
    },
    /// The native binding or checked adapter failed; this is distinct from UNKNOWN.
    SolverError(SmtError),
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

/// A finite refinement stopped before all candidate answers were obtained.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CandidateError {
    #[error("the root query work budget was exhausted")]
    Work,
    #[error("candidate query stopped: {0:?}")]
    Query(QueryReason),
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

/// A retained guarantee over scoped Word256 expressions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Constraint {
    /// The word is zero or nonzero.
    Truth {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// True requires a nonzero word; false requires zero.
        nonzero: bool,
    },
    /// Inclusive unsigned word bounds.
    Range {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// Inclusive lower endpoint in the declared ordering.
        lower: U256,
        /// Inclusive upper endpoint in the declared ordering.
        upper: U256,
    },
    /// Inclusive signed-order bounds, encoded after XOR with the sign bit.
    SignedRange {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// Inclusive lower endpoint in the declared ordering.
        lower: U256,
        /// Inclusive upper endpoint in the declared ordering.
        upper: U256,
    },
    /// Unsigned word congruence.
    Modulo {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// Positive modulus for the unsigned word.
        modulus: U256,
        /// Normalized residue modulo the modulus.
        residue: U256,
    },
    /// Guaranteed zero and one bits.
    Bits {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// Bits which must be zero.
        zero: U256,
        /// Bits which must be one.
        one: U256,
    },
    /// Equality of two word expressions.
    Equal {
        /// First word in an equality constraint.
        left: ExprId,
        /// Second word in an equality constraint.
        right: ExprId,
    },
    /// Membership in a complete finite set.
    Members {
        /// Word expression constrained by this atom.
        expression: ExprId,
        /// Complete set of allowed word values.
        values: BTreeSet<U256>,
    },
}
impl Constraint {
    pub(crate) fn expressions(&self) -> Vec<&ExprId> {
        match self {
            Self::Truth { expression, .. }
            | Self::Range { expression, .. }
            | Self::SignedRange { expression, .. }
            | Self::Modulo { expression, .. }
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
            Self::SignedRange {
                expression,
                lower,
                upper,
            } => Self::SignedRange {
                expression: expression.rename_fresh(map, limits)?,
                lower: *lower,
                upper: *upper,
            },
            Self::Modulo {
                expression,
                modulus,
                residue,
            } => Self::Modulo {
                expression: expression.rename_fresh(map, limits)?,
                modulus: *modulus,
                residue: *residue,
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
    path: RelationPath,
}
impl RelationState {
    /// Whether this state has been proved contradictory.
    pub fn is_bottom(&self) -> bool {
        self.bottom
    }
    /// Number of retained conjunction atoms.
    pub fn len(&self) -> usize {
        self.constraints.len().saturating_add(self.path.atoms())
    }
    /// No retained guarantee and no known contradiction.
    pub fn is_empty(&self) -> bool {
        !self.bottom
            && self.constraints.is_empty()
            && matches!(self.path.kind(), RelationPathKind::True)
    }
    /// Conservative work estimate for retention/traversal.
    pub fn work_size(&self) -> usize {
        self.constraints.iter().fold(self.path.nodes(), |n, c| {
            c.expressions()
                .iter()
                .fold(n.saturating_add(1), |n, e| n.saturating_add(e.work_size()))
        })
    }
    fn insert(
        &mut self,
        constraint: Constraint,
        limits: &RelationLimits,
    ) -> Result<CheckResult, QueryReason> {
        self.insert_inner(constraint, limits, None)
    }
    fn insert_inner(
        &mut self,
        constraint: Constraint,
        limits: &RelationLimits,
        cache: Option<&mut embedded_smt::CacheQuery<'_>>,
    ) -> Result<CheckResult, QueryReason> {
        if self.bottom {
            return Ok(CheckResult::Unsat);
        }
        self.retain(constraint, limits)?;
        let checked = match cache {
            Some(cache) => solver::check_cached(self, None, limits, cache),
            None => self.check(limits),
        };
        if checked == CheckResult::Unsat {
            self.bottom = true;
        }
        Ok(checked)
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
        if !self.constraints.contains(&constraint) && self.len() >= limits.max_constraints {
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
        if let Some(values) = numeric.constants() {
            if values.len() > limits.max_nodes {
                return Err(QueryReason::ExpressionLimit);
            }
            // A complete finite set, filtered by the product, already implies
            // its masks, bounds, congruence and nonzero constraint. Retaining
            // those again can create expensive redundant Word256 divisions.
            return self.retain(
                Constraint::Members {
                    expression: expression.clone(),
                    values: values
                        .iter()
                        .copied()
                        .filter(|value| numeric.contains(*value))
                        .collect(),
                },
                limits,
            );
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
        let (lower, upper) = numeric.interval().signed_bounds();
        let sign = U256::from(1) << 255usize;
        let biased_one = (one & !sign) | (zero & sign);
        let biased_zero = (zero & !sign) | (one & sign);
        if (lower != U256::ZERO || upper != U256::MAX)
            && (biased_one < lower || !biased_zero > upper)
        {
            self.retain(
                Constraint::SignedRange {
                    expression: expression.clone(),
                    lower,
                    upper,
                },
                limits,
            )?;
        }
        if let Some((modulus, residue)) = numeric.congruence().modulus_residue()
            && modulus != U256::from(1)
        {
            self.retain(
                Constraint::Modulo {
                    expression: expression.clone(),
                    modulus,
                    residue,
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
    /// Use [`Self::assume_checked`] when coverage depends on retaining the guard.
    pub fn assume(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
    ) -> CheckResult {
        self.assume_checked(expression, nonzero, limits)
            .unwrap_or_else(CheckResult::Unknown)
    }
    /// Retain an assumption before checking its feasibility.
    ///
    /// `Err` leaves this state unchanged because the assumption did not fit its
    /// policy. `Ok(Unknown(_))` keeps the complete assumption: encoding or solver
    /// failure loses pruning precision without losing its guarded input domain.
    /// Coverage-sensitive callers must preserve this distinction. An already
    /// contradictory state returns `Ok(Unsat)` without needing another atom.
    pub fn assume_checked(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
    ) -> Result<CheckResult, QueryReason> {
        self.insert(
            Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            },
            limits,
        )
    }
    /// Branch-only cached feasibility; retention and full encoding still precede
    /// lookup. Rejected retention leaves the state unchanged, and only a proved
    /// contradiction installs bottom.
    pub(crate) fn assume_checked_cached(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
        cache: &mut embedded_smt::CacheQuery<'_>,
    ) -> Result<CheckResult, QueryReason> {
        self.insert_inner(
            Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            },
            limits,
            Some(cache),
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
        .unwrap_or_else(CheckResult::Unknown)
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
        .unwrap_or_else(CheckResult::Unknown)
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
        .unwrap_or_else(CheckResult::Unknown)
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
            .chain(self.path.expressions())
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
    /// Cached exact-candidate feasibility without retaining the candidate.
    /// The caller pays its original query fee before constructing the cache borrow.
    pub(crate) fn can_equal_cached(
        &self,
        expression: &ExprId,
        value: U256,
        limits: &RelationLimits,
        cache: &mut embedded_smt::CacheQuery<'_>,
    ) -> CheckResult {
        solver::check_cached(
            self,
            Some(Constraint::Equal {
                left: expression.clone(),
                right: ExprId::constant(value),
            }),
            limits,
            cache,
        )
    }
    /// Check the same complete conjunction for every candidate, in input order.
    /// A shared immutable encoding does not merge identities or query fuel.
    pub(crate) fn can_equal_candidates(
        &self,
        expression: &ExprId,
        candidates: &[U256],
        limits: &RelationLimits,
        budget: &mut embedded_smt::QueryBudget<'_>,
    ) -> Result<Vec<CheckResult>, CandidateError> {
        solver::candidates::check(self, expression, candidates, limits, budget)
    }
    /// Ask whether every satisfying valuation agrees on this expression.
    pub fn unique_value(&self, expression: &ExprId, limits: &RelationLimits) -> ValueQuery {
        solver::unique(self, expression, limits)
    }
    pub(crate) fn unique_value_cached(
        &self,
        expression: &ExprId,
        limits: &RelationLimits,
        cache: &mut embedded_smt::CacheQuery<'_>,
    ) -> ValueQuery {
        solver::unique_cached(self, expression, limits, cache)
    }
    /// Cover both paths by keeping only guarantees common to both.
    /// This intentionally loses disjunction precision, never intersects paths.
    pub fn join(&self, other: &Self) -> Self {
        if self == other {
            return self.clone();
        }
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
            path: if self.path == other.path {
                self.path.clone()
            } else {
                RelationPath::default()
            },
        }
    }
    /// Preserve both complete paths, selected by the same nonzero guard as values/states.
    /// Failure leaves both input states untouched; callers must retain the pending alternative.
    pub fn join_guarded(
        &self,
        other: &Self,
        selector: ExprId,
        limits: &RelationLimits,
    ) -> Result<Self, QueryReason> {
        if !limits.enabled {
            return Err(QueryReason::Disabled);
        }
        if !limits.valid() {
            return Err(QueryReason::Configuration);
        }
        if embedded_smt::current_cancellation().is_cancelled() {
            return Err(QueryReason::Cancelled);
        }
        let checked = |state: &Self| {
            if state.len() > limits.max_constraints {
                return Err(QueryReason::ConstraintLimit);
            }
            if state.work_size() > limits.max_nodes || state.path.depth() > limits.max_depth {
                return Err(QueryReason::ExpressionLimit);
            }
            if state
                .constraints
                .iter()
                .flat_map(Constraint::expressions)
                .any(|e| e.depth() > limits.max_depth)
            {
                return Err(QueryReason::ExpressionLimit);
            }
            Ok(())
        };
        if self == other {
            checked(self)?;
            return Ok(self.clone());
        }
        if let Some(value) = selector.as_constant() {
            let selected = if value == U256::ZERO { other } else { self };
            checked(selected)?;
            return Ok(selected.clone());
        }
        // These exact atoms hold on either alternative. Keep them once outside
        // the selector: ite(s, C && L, C && R) = C && ite(s, L, R).
        // Conditional atoms already in a path remain under their original guards.
        let common: BTreeSet<_> = self
            .constraints
            .intersection(&other.constraints)
            .cloned()
            .collect();
        let residual = |state: &Self| Self {
            constraints: state.constraints.difference(&common).cloned().collect(),
            bottom: state.bottom,
            path: state.path.clone(),
        };
        let path = RelationPath::guarded(
            RelationPath::from_state(&residual(self)),
            RelationPath::from_state(&residual(other)),
            selector,
            limits,
        )?;
        let joined = Self {
            constraints: common,
            bottom: self.bottom && other.bottom,
            path,
        };
        // The residual path's bound excludes the factored atoms. Apply the
        // complete-state policy before publishing either part of the result.
        checked(&joined)?;
        Ok(joined)
    }
    /// Complete immutable branch formula for read-only solver/report traversal.
    pub fn path(&self) -> &RelationPath {
        &self.path
    }
    /// Guarantees true on every retained path, suitable for scalar consumers.
    pub fn constraints(&self) -> impl Iterator<Item = &Constraint> {
        self.constraints.iter()
    }
    /// Widen by forgetting any guarantee absent from the previous state.
    pub fn widen(&self, previous: &Self) -> Self {
        self.join(previous)
    }
    /// Forget guarantees, preserving reachability rather than fabricating bottom.
    pub fn forget(&mut self) {
        self.constraints.clear();
        self.bottom = false;
        self.path = RelationPath::default();
    }
    /// Runtime leaves needing capture-avoiding renaming during summary replay.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        self.constraints
            .iter()
            .flat_map(|c| c.expressions())
            .chain(self.path.expressions())
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
            path: self.path.rename(map, limits)?,
        })
    }
}

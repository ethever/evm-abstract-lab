//! Bounded cross-value guarantees with local consequence propagation.
//!
//! Joins retain common guarantees or guarded alternatives. Only a proved local
//! contradiction prunes; bounds and cancellation preserve retained assumptions.
//! A consistent local approximation is not a satisfiability proof.

use super::{
    AlgebraRef, EvmNumericAlgebra, WordElement, WordObservation,
    symbolic::{ExprError, ExprId, ExprKind, ExprLimits},
};
use alloy_primitives::U256;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

mod local;
mod path;
mod scalar;
pub use path::{RelationPath, RelationPathKind};
pub use scalar::EvmScalarQuery;
#[cfg(test)]
mod tests;

/// Explicit bounded policy for retention and local consequence propagation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct RelationLimits {
    /// Enable relational queries; scalar/value identity precision is independent.
    pub enabled: bool,
    /// Maximum retained conjunction atoms.
    pub max_constraints: usize,
    /// Maximum expression traversal and propagation work in one query.
    pub max_nodes: usize,
    /// Maximum expression nesting.
    pub max_depth: usize,
}
impl Default for RelationLimits {
    fn default() -> Self {
        Self {
            enabled: true,
            max_constraints: 128,
            max_nodes: 1024,
            max_depth: 64,
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
    /// Zero limits are configuration errors, never unbounded traversal settings.
    pub fn valid(self) -> bool {
        self.max_constraints > 0 && self.max_nodes > 0 && self.max_depth > 0
    }
}

/// A real bound, configuration error or cancellation stopped local reasoning.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum QueryReason {
    /// Explicit caller cancellation; the controlled analysis stops.
    Cancelled,
    /// Relational refinement was explicitly disabled.
    Disabled,
    /// Invalid zero-sized policy.
    Configuration,
    /// The next assumption cannot fit the retained conjunction.
    ConstraintLimit,
    /// The bounded scalar fact table could not complete the projection.
    ScalarFactLimit,
    /// Local fact interpretation failed without proving numerical bottom.
    ScalarFactError(super::facts::FactError),
    /// Expression size, depth or local propagation work exceeds the policy.
    ExpressionLimit,
}

/// Only a proved contradiction justifies eliminating a path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum CheckResult {
    /// No contradiction was found; no satisfying valuation is claimed.
    MayBeFeasible,
    /// No valuation satisfies the retained guarantees.
    ProvenImpossible,
    /// Retain the path and all successfully retained assumptions.
    Undetermined(QueryReason),
}

/// A value obtained from sound bounded local consequences.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum EvmValueQuery<N: WordElement> {
    /// Every valuation of the retained guarantees has this value.
    Exact(U256),
    /// The state is proved impossible.
    ProvenImpossible,
    /// A sound local upper approximation; top is ordinary imprecision.
    Bounds(Box<N>),
    /// A real bound or cancellation prevented the query from completing.
    Undetermined(QueryReason),
}

/// Intersect two numeric upper approximations without changing expression identity.
/// The caller pays scalar reduction work before invoking this helper.
pub(crate) fn intersect_numeric<D: EvmNumericAlgebra>(
    left: &D::Element,
    right: &D::Element,
    domain: AlgebraRef<'_, D>,
) -> Option<D::Element> {
    intersect_numeric_with(left, right, domain, |_| {
        Ok::<_, std::convert::Infallible>(())
    })
    .unwrap_or_else(|never| match never {})
}

// Pure and staged callers share import/refusal semantics. The callback runs
// only for a feasible imported element, before its reduction or projection.
pub(super) fn intersect_numeric_with<D: EvmNumericAlgebra, E>(
    left: &D::Element,
    right: &D::Element,
    domain: AlgebraRef<'_, D>,
    pay_reduction: impl FnOnce(&D::Element) -> Result<(), E>,
) -> Result<Option<D::Element>, E> {
    let numeric = match left.constrain(&right.observation()) {
        Ok(numeric) => numeric,
        Err(
            super::facts::FactError::Contradiction { .. }
            | super::facts::FactError::ConflictingBits
            | super::facts::FactError::InvalidBounds
            | super::facts::FactError::EmptyFiniteSet,
        ) => return Ok(None),
        // Optional import failure is not a contradiction of this universe.
        Err(_) => return Ok(Some(left.clone())),
    };
    if numeric
        .constants()
        .is_some_and(|values| !values.iter().any(|v| numeric.contains(*v)))
    {
        return Ok(None);
    }
    pay_reduction(&numeric)?;
    let reduced = domain.numeric_algebra().reduce_numeric(&numeric);
    Ok((reduced.status != super::ReductionStatus::Empty)
        .then(|| domain.numeric_algebra().project_numeric(&reduced.value)))
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
    /// Import sound scalar guarantees without checking each component separately.
    /// Budget failure may leave a sound subset installed; it never creates bottom.
    pub fn observe<N: WordElement>(
        &mut self,
        expression: &ExprId,
        numeric: &N,
        limits: &RelationLimits,
    ) -> Result<(), QueryReason> {
        self.observe_observation(expression, &numeric.observation(), limits)
    }

    /// Retain guarantees about this exact expression/source occurrence.
    pub fn observe_observation(
        &mut self,
        expression: &ExprId,
        numeric: &WordObservation,
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
        // Comparison expressions are already exact 0/1 words in the numeric interpreter.
        // Reasserting their range, 255 zero bits and {0,1} membership adds
        // redundant constraints to the same bounded conjunction.
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
            // those again duplicates facts already carried by the finite set.
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
    /// An interrupted local check retains the new sound assumption; a capacity failure
    /// preserves the prior, weaker state instead of pruning the path.
    /// Use [`Self::assume_checked`] when coverage depends on retaining the guard.
    pub fn assume<D: EvmNumericAlgebra>(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        self.assume_checked(expression, nonzero, limits, domain)
            .unwrap_or_else(CheckResult::Undetermined)
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
        if analysis_control::current_cancellation().is_cancelled() {
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
    /// Complete immutable branch formula for read-only local/report traversal.
    pub fn path(&self) -> &RelationPath {
        &self.path
    }
    /// Guarantees true on every retained path, suitable for scalar consumers.
    pub fn constraints(&self) -> impl ExactSizeIterator<Item = &Constraint> {
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

impl RelationState {
    /// Evaluate using the same injected numeric policy as the current execution.
    pub fn check<D: EvmNumericAlgebra>(
        &self,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        match local::project(self, None, None, limits, None, domain) {
            Ok(None) => CheckResult::ProvenImpossible,
            Ok(Some(_)) => CheckResult::MayBeFeasible,
            Err(reason) => CheckResult::Undetermined(reason),
        }
    }

    /// Evaluate using the same injected numeric policy as the current execution.
    pub fn can_equal<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        value: U256,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        match local::project(self, Some(expression), Some(value), limits, None, domain) {
            Ok(None) => CheckResult::ProvenImpossible,
            Ok(Some(_)) => CheckResult::MayBeFeasible,
            Err(reason) => CheckResult::Undetermined(reason),
        }
    }

    /// Evaluate using the same injected numeric policy as the current execution.
    pub fn local_value<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValueQuery<D::Element> {
        match local::project(self, Some(expression), None, limits, None, domain) {
            Ok(None) => EvmValueQuery::ProvenImpossible,
            Ok(Some(value)) => value.singleton().map_or_else(
                || EvmValueQuery::Bounds(Box::new(value.clone())),
                EvmValueQuery::Exact,
            ),
            Err(reason) => EvmValueQuery::Undetermined(reason),
        }
    }

    /// Evaluate using the same injected numeric policy as the current execution.
    pub(crate) fn local_value_budgeted<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        limits: &RelationLimits,
        budget: &mut crate::resource::WorkBudget,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValueQuery<D::Element> {
        match local::project(self, Some(expression), None, limits, Some(budget), domain) {
            Ok(None) => EvmValueQuery::ProvenImpossible,
            Ok(Some(value)) => value.singleton().map_or_else(
                || EvmValueQuery::Bounds(Box::new(value.clone())),
                EvmValueQuery::Exact,
            ),
            Err(reason) => EvmValueQuery::Undetermined(reason),
        }
    }

    /// Evaluate using the same injected numeric policy as the current execution.
    pub(crate) fn can_equal_budgeted<D: EvmNumericAlgebra>(
        &self,
        expression: &ExprId,
        value: U256,
        limits: &RelationLimits,
        budget: &mut crate::resource::WorkBudget,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        match local::project(
            self,
            Some(expression),
            Some(value),
            limits,
            Some(budget),
            domain,
        ) {
            Ok(None) => CheckResult::ProvenImpossible,
            Ok(Some(_)) => CheckResult::MayBeFeasible,
            Err(reason) => CheckResult::Undetermined(reason),
        }
    }

    /// Evaluate using the same injected numeric policy as the current execution.
    pub(crate) fn assume_checked_budgeted<D: EvmNumericAlgebra>(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
        budget: &mut crate::resource::WorkBudget,
        domain: AlgebraRef<'_, D>,
    ) -> Result<CheckResult, QueryReason> {
        if self.bottom {
            return Ok(CheckResult::ProvenImpossible);
        }
        self.retain(
            Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            },
            limits,
        )?;
        let answer = match local::project(self, None, None, limits, Some(budget), domain) {
            Ok(None) => CheckResult::ProvenImpossible,
            Ok(Some(_)) => CheckResult::MayBeFeasible,
            Err(reason) => CheckResult::Undetermined(reason),
        };
        if answer == CheckResult::ProvenImpossible {
            self.bottom = true;
        }
        Ok(answer)
    }

    /// Retain an assumption before checking its feasibility.
    ///
    /// `Err` leaves this state unchanged because the assumption did not fit its
    /// policy. `Ok(Undetermined(_))` keeps the complete assumption: a traversal
    /// bound loses pruning precision without losing its guarded input domain.
    /// Coverage-sensitive callers must preserve this distinction. An already
    /// contradictory state returns `Ok(ProvenImpossible)` without needing another atom.
    pub fn assume_checked<D: EvmNumericAlgebra>(
        &mut self,
        expression: &ExprId,
        nonzero: bool,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> Result<CheckResult, QueryReason> {
        self.insert(
            Constraint::Truth {
                expression: expression.clone(),
                nonzero,
            },
            limits,
            domain,
        )
    }

    /// Retain this scoped guarantee and check through the injected algebra.
    pub fn add_unsigned_bounds<D: EvmNumericAlgebra>(
        &mut self,
        expression: &ExprId,
        lower: U256,
        upper: U256,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        self.insert(
            Constraint::Range {
                expression: expression.clone(),
                lower,
                upper,
            },
            limits,
            domain,
        )
        .unwrap_or_else(CheckResult::Undetermined)
    }

    /// Retain this scoped guarantee and check through the injected algebra.
    pub fn add_known_bits<D: EvmNumericAlgebra>(
        &mut self,
        expression: &ExprId,
        zero: U256,
        one: U256,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        self.insert(
            Constraint::Bits {
                expression: expression.clone(),
                zero,
                one,
            },
            limits,
            domain,
        )
        .unwrap_or_else(CheckResult::Undetermined)
    }

    /// Retain this scoped guarantee and check through the injected algebra.
    pub fn add_equal<D: EvmNumericAlgebra>(
        &mut self,
        left: &ExprId,
        right: &ExprId,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> CheckResult {
        self.insert(
            Constraint::Equal {
                left: left.clone(),
                right: right.clone(),
            },
            limits,
            domain,
        )
        .unwrap_or_else(CheckResult::Undetermined)
    }
    fn insert<D: EvmNumericAlgebra>(
        &mut self,
        constraint: Constraint,
        limits: &RelationLimits,
        domain: AlgebraRef<'_, D>,
    ) -> Result<CheckResult, QueryReason> {
        if self.bottom {
            return Ok(CheckResult::ProvenImpossible);
        }
        self.retain(constraint, limits)?;
        let checked = self.check(limits, domain);
        if checked == CheckResult::ProvenImpossible {
            self.bottom = true;
        }
        Ok(checked)
    }
}

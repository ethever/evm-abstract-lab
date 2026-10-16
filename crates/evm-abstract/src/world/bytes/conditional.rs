//! Branch-local operand guarantees, separate from unconditional must facts.
//!
//! Joins retain only the changed facts below their selector. Shared subtrees are
//! immutable; lookup follows determined guards and never scans byte-state roots.

mod context;
mod length;
pub(super) use context::Context;
pub(super) use context::{CONTEXTS, Preparation, preparation};

use super::{RangeError, native};
use crate::domain::{
    AbstractValue, Domain,
    relational::{QueryReason, RelationLimits, RelationState},
    symbolic::{ExprError, ExprId, ExprLimits},
};
use alloy_primitives::U256;
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq},
};
use std::{collections::BTreeMap, sync::Arc};

type Facts = BTreeMap<ExprId, AbstractValue>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct ConditionalFacts {
    branches: Vec<Arc<Branch>>,
    entries: usize,
    depth: usize,
    work: usize,
    largest_expression: usize,
    limited: bool,
    lookup_scopes: usize,
    lookup_numeric_work: usize,
    fact_entries: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Branch {
    condition: ExprId,
    then_facts: Scope,
    else_facts: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct Scope {
    #[serde(serialize_with = "native::serialize_facts")]
    facts: Facts,
    conditional: ConditionalFacts,
}

impl Serialize for ConditionalFacts {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let limits = crate::domain::symbolic::GraphLimits::default();
        self.check(ExprLimits {
            max_nodes: limits.max_nodes,
            max_depth: limits.max_depth,
        })
        .map_err(S::Error::custom)?;
        if self.work > limits.max_nodes {
            return Err(S::Error::custom(
                "conditional operand facts exceed native metadata policy",
            ));
        }
        let mut sequence = serializer.serialize_seq(Some(self.branches.len()))?;
        for branch in &self.branches {
            sequence.serialize_element(branch.as_ref())?;
        }
        sequence.end()
    }
}

impl ConditionalFacts {
    fn from_branches(branches: Vec<Arc<Branch>>) -> Self {
        let mut out = Self {
            branches,
            ..Self::default()
        };
        for branch in &out.branches {
            out.entries = out.entries.saturating_add(1);
            out.work = out.work.saturating_add(branch.condition.work_size());
            out.largest_expression = out.largest_expression.max(branch.condition.nodes());
            let mut depth = branch.condition.depth();
            let mut lookup_scopes = 0;
            let mut lookup_numeric_work = 0;
            for scope in [&branch.then_facts, &branch.else_facts] {
                out.fact_entries = out
                    .fact_entries
                    .saturating_add(scope.facts.len())
                    .saturating_add(scope.conditional.fact_entries);
                lookup_scopes = lookup_scopes.max(
                    scope
                        .conditional
                        .lookup_scopes
                        .saturating_add(usize::from(!scope.facts.is_empty())),
                );
                out.entries = out
                    .entries
                    .saturating_add(scope.facts.len().saturating_mul(2))
                    .saturating_add(scope.conditional.entries);
                out.work = out.work.saturating_add(scope.conditional.work);
                out.largest_expression = out
                    .largest_expression
                    .max(scope.conditional.largest_expression);
                out.limited |= scope.conditional.limited;
                depth = depth.max(scope.conditional.depth);
                let mut numeric_work = 0;
                for (expression, value) in &scope.facts {
                    numeric_work = numeric_work.max(value.numeric().work_size());
                    out.work = out
                        .work
                        .saturating_add(expression.work_size())
                        .saturating_add(value.work_size());
                    depth = depth.max(expression.depth());
                    depth = depth.max(value.expression().map_or(0, ExprId::depth));
                    out.largest_expression = out
                        .largest_expression
                        .max(expression.nodes())
                        .max(value.expression().map_or(0, ExprId::nodes));
                    out.limited |= value.symbolic_limit_reached();
                }
                lookup_numeric_work = lookup_numeric_work.max(
                    scope
                        .conditional
                        .lookup_numeric_work
                        .saturating_add(numeric_work),
                );
            }
            out.depth = out.depth.max(depth.saturating_add(1));
            out.lookup_scopes = out.lookup_scopes.saturating_add(lookup_scopes);
            out.lookup_numeric_work = out.lookup_numeric_work.saturating_add(lookup_numeric_work);
        }
        out
    }

    pub(super) fn join(
        left: (&Facts, &Self),
        right: (&Facts, &Self),
        common: &Facts,
        condition: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        let scope = |(facts, conditional): (&Facts, &Self)| Scope {
            facts: facts
                .iter()
                .filter(|(expression, value)| common.get(*expression) != Some(*value))
                .map(|(expression, value)| (expression.clone(), value.clone()))
                .collect(),
            conditional: conditional.clone(),
        };
        let then_facts = scope(left);
        let else_facts = scope(right);
        let out = if then_facts.facts.is_empty()
            && else_facts.facts.is_empty()
            && then_facts.conditional == else_facts.conditional
        {
            then_facts.conditional
        } else {
            Self::from_branches(vec![Arc::new(Branch {
                condition,
                then_facts,
                else_facts,
            })])
        };
        out.check(limits)?;
        Ok(out)
    }

    /// Copy captures both snapshots' implications. No guard is selected and no
    /// branch-local bound becomes an unconditional guarantee of its source.
    pub(super) fn extend(&mut self, other: &Self, limits: ExprLimits) -> Result<(), ExprError> {
        let mut branches = self.branches.clone();
        for branch in &other.branches {
            if !branches.iter().any(|old| Arc::ptr_eq(old, branch)) {
                branches.push(Arc::clone(branch));
            }
        }
        let out = Self::from_branches(branches);
        out.check(limits)?;
        *self = out;
        Ok(())
    }

    pub(super) fn check(&self, limits: ExprLimits) -> Result<(), ExprError> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled);
        }
        let nodes = if self.limited {
            usize::MAX
        } else {
            self.entries.max(self.largest_expression)
        };
        if nodes > limits.max_nodes || self.depth > limits.max_depth {
            return Err(ExprError::Limit {
                nodes,
                depth: self.depth,
            });
        }
        Ok(())
    }

    pub(super) fn len(&self) -> usize {
        self.entries
    }

    pub(super) fn work_size(&self) -> usize {
        self.work
    }

    /// Conservative fallback for readers without a prepared metadata demand.
    pub(super) fn lookup_work(&self, domain: Domain) -> usize {
        self.lookup_scopes
            .saturating_mul(
                domain
                    .refinement_work(&AbstractValue::top(), 1)
                    .saturating_add(1),
            )
            .saturating_add(self.lookup_numeric_work.saturating_mul(2))
    }

    pub(super) fn symbolic_limit_reached(&self) -> bool {
        self.limited
    }

    // Each shared branch is visited once for metadata operations. Reads instead
    // use lookup below, which stops immediately at an undetermined selector.
    pub(super) fn visit(&self, visit: &mut impl FnMut(&ExprId, Option<&AbstractValue>)) {
        let mut seen = std::collections::BTreeSet::new();
        let mut pending = self.branches.iter().collect::<Vec<_>>();
        while let Some(branch) = pending.pop() {
            if !seen.insert(Arc::as_ptr(branch)) {
                continue;
            }
            visit(&branch.condition, None);
            for scope in [&branch.then_facts, &branch.else_facts] {
                for (expression, value) in &scope.facts {
                    visit(expression, Some(value));
                }
                pending.extend(&scope.conditional.branches);
            }
        }
    }

    pub(super) fn map(
        &self,
        map: &mut impl FnMut(
            &ExprId,
            Option<&AbstractValue>,
        ) -> Result<(ExprId, Option<AbstractValue>), RangeError>,
    ) -> Result<Self, RangeError> {
        fn mapped(
            source: &ConditionalFacts,
            map: &mut impl FnMut(
                &ExprId,
                Option<&AbstractValue>,
            ) -> Result<(ExprId, Option<AbstractValue>), RangeError>,
            memo: &mut BTreeMap<*const Branch, ConditionalFacts>,
        ) -> Result<ConditionalFacts, RangeError> {
            let mut branches = Vec::new();
            for branch in &source.branches {
                if let Some(old) = memo.get(&Arc::as_ptr(branch)) {
                    branches.extend(old.branches.iter().map(Arc::clone));
                    continue;
                }
                let condition = map(&branch.condition, None)?.0;
                let mut scope = |source: &Scope| -> Result<Scope, RangeError> {
                    let mut facts = Facts::new();
                    for (expression, value) in &source.facts {
                        let (expression, value) = map(expression, Some(value))?;
                        if let Some(mut value) = value {
                            if let Some(constant) = expression.as_constant() {
                                value = Domain::default()
                                    .join(&value, &AbstractValue::constant(constant))
                                    .with_numeric(crate::domain::NumericValue::constant(constant));
                            }
                            // Renaming may merge keys; keep every possible source.
                            facts
                                .entry(expression)
                                .and_modify(|old| *old = Domain::default().join(old, &value))
                                .or_insert(value);
                        }
                    }
                    Ok(Scope {
                        facts,
                        conditional: mapped(&source.conditional, map, memo)?,
                    })
                };
                let out = if let Some(value) = condition.as_constant() {
                    // A selected scope is the whole implication now. Do not map
                    // the inactive subtree: its retired selectors and errors
                    // cannot participate in this projected state.
                    let selected = scope(if value == U256::ZERO {
                        &branch.else_facts
                    } else {
                        &branch.then_facts
                    })?;
                    if selected.facts.is_empty() {
                        // Avoid accumulating constant wrappers around a shared
                        // active subtree across repeated joins/projections.
                        selected.conditional
                    } else {
                        // Keep selected guarantees scoped; do not promote them
                        // into the separate unconditional must-fact table.
                        ConditionalFacts::from_branches(vec![Arc::new(Branch {
                            condition: ExprId::constant(U256::ONE),
                            then_facts: selected,
                            else_facts: Scope {
                                facts: Facts::new(),
                                conditional: ConditionalFacts::default(),
                            },
                        })])
                    }
                } else {
                    ConditionalFacts::from_branches(vec![Arc::new(Branch {
                        condition,
                        then_facts: scope(&branch.then_facts)?,
                        else_facts: scope(&branch.else_facts)?,
                    })])
                };
                branches.extend(out.branches.iter().map(Arc::clone));
                memo.insert(Arc::as_ptr(branch), out);
            }
            Ok(ConditionalFacts::from_branches(branches))
        }
        mapped(self, map, &mut BTreeMap::new())
    }

    pub(super) fn relations(
        &self,
        mut incoming: RelationState,
        limits: &RelationLimits,
    ) -> Result<RelationState, QueryReason> {
        for branch in &self.branches {
            let scope = |scope: &Scope| {
                let mut facts = incoming.clone();
                for (expression, value) in &scope.facts {
                    facts.observe(expression, value.numeric(), limits)?;
                }
                scope.conditional.relations(facts, limits)
            };
            incoming = scope(&branch.then_facts)?.join_guarded(
                &scope(&branch.else_facts)?,
                branch.condition.clone(),
                limits,
            )?;
        }
        Ok(incoming)
    }
}

impl super::ByteArray {
    /// Complete numeric guarantees, including branch-local bounds under their
    /// original guards. `symbolic_facts` deliberately exports must facts only.
    /// This builds a bounded formula; it performs no feasibility query.
    pub fn symbolic_relations(
        &self,
        limits: &RelationLimits,
    ) -> Result<RelationState, QueryReason> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(QueryReason::Cancelled);
        }
        if !limits.enabled {
            return Err(QueryReason::Disabled);
        }
        if !limits.valid() {
            return Err(QueryReason::Configuration);
        }
        if self.failure.is_some() {
            return Err(QueryReason::ExpressionLimit);
        }
        self.conditional
            .check(limits.expressions())
            .map_err(|error| {
                if error == ExprError::Cancelled {
                    QueryReason::Cancelled
                } else {
                    QueryReason::ExpressionLimit
                }
            })?;
        let mut out = RelationState::default();
        for (expression, value) in &self.operands {
            out.observe(expression, value.numeric(), limits)?;
        }
        self.conditional.relations(out, limits)
    }
}

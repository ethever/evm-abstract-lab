//! Temporary metadata only. A prepared word demand pays for this entire arena
//! before construction; its node counter bounds probes, comparisons and copies.

use super::Context;
use crate::domain::{
    AbstractValue, Domain, DomainSpec,
    symbolic::{ExprError, ExprId, ExprLimits},
};
use std::{cell::Cell, collections::BTreeMap};

// The whole demand, including its symbolic and numeric walks, shares these
// bounds. Exhaustion refuses only this optional attempt; the paid caller
// retries with the complete unprepared algorithm before returning a value.
pub(in crate::world::bytes) const CONTEXTS: usize = 32;
const POLICIES: usize = 2;

pub(super) struct Demand {
    pub(super) limits: ExprLimits,
    policies: [DomainSpec; POLICIES],
    maximum: usize,
    used: Cell<usize>,
    contexts: Cell<usize>,
    maximum_key_work: Cell<usize>,
    #[cfg(test)]
    source_key_visits: Cell<usize>,
    refused: Cell<bool>,
}

impl Demand {
    pub(super) fn new(domain: Domain, maximum: usize, maximum_key_work: usize) -> Self {
        let spec = domain.spec();
        Self {
            limits: spec.relations().expressions(),
            policies: [
                spec,
                spec.with_relations(crate::domain::relational::RelationLimits {
                    enabled: false,
                    ..spec.relations()
                }),
            ],
            maximum,
            used: Cell::new(0),
            contexts: Cell::new(1),
            maximum_key_work: Cell::new(maximum_key_work),
            #[cfg(test)]
            source_key_visits: Cell::new(0),
            refused: Cell::new(false),
        }
    }

    pub(super) fn include_key(&self, work: usize) {
        self.maximum_key_work
            .set(self.maximum_key_work.get().max(work));
    }

    pub(super) fn source_key(&self, work: usize) {
        self.include_key(work);
        #[cfg(test)]
        self.source_key_visits.set(self.source_key_visits.get() + 1);
    }

    #[cfg(test)]
    pub(super) fn source_key_visits(&self) -> usize {
        self.source_key_visits.get()
    }

    pub(super) fn policy(&self, domain: Domain) -> bool {
        if !self.policies.contains(&domain.spec()) {
            self.refuse();
            return false;
        }
        !self.refused() && self.check().is_ok()
    }

    pub(super) fn check(&self) -> Result<(), ExprError> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled);
        }
        Ok(())
    }

    pub(super) fn refuse(&self) {
        self.refused.set(true);
    }

    pub(super) fn refused(&self) -> bool {
        self.refused.get()
    }

    pub(super) fn reserve(&self, nodes: usize) -> bool {
        if self.refused() || self.check().is_err() {
            return false;
        }
        let Some(next) = self.used.get().checked_add(nodes) else {
            self.refuse();
            return false;
        };
        if next > self.maximum {
            self.refuse();
            return false;
        }
        self.used.set(next);
        true
    }

    pub(super) fn context(&self, nodes: usize, depth: usize) -> bool {
        if self.contexts.get() >= CONTEXTS
            || nodes > self.limits.max_nodes
            || depth > self.limits.max_depth
        {
            self.refuse();
            return false;
        }
        if !self.reserve(nodes.saturating_add(1)) {
            return false;
        }
        self.contexts.set(self.contexts.get() + 1);
        true
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> usize {
        self.used.get()
    }
}

#[derive(Default)]
struct Policy {
    values: BTreeMap<ExprId, Option<AbstractValue>>,
    // Missing keys have no intersections to repeat. One complete negative
    // result is retained; other misses repeat their fully paid map probes.
    absent: Option<ExprId>,
    work: usize,
}

#[derive(Default)]
pub(super) struct Memo<'a> {
    policies: Vec<(DomainSpec, Policy)>,
    // The retained condition in each entry keeps this pointer key alive. A
    // structurally equal different allocation may miss, never falsely hit.
    pub(super) branches: BTreeMap<(usize, bool), (ExprId, Context<'a>)>,
}

impl Memo<'_> {
    pub(super) fn get(
        &self,
        expression: &ExprId,
        domain: Domain,
        demand: &Demand,
        reserve: &impl Fn(usize) -> bool,
    ) -> Option<Option<AbstractValue>> {
        if !reserve(POLICIES) {
            return None;
        }
        let (_, policy) = self
            .policies
            .iter()
            .find(|(spec, _)| *spec == domain.spec())?;
        if let Some(absent) = &policy.absent {
            if !reserve(expression.comparison_work(absent)) {
                return None;
            }
            if expression == absent {
                return Some(None);
            }
        }
        if !probe(policy.values.len(), expression, demand, reserve) {
            return None;
        }
        let value = policy.values.get(expression)?;
        if !reserve(value.as_ref().map_or(1, AbstractValue::work_size)) {
            return None;
        }
        Some(value.clone())
    }

    pub(super) fn insert(
        &mut self,
        expression: &ExprId,
        value: &Option<AbstractValue>,
        domain: Domain,
        demand: &Demand,
        reserve: &impl Fn(usize) -> bool,
    ) {
        if !reserve(POLICIES) {
            return;
        }
        let index = match self
            .policies
            .iter()
            .position(|(spec, _)| *spec == domain.spec())
        {
            Some(index) => index,
            None if self.policies.len() < POLICIES => {
                if !reserve(1) {
                    return;
                }
                self.policies.push((domain.spec(), Policy::default()));
                self.policies.len() - 1
            }
            None => {
                demand.refuse();
                return;
            }
        };
        let policy = &mut self.policies[index].1;
        let Some(value) = value else {
            if reserve(
                expression
                    .work_size()
                    .saturating_add(policy.absent.as_ref().map_or(0, ExprId::work_size))
                    .saturating_add(1),
            ) {
                policy.absent = Some(expression.clone());
            }
            return;
        };
        let work = expression.work_size().saturating_add(value.work_size());
        // Positive keys belong only to the immutable source maps or
        // bounded choices, so all of them are retained until demand completion.
        if !probe(policy.values.len(), expression, demand, reserve)
            || !reserve(
                policy
                    .work
                    .saturating_add(value.work_size())
                    .saturating_add(1),
            )
        {
            return;
        }
        policy
            .values
            .insert(expression.clone(), Some(value.clone()));
        policy.work = policy.work.saturating_add(work);
    }
}

/// Reserve a B-tree search from cached key sizes, without visiting its keys.
/// Rust 1.99's B-tree has at most eleven keys per node. Its height is at most
/// one plus log2(len + 1); scanning all keys is a second independent upper bound.
/// The frozen demand's maximum covers source keys and newly retained choices,
/// including structural comparisons when expression fingerprints collide.
/// Taking a length instead of a map makes a planning key scan impossible here.
pub(super) fn probe(
    len: usize,
    expression: &ExprId,
    demand: &Demand,
    reserve: &impl Fn(usize) -> bool,
) -> bool {
    let levels = (len.saturating_add(1).ilog2() as usize).saturating_add(1);
    let comparisons = len.min(11usize.saturating_mul(levels));
    reserve(
        comparisons
            .saturating_mul(
                expression
                    .work_size()
                    .saturating_add(demand.maximum_key_work.get())
                    .saturating_add(1),
            )
            .saturating_add(1),
    )
}

pub(in crate::world::bytes) struct Preparation {
    pub(in crate::world::bytes) nodes: usize,
    pub(in crate::world::bytes) work: usize,
}

/// Pure size arithmetic, with no fact merges or allocating preparation. The
/// transfer reserves `work` before entering the checked read. Numeric meets
/// are bounded separately from the metered metadata walk: for a key with n
/// source facts there are at most n-1 scope meets plus one guard-truth meet.
pub(in crate::world::bytes) fn preparation(
    root_work: usize,
    offset: &AbstractValue,
    fact_work: usize,
    fact_count: usize,
    conditional: &super::ConditionalFacts,
    domain: Domain,
) -> Preparation {
    let metadata = conditional
        .work_size()
        .saturating_add(fact_work)
        .saturating_add(offset.work_size().saturating_mul(2))
        .saturating_add(1);
    let nodes = root_work
        .saturating_add(offset.work_size())
        .saturating_add(1)
        .saturating_mul(32)
        .saturating_mul(
            metadata
                .saturating_add(domain.capacity())
                .saturating_add(32),
        );
    let facts = fact_count
        .saturating_add(1)
        .saturating_add(conditional.fact_entries);
    let meets = CONTEXTS.saturating_mul(POLICIES).saturating_mul(facts);
    let reduction = domain
        .refinement_work(&AbstractValue::top(), 1)
        // Scope merging also preserves the original numeric/metadata join;
        // its fixed-width congruence gcd is separate from the meet/reduction.
        .saturating_add(4096)
        .saturating_add(metadata.saturating_mul(2))
        .saturating_add(domain.capacity())
        .saturating_add(32);
    Preparation {
        nodes,
        work: nodes.saturating_add(meets.saturating_mul(reduction)),
    }
}

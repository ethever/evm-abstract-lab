//! Demand-local metadata: N bounds probes, comparisons and copies independently
//! of payment. Each admitted action owes the same original WorkBudget first.

use super::Context;
use crate::domain::AlgebraRef;
use crate::domain::{
    DomainSpec, EvmValue,
    symbolic::{ExprError, ExprId, ExprLimits},
};
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::resource::WorkBudget;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
};

// The whole demand, including its symbolic and numeric walks, shares these
// bounds. Arena/node/depth refusal retries the fully paid original algorithm;
// a failed live metadata or numeric payment stops the entire read as Work.
// The initial root counts once; at most 31 children are retained per demand.
// Complete temporary children still pay every action under the same N bound.
pub(in crate::world::bytes) const CONTEXTS: usize = 32;
const POLICIES: usize = 2;

// Four Rc shell construction/release pairs (choices, initial active, memo,
// selected active) and four fixed field-handle retention/release pairs. This
// finite per-Context tariff includes child scaffolding, not payload copies.
pub(super) const CONTEXT_LIFETIME_WORK: usize = 4 * 4;
// The shared payer Rc is constructed and released only once per demand.
pub(super) const DEMAND_LIFETIME_WORK: usize = 2;

enum Payer<'a> {
    Budget(RefCell<&'a mut WorkBudget>),
    // Existing private probes remain unmetered reference oracles only.
    #[cfg(test)]
    Oracle,
}

pub(super) struct Demand<'a, D: EvmNumericAlgebra> {
    pub(super) domain: AlgebraRef<'a, D>,
    pub(super) limits: ExprLimits,
    policies: [DomainSpec; POLICIES],
    maximum: usize,
    used: Cell<usize>,
    retained_contexts: Cell<usize>,
    maximum_key_work: Cell<usize>,
    #[cfg(test)]
    source_key_visits: Cell<usize>,
    refused: Cell<bool>,
    payer: Payer<'a>,
    reduction_work: usize,
    work_exhausted: Cell<bool>,
    cancelled: Cell<bool>,
    #[cfg(test)]
    cancel_after_payments: RefCell<Option<(usize, analysis_control::Cancellation)>>,
    // Accepted action receipts only: true denotes a numeric primitive.
    #[cfg(test)]
    payments: RefCell<Vec<(bool, usize)>>,
    // Completed payment checkpoints followed by actual Reader algebra calls.
    #[cfg(test)]
    reader_calls: Cell<(usize, usize)>,
}

impl<'a, D: EvmNumericAlgebra> Demand<'a, D> {
    pub(super) fn new(
        domain: AlgebraRef<'a, D>,
        preparation: &Preparation,
        maximum_key_work: usize,
        budget: &'a mut WorkBudget,
    ) -> Self {
        let exhausted = budget.exhausted();
        let out = Self::with_payer(
            domain,
            preparation.nodes,
            maximum_key_work,
            preparation.reduction_work,
            Payer::Budget(RefCell::new(budget)),
        );
        out.work_exhausted.set(exhausted);
        out
    }

    #[cfg(test)]
    pub(super) fn oracle(
        domain: AlgebraRef<'a, D>,
        maximum: usize,
        maximum_key_work: usize,
    ) -> Self {
        Self::with_payer(domain, maximum, maximum_key_work, 0, Payer::Oracle)
    }

    fn with_payer(
        domain: AlgebraRef<'a, D>,
        maximum: usize,
        maximum_key_work: usize,
        reduction_work: usize,
        payer: Payer<'a>,
    ) -> Self {
        let spec = domain.spec();
        Self {
            domain,
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
            retained_contexts: Cell::new(1),
            maximum_key_work: Cell::new(maximum_key_work),
            #[cfg(test)]
            source_key_visits: Cell::new(0),
            refused: Cell::new(false),
            payer,
            reduction_work,
            work_exhausted: Cell::new(false),
            cancelled: Cell::new(false),
            #[cfg(test)]
            cancel_after_payments: RefCell::new(None),
            #[cfg(test)]
            payments: RefCell::new(Vec::new()),
            #[cfg(test)]
            reader_calls: Cell::new((0, 0)),
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

    pub(super) fn policy(&self, domain: AlgebraRef<'_, D>) -> bool {
        if !std::ptr::eq(*self.domain.numeric_algebra(), *domain.numeric_algebra())
            || !self.policies.contains(&domain.spec())
        {
            self.refuse();
            return false;
        }
        !self.refused() && !self.work_exhausted() && self.check().is_ok()
    }

    pub(super) fn check(&self) -> Result<(), ExprError> {
        if analysis_control::current_cancellation().is_cancelled() {
            self.cancelled.set(true);
        }
        if self.cancelled.get() {
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

    pub(super) fn work_exhausted(&self) -> bool {
        self.work_exhausted.get()
    }

    /// Borrow only for payment, never across an action or cancellation hook.
    /// All clones share the original ledger and sticky financial refusal.
    fn charge(&self, work: usize) -> bool {
        if self.check().is_err() || self.work_exhausted() || self.refused() {
            return false;
        }
        let paid = match &self.payer {
            Payer::Budget(budget) => {
                let mut budget = budget.borrow_mut();
                !budget.exhausted() && budget.charge(work)
            }
            #[cfg(test)]
            Payer::Oracle => true,
        };
        // A cancellation observed during charge remains a typed interruption.
        if self.check().is_err() {
            return false;
        }
        if !paid {
            self.work_exhausted.set(true);
        }
        paid
    }

    pub(super) fn pay_metadata(&self, work: usize) -> bool {
        if !self.charge(work) {
            return false;
        }
        #[cfg(test)]
        self.payments.borrow_mut().push((false, work));
        true
    }

    pub(super) fn pay(&self, work: usize) -> bool {
        if !self.charge(work) {
            return false;
        }
        #[cfg(test)]
        self.payments.borrow_mut().push((true, work));
        // Existing controls cancel during numeric work, never an earlier
        // metadata payment. No live ledger borrow crosses this checkpoint.
        #[cfg(test)]
        {
            let mut cancellation = self.cancel_after_payments.borrow_mut();
            if let Some((remaining, token)) = cancellation.as_mut() {
                *remaining -= 1;
                if *remaining == 0 {
                    token.cancel();
                    *cancellation = None;
                }
            }
        }
        self.check().is_ok()
    }

    pub(super) fn intersect(&self) -> bool {
        self.pay(self.reduction_work)
    }

    /// Reader transfers are distinct from Context's intersections and joins.
    /// The reference oracle keeps its old metadata cap without Reader fees.
    pub(super) fn pay_reader(&self, price: impl FnOnce() -> usize) -> bool {
        match &self.payer {
            Payer::Budget(_) => self.pay(price()),
            #[cfg(test)]
            Payer::Oracle => true,
        }
    }

    #[cfg(test)]
    pub(super) fn record_reader_call(&self, join: bool) {
        let (applies, joins) = self.reader_calls.get();
        self.reader_calls
            .set((applies + usize::from(!join), joins + usize::from(join)));
    }

    #[cfg(test)]
    pub(super) fn reader_calls(&self) -> (usize, usize) {
        self.reader_calls.get()
    }

    pub(super) fn reserve(&self, nodes: usize) -> bool {
        self.reserve_priced(nodes, nodes)
    }

    /// Retained payloads prepay their final release. A replaced negative key
    /// already owns that release, so its unchanged N term is not paid twice.
    pub(super) fn reserve_owned(&self, nodes: usize, release: usize, replaced: usize) -> bool {
        self.reserve_priced(
            nodes,
            nodes.saturating_sub(replaced).saturating_add(release),
        )
    }

    fn reserve_priced(&self, nodes: usize, work: usize) -> bool {
        if self.check().is_err() || self.work_exhausted() || self.refused() {
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
        // Optional capacity refusal owes no action payment. Once admitted,
        // payment must succeed before either the action or used is committed.
        if !self.pay_metadata(work) {
            return false;
        }
        self.used.set(next);
        true
    }

    /// Every temporary child owes the original admission work independently of
    /// whether its complete Context can also be retained in a branch memo.
    pub(super) fn context(&self, nodes: usize, depth: usize) -> bool {
        if nodes > self.limits.max_nodes || depth > self.limits.max_depth {
            self.refuse();
            return false;
        }
        self.reserve(nodes.saturating_add(1))
    }

    pub(super) fn can_retain_context(&self) -> bool {
        self.retained_contexts.get() < CONTEXTS
    }

    /// Called only after a complete child was successfully inserted. Demand
    /// owns no Context, so the retained parent-to-child memo graph stays acyclic.
    pub(super) fn record_retained_context(&self) {
        debug_assert!(self.can_retain_context());
        self.retained_contexts.set(self.retained_contexts.get() + 1);
    }

    #[cfg(test)]
    pub(super) fn used(&self) -> usize {
        self.used.get()
    }

    #[cfg(test)]
    pub(super) fn retained_contexts(&self) -> usize {
        self.retained_contexts.get()
    }

    #[cfg(test)]
    pub(super) fn paid_work(&self) -> usize {
        match &self.payer {
            Payer::Budget(budget) => budget.borrow().used(),
            Payer::Oracle => 0,
        }
    }

    #[cfg(test)]
    pub(super) fn payments(&self) -> Vec<(bool, usize)> {
        self.payments.borrow().clone()
    }

    #[cfg(test)]
    pub(super) fn cancel_after_payments(
        &self,
        count: usize,
        token: analysis_control::Cancellation,
    ) {
        assert!(count > 0);
        *self.cancel_after_payments.borrow_mut() = Some((count, token));
    }
}

struct Policy<N: WordElement> {
    values: BTreeMap<ExprId, Option<EvmValue<N>>>,
    // Missing keys have no intersections to repeat. One complete negative
    // result is retained; other misses repeat their fully paid map probes.
    absent: Option<ExprId>,
    work: usize,
}

pub(super) struct Memo<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> {
    marker: std::marker::PhantomData<&'a D>,
    policies: Vec<(DomainSpec, Policy<N>)>,
    // The retained condition in each entry keeps this pointer key alive. A
    // structurally equal different allocation may miss, never falsely hit.
    pub(super) branches: BTreeMap<(usize, bool), (ExprId, Context<'a, N, D>)>,
}

impl<N: WordElement, D: EvmNumericAlgebra<Element = N>> Memo<'_, N, D> {
    pub(super) fn get(
        &self,
        expression: &ExprId,
        domain: AlgebraRef<'_, D>,
        demand: &Demand<'_, D>,
        reserve: &impl Fn(usize) -> bool,
    ) -> Option<Option<EvmValue<N>>> {
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
        if !reserve(value.as_ref().map_or(1, EvmValue::<N>::work_size)) {
            return None;
        }
        Some(value.clone())
    }

    pub(super) fn insert(
        &mut self,
        expression: &ExprId,
        value: &Option<EvmValue<N>>,
        domain: AlgebraRef<'_, D>,
        demand: &Demand<'_, D>,
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
                if !demand.reserve_owned(1, 1, 0) {
                    return;
                }
                self.policies.push((domain.spec(), Policy::<N>::default()));
                self.policies.len() - 1
            }
            None => {
                demand.refuse();
                return;
            }
        };
        let policy = &mut self.policies[index].1;
        let Some(value) = value else {
            let old = policy.absent.as_ref().map_or(0, ExprId::work_size);
            if demand.reserve_owned(
                expression.work_size().saturating_add(old).saturating_add(1),
                expression.work_size(),
                old,
            ) {
                policy.absent = Some(expression.clone());
            }
            return;
        };
        let work = expression.work_size().saturating_add(value.work_size());
        // Positive keys belong only to the immutable source maps or
        // bounded choices, so all of them are retained until demand completion.
        if !probe(policy.values.len(), expression, demand, reserve)
            || !demand.reserve_owned(
                policy
                    .work
                    .saturating_add(value.work_size())
                    .saturating_add(1),
                value.work_size().saturating_add(1),
                0,
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
/// The frozen demand's maximum covers source keys and every constructed choice,
/// including structural comparisons when expression fingerprints collide.
/// Taking a length instead of a map makes a planning key scan impossible here.
pub(super) fn probe<D: EvmNumericAlgebra>(
    len: usize,
    expression: &ExprId,
    demand: &Demand<'_, D>,
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
    // Retain the capacity receipt independently of the actual payment ledger.
    #[cfg(test)]
    pub(in crate::world::bytes) work: usize,
    pub(super) reduction_work: usize,
}

/// A real join still runs after the equal/Top meet shortcuts. Use the same
/// fixed-width gcd and B-tree insertion tariff as the byte writer, plus
/// expression comparisons and materialization of joined components. The caller
/// and memo's own AV snapshots remain arena work. Policies without the product shortcut certificate receive the conservative
/// candidate-rebuilding envelope, including one congruence fold per candidate.
pub(super) fn join_work<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    domain: AlgebraRef<'_, D>,
    left: &EvmValue<N>,
    right: &EvmValue<N>,
) -> usize {
    let candidates = match (left.constants(), right.constants()) {
        (Some(left), Some(right)) => left.len().saturating_add(right.len()),
        _ => 0,
    };
    // The sum also covers the first capacity-overflow insertion before Top.
    let height = (usize::BITS - candidates.leading_zeros()) as usize + 1;
    let origins = match (
        left.provenance().origins().sources(),
        right.provenance().origins().sources(),
    ) {
        (Some(left), Some(right)) => left.len().saturating_add(right.len()),
        _ => 0,
    };
    let origin_height = (usize::BITS - origins.leading_zeros()) as usize + 1;
    let retained = candidates.min(domain.capacity());
    let (congruence_joins, summaries) =
        if !domain.numeric_algebra().supports_product_scalar_shortcuts() {
            (
                retained.saturating_sub(1).max(1),
                retained.saturating_mul(256),
            )
        } else {
            (1, 0)
        };
    4096usize
        .saturating_mul(congruence_joins)
        .saturating_add(candidates.saturating_mul(height).saturating_mul(256))
        .saturating_add(origins.saturating_mul(origin_height))
        .saturating_add(summaries)
        .saturating_add(
            left.work_size()
                .saturating_add(right.work_size())
                .saturating_mul(2),
        )
        .saturating_add(32)
}

/// Pure size arithmetic, with no fact merges or allocating preparation. The
/// node receipt is a hard scratch cap, not an upfront financial reservation.
/// Metadata and the original numeric bound are paid at their actual actions.
pub(in crate::world::bytes) fn preparation<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root_work: usize,
    offset: &EvmValue<N>,
    fact_work: usize,
    _fact_count: usize,
    conditional: &super::ConditionalFacts<N>,
    domain: AlgebraRef<'_, D>,
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
    let reduction = domain
        .refinement_work(&EvmValue::<N>::top(), 1)
        // Keep the original generic meet/reduction bound and numeric scratch
        // allowance. Its separate 4096 join gcd now belongs only to join_work.
        // Metadata is a cached upper bound on the numeric operand/result sizes;
        // Caller-side probes and operand/memo copies pay their own N-sized actions;
        // copies internal to numeric primitives belong to their action tariff.
        .saturating_add(metadata.saturating_mul(2))
        .saturating_add(domain.capacity())
        .saturating_add(32);
    Preparation {
        nodes,
        #[cfg(test)]
        work: nodes,
        reduction_work: reduction,
    }
}

impl<N: WordElement> Default for Policy<N> {
    fn default() -> Self {
        Self {
            values: BTreeMap::new(),
            absent: None,
            work: 0,
        }
    }
}
impl<'a, N: WordElement, D: EvmNumericAlgebra<Element = N>> Default for Memo<'a, N, D> {
    fn default() -> Self {
        Self {
            marker: std::marker::PhantomData,
            policies: Vec::new(),
            branches: BTreeMap::new(),
        }
    }
}

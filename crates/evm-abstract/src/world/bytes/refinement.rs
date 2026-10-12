//! Project externally proved path guards into bounded state metadata.
//!
//! The caller owns proof queries and their work ledger. This module never obtains
//! a model or changes immutable content, source identities, or historical reads.

use super::{ByteArray, RangeError, numeric};
use crate::{
    domain::{
        AbstractValue, Domain, DomainSpec, Profile,
        relational::{QueryReason, RelationLimits, RelationState, ScalarQuery},
        symbolic::{
            ByteStateId, ByteStateKind, ExprError, ExprId, ExprKind, ExprLimits, SlotStateId,
            SlotStateKind,
        },
    },
    world::StoreError,
};
use alloy_primitives::U256;
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroUsize,
};
use thiserror::Error;

/// Checked metadata projection or the caller's actual proof failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GuardRefinementError<E> {
    /// Existing byte state, cancellation, or metadata traversal policy failed.
    #[error("{0}")]
    Bytes(RangeError),
    /// Existing account state failed its retention policy.
    #[error("{0}")]
    Store(StoreError),
    /// Local scalar intersection could not retain a proved guard.
    #[error("guard fact projection failed: {0:?}")]
    Facts(QueryReason),
    /// The caller did not complete its budgeted proof.
    #[error("guard proof failed: {0:?}")]
    Proof(E),
}

impl<E> From<RangeError> for GuardRefinementError<E> {
    fn from(error: RangeError) -> Self {
        Self::Bytes(error)
    }
}

impl<E> From<StoreError> for GuardRefinementError<E> {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::world) enum Node {
    Expression(ExprId),
    Bytes(ByteStateId),
    Slots(SlotStateId),
}

/// Visit distinct graph metadata only. Exact payloads and logical COPY lengths
/// contribute no per-byte work to conditional fact projection.
pub(in crate::world) fn guards(
    mut pending: Vec<Node>,
    limits: ExprLimits,
) -> Result<BTreeSet<ExprId>, ExprError> {
    let mut seen = BTreeSet::new();
    let mut out = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if embedded_smt::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled);
        }
        if !seen.insert(node.clone()) {
            continue;
        }
        let depth = match &node {
            Node::Expression(e) => e.depth(),
            Node::Bytes(b) => b.depth(),
            Node::Slots(s) => s.depth(),
        };
        if seen.len() > limits.max_nodes || depth > limits.max_depth {
            return Err(ExprError::Limit {
                nodes: seen.len(),
                depth,
            });
        }
        match node {
            Node::Expression(e) => match e.kind() {
                ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_) => {}
                ExprKind::Operation { args, .. } => {
                    pending.extend(args.iter().cloned().map(Node::Expression));
                }
                ExprKind::Ite {
                    condition,
                    then_value,
                    else_value,
                } => {
                    out.insert(condition.clone());
                    pending.extend(
                        [condition, then_value, else_value].map(|e| Node::Expression(e.clone())),
                    );
                }
                ExprKind::ByteRead { state, index } => {
                    pending.extend([Node::Bytes(state.clone()), Node::Expression(index.clone())]);
                }
                ExprKind::WordRead { view, offset } => {
                    pending.push(Node::Expression(offset.clone()));
                    pending.extend(view_nodes(view));
                }
                ExprKind::SlotRead { state, key } => {
                    pending.extend([Node::Slots(state.clone()), Node::Expression(key.clone())]);
                }
                ExprKind::Keccak { input } => pending.extend(view_nodes(input)),
            },
            Node::Bytes(b) => match b.kind() {
                ByteStateKind::Zero | ByteStateKind::Exact(_) => {}
                ByteStateKind::Input {
                    source,
                    length,
                    bindings,
                } => {
                    pending.extend([
                        Node::Expression(source.token().clone()),
                        Node::Expression(length.clone()),
                    ]);
                    pending.extend(bindings.iter().map(|b| Node::Expression(b.value.clone())));
                }
                ByteStateKind::WriteByte {
                    previous,
                    offset,
                    value,
                }
                | ByteStateKind::WriteWord {
                    previous,
                    offset,
                    value,
                } => {
                    pending.extend([
                        Node::Bytes(previous.clone()),
                        Node::Expression(offset.clone()),
                        Node::Expression(value.clone()),
                    ]);
                }
                ByteStateKind::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => {
                    pending.extend([
                        Node::Bytes(previous.clone()),
                        Node::Expression(destination.clone()),
                        Node::Expression(length.clone()),
                    ]);
                    pending.extend(view_nodes(source));
                }
                ByteStateKind::Slice { source } => pending.extend(view_nodes(source)),
                ByteStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => {
                    out.insert(condition.clone());
                    pending.extend([
                        Node::Expression(condition.clone()),
                        Node::Bytes(then_state.clone()),
                        Node::Bytes(else_state.clone()),
                    ]);
                }
            },
            Node::Slots(s) => match s.kind() {
                SlotStateKind::Zero => {}
                SlotStateKind::Input { source, bindings } => {
                    pending.push(Node::Expression(source.token().clone()));
                    pending.extend(bindings.iter().map(|b| Node::Expression(b.value.clone())));
                }
                SlotStateKind::Store {
                    previous,
                    key,
                    value,
                } => {
                    pending.extend([
                        Node::Slots(previous.clone()),
                        Node::Expression(key.clone()),
                        Node::Expression(value.clone()),
                    ]);
                }
                SlotStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => {
                    out.insert(condition.clone());
                    pending.extend([
                        Node::Expression(condition.clone()),
                        Node::Slots(then_state.clone()),
                        Node::Slots(else_state.clone()),
                    ]);
                }
            },
        }
    }
    Ok(out)
}

fn view_nodes(view: &crate::domain::symbolic::ByteView) -> [Node; 3] {
    [
        Node::Bytes(view.state.clone()),
        Node::Expression(view.offset.clone()),
        Node::Expression(view.length.clone()),
    ]
}

/// Intersect every old numeric component with a truth already proved by the
/// caller. `observe`/`refine_numeric` here import facts without a native query.
pub(in crate::world) fn retain_truth<E>(
    facts: &mut BTreeMap<ExprId, AbstractValue>,
    condition: &ExprId,
    nonzero: bool,
    domain: Domain,
    limits: ExprLimits,
) -> Result<bool, GuardRefinementError<E>> {
    let old = facts
        .get(condition)
        .cloned()
        .unwrap_or_else(AbstractValue::top);
    let bound = if nonzero {
        AbstractValue::unsigned_range(U256::ONE, U256::MAX).expect("nonempty word range")
    } else {
        AbstractValue::constant(U256::ZERO)
    };
    let policy = RelationLimits {
        enabled: true,
        max_nodes: limits.max_nodes,
        max_depth: limits.max_depth,
        ..domain.spec().relations()
    };
    let mut guarantee = RelationState::default();
    guarantee
        .observe(condition, bound.numeric(), &policy)
        .map_err(GuardRefinementError::Facts)?;
    let spec = domain.spec();
    // Guard truth is semantic metadata even with a constant-only numeric profile.
    let fact_domain = Domain::from_spec(
        DomainSpec::new(
            Profile::Product,
            NonZeroUsize::new(spec.capacity()).expect("validated capacity"),
            NonZeroUsize::new(spec.reduction_rounds()).expect("validated reduction rounds"),
            NonZeroUsize::new(spec.fact_limit()).expect("validated fact limit"),
        )
        .with_relations(policy),
    );
    match guarantee.refine_numeric(condition, old.numeric(), fact_domain, &policy) {
        ScalarQuery::Refined { numeric, .. } => {
            facts.insert(
                condition.clone(),
                old.with_numeric(*numeric)
                    .with_expression(condition.clone()),
            );
            Ok(true)
        }
        ScalarQuery::Infeasible => Ok(false),
        ScalarQuery::Unknown(reason) => Err(GuardRefinementError::Facts(reason)),
        ScalarQuery::Unchanged => Err(GuardRefinementError::Facts(QueryReason::ScalarFactLimit)),
    }
}

pub(in crate::world) fn refresh_scalar(
    value: &mut AbstractValue,
    facts: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
) {
    if let Some(expression) = value.expression()
        && let Some(exact) =
            numeric::scalar(expression, facts, numeric::numeric_domain(domain)).singleton()
        && value.contains(exact)
    {
        *value = value
            .clone()
            .with_numeric(crate::domain::NumericValue::constant(exact));
    }
}

impl ByteArray {
    /// Apply only caller-proved guard truth. None retains both alternatives;
    /// false means the imported facts prove the whole state infeasible. Errors
    /// leave this wrapper unchanged. The callback owns query/budget diagnostics.
    pub fn refine_guards<E>(
        &mut self,
        domain: Domain,
        limits: ExprLimits,
        proof: &mut impl FnMut(&ExprId) -> Result<Option<bool>, E>,
    ) -> Result<bool, GuardRefinementError<E>> {
        self.check_existing(limits)?;
        let mut pending = vec![Node::Bytes(self.root.clone())];
        self.visit_values(&mut |v| {
            if let Some(e) = v.expression() {
                pending.push(Node::Expression(e.clone()));
            }
        });
        let conditions = guards(pending, limits).map_err(RangeError::from)?;
        let mut out = self.clone();
        for condition in conditions {
            if let Some(nonzero) = proof(&condition).map_err(GuardRefinementError::Proof)?
                && !retain_truth(&mut out.operands, &condition, nonzero, domain, limits)?
            {
                return Ok(false);
            }
        }
        refresh_scalar(&mut out.length, &out.operands, domain);
        out.refresh(domain);
        out.check_existing(limits)?;
        *self = out;
        Ok(true)
    }
}

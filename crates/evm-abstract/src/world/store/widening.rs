//! Preserve complete account cases while discarding redundant cycle metadata.

use super::{InitialIndependence, Plane, Store, StoreError};
use crate::{
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprError, ExprLimits, SlotStateId, SlotStateKind},
    },
    world::bytes::{
        numeric,
        refinement::Node,
        widening::{refresh_constant, retained_facts},
    },
};
use std::collections::BTreeMap;

fn scalar_nodes(store: &Store) -> Vec<Node> {
    let mut pending = Vec::new();
    for value in store
        .balances
        .values()
        .chain(store.nonces.values())
        .chain([&store.balance_default])
    {
        if let Some(expression) = value.expression() {
            pending.push(Node::Expression(expression.clone()));
        }
    }
    for log in store.possible_logs.values() {
        pending.push(Node::Bytes(log.data.state_root().clone()));
        for value in log.topics.iter().chain([log.data.len()]) {
            if let Some(expression) = value.expression() {
                pending.push(Node::Expression(expression.clone()));
            }
        }
    }
    pending
}

impl Plane {
    fn default_upper(
        &self,
        root: &SlotStateId,
        domain: Domain,
        memo: &mut BTreeMap<SlotStateId, AbstractValue>,
    ) -> Result<AbstractValue, StoreError> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        if let Some(value) = memo.get(root) {
            return Ok(value.clone());
        }
        let domain = numeric::numeric_domain(domain);
        let value = match root.kind() {
            SlotStateKind::Zero => numeric::scalar(
                &crate::domain::symbolic::ExprId::constant(alloy_primitives::U256::ZERO),
                &self.operands,
                domain,
            ),
            SlotStateKind::Input { .. } => AbstractValue::top(),
            SlotStateKind::Store {
                previous, value, ..
            } => domain.join(
                &self.default_upper(previous, domain, memo)?,
                &numeric::scalar(value, &self.operands, domain),
            ),
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                let truth = numeric::scalar(condition, &self.operands, domain);
                if !truth.may_be_nonzero() {
                    self.default_upper(else_state, domain, memo)?
                } else if !truth.may_be_zero() {
                    self.default_upper(then_state, domain, memo)?
                } else {
                    domain.join(
                        &self.default_upper(then_state, domain, memo)?,
                        &self.default_upper(else_state, domain, memo)?,
                    )
                }
            }
        };
        memo.insert(root.clone(), value.clone());
        Ok(value)
    }

    fn widening_form(
        &self,
        scalars: &[Node],
        domain: Domain,
        limits: ExprLimits,
    ) -> Result<Self, StoreError> {
        let mut pending = scalars.to_vec();
        pending.push(Node::Slots(self.base.clone()));
        pending.extend(self.roots.values().cloned().map(Node::Slots));
        let mut out = self.clone();
        out.operands = retained_facts(pending, &self.operands, limits)?;
        // Discard branch-narrowed cache projections and reread the same roots
        // using only the external facts retained above.
        for (address, key) in self.slots.keys().copied() {
            let value = out.read_checked(address, &AbstractValue::constant(key), domain)?;
            out.slots.insert((address, key), value);
        }
        let mut memo = BTreeMap::new();
        out.global_default = out.default_upper(&out.base, domain, &mut memo)?;
        for address in self.defaults.keys().chain(self.roots.keys()).copied() {
            out.defaults.insert(
                address,
                out.default_upper(&out.root_at(address)?, domain, &mut memo)?,
            );
        }
        out.initial_independence = InitialIndependence::new(false);
        Ok(out)
    }
}

impl Store {
    /// Canonical whole-case metadata, with source facts and rollback roots intact.
    /// This deliberately forgets must-independence rather than adding RPC facts.
    pub(crate) fn widening_form(
        &self,
        domain: Domain,
        limits: ExprLimits,
    ) -> Result<Self, StoreError> {
        self.check_existing(limits)?;
        let scalars = scalar_nodes(self);
        let mut out = self.clone();
        out.persistent = self.persistent.widening_form(&scalars, domain, limits)?;
        out.transient = self.transient.widening_form(&scalars, domain, limits)?;
        out.balances.update_values(refresh_constant);
        out.nonces.update_values(refresh_constant);
        refresh_constant(&mut out.balance_default);
        for key in self.possible_logs.keys() {
            let log = out.possible_logs.get_mut(key).expect("retained event key");
            for topic in &mut log.topics {
                refresh_constant(topic);
            }
            log.data = log.data.widening_form(domain, limits)?;
        }
        out.check_existing(limits)?;
        Ok(out)
    }
}

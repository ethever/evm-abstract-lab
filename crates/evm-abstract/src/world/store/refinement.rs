//! Conditional numeric projection keeps immutable account roots and RPC facts.

use super::{EvmStore, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::{
    domain::symbolic::{ExprId, ExprLimits},
    world::{
        GuardRefinementError,
        bytes::refinement::{Node, guards, refresh_scalar, retain_truth},
    },
};
use std::collections::{BTreeMap, BTreeSet};

impl<N: WordElement> EvmStore<N> {
    /// Collect distinct ITE selectors from both storage planes, account scalars
    /// and log data under the existing limits, without changing this wrapper or
    /// initial observations. Proofs apply to this same frozen state.
    pub(crate) fn guard_conditions(
        &self,
        limits: ExprLimits,
    ) -> Result<BTreeSet<ExprId>, StoreError> {
        self.check_existing(limits)?;
        let mut pending = Vec::new();
        for plane in [&self.persistent, &self.transient] {
            pending.push(Node::Slots(plane.base.clone()));
            pending.extend(plane.roots.values().cloned().map(Node::Slots));
        }
        for log in self.possible_logs.values() {
            pending.push(Node::Bytes(log.data.state_root().clone()));
        }
        self.visit_values(&mut |value| {
            if let Some(expression) = value.expression() {
                pending.push(Node::Expression(expression.clone()));
            }
        });
        let mut out = guards(pending, limits).map_err(StoreError::from)?;
        for log in self.possible_logs.values() {
            log.data.extend_conditional_guards(&mut out);
        }
        Ok(out)
    }

    /// Project caller-proved zero/nonzero selectors through both planes and log
    /// data. No proof result becomes an initial/RPC observation. False means
    /// infeasible; any error leaves the transaction checkpoint unchanged.
    pub fn refine_guards<E, D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        domain: AlgebraRef<'_, D>,
        limits: ExprLimits,
        proof: &mut impl FnMut(&ExprId) -> Result<Option<bool>, E>,
    ) -> Result<bool, GuardRefinementError<E>> {
        let conditions = self.guard_conditions(limits)?;
        let mut truths = BTreeMap::new();
        for condition in conditions {
            truths.insert(
                condition.clone(),
                proof(&condition).map_err(GuardRefinementError::Proof)?,
            );
        }
        let mut out = self.clone();
        for plane in [&mut out.persistent, &mut out.transient] {
            for (condition, truth) in &truths {
                if let Some(nonzero) = truth
                    && !retain_truth(&mut plane.operands, condition, *nonzero, domain, limits)?
                {
                    return Ok(false);
                }
            }
            for (address, key) in plane.slots.keys().copied().collect::<Vec<_>>() {
                let value = plane.read_checked(
                    address,
                    &crate::domain::EvmValue::<N>::constant(key),
                    domain,
                )?;
                plane.slots.insert((address, key), value);
            }
        }
        out.balances
            .update_values(|v| refresh_scalar(v, &out.persistent.operands, domain));
        out.nonces
            .update_values(|v| refresh_scalar(v, &out.persistent.operands, domain));
        refresh_scalar(&mut out.balance_default, &out.persistent.operands, domain);
        for key in out.possible_logs.keys().cloned().collect::<Vec<_>>() {
            let log = out.possible_logs.get_mut(&key).expect("retained event key");
            for topic in &mut log.topics {
                refresh_scalar(topic, &out.persistent.operands, domain);
            }
            if !log.data.refine_guards(domain, limits, &mut |condition| {
                Ok::<_, E>(truths.get(condition).copied().flatten())
            })? {
                return Ok(false);
            }
        }
        out.check_existing(limits)?;
        *self = out;
        Ok(true)
    }
}

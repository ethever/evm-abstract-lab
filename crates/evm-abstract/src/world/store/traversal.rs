//! Summary renaming covers slot bases, account roots, and event byte views.

use super::{EvmStore, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::EvmNumericAlgebra;
use crate::domain::WordElement;
use crate::domain::{
    EvmValue,
    symbolic::{ExprId, ExprLimits},
};
use std::collections::{BTreeMap, BTreeSet};

impl<N: WordElement> EvmStore<N> {
    /// Import proved numeric guarantees using the immutable original term keys.
    /// Compact operand values need not retain an optional expression themselves.
    pub(crate) fn refine_operands(&mut self, update: &mut impl FnMut(&ExprId, &mut EvmValue<N>)) {
        for plane in [&mut self.persistent, &mut self.transient] {
            for (expression, value) in &mut plane.operands {
                update(expression, value);
            }
        }
    }

    /// Runtime symbols from roots, source tokens, scalar accounts and event views.
    pub fn fresh_leaves(&self) -> BTreeSet<ExprId> {
        let mut out = BTreeSet::new();
        for plane in [&self.persistent, &self.transient] {
            out.extend(plane.base.fresh_leaves());
            for root in plane.roots.values() {
                out.extend(root.fresh_leaves());
            }
        }
        self.visit_values(&mut |v| {
            if let Some(e) = v.expression() {
                out.extend(e.fresh_leaves());
            }
        });
        out
    }
    /// Apply one map to the whole store. Any error leaves the savepoint untouched.
    pub fn rename_fresh_with<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), StoreError> {
        let mut out = self.clone();
        for plane in [&mut out.persistent, &mut out.transient] {
            plane.base = plane.base.rename_fresh(map, limits)?;
            let mut roots = super::OrderedMap::new();
            for (address, root) in &plane.roots {
                roots.insert(*address, root.rename_fresh(map, limits)?);
            }
            plane.roots = roots;
            for (key, value) in plane
                .slots
                .iter()
                .map(|(key, value)| (*key, value.clone()))
                .collect::<Vec<_>>()
            {
                let mut value = value;
                value.rename_fresh(map, limits)?;
                plane.slots.insert(key, value);
            }
            for (key, value) in plane
                .defaults
                .iter()
                .map(|(key, value)| (*key, value.clone()))
                .collect::<Vec<_>>()
            {
                let mut value = value;
                value.rename_fresh(map, limits)?;
                plane.defaults.insert(key, value);
            }
            plane.global_default.rename_fresh(map, limits)?;
            let previous = std::mem::take(&mut plane.operands);
            for (e, v) in previous {
                let mut v = v;
                v.rename_fresh(map, limits)?;
                crate::world::bytes::operands::remember(
                    &mut plane.operands,
                    e.rename_fresh(map, limits)?,
                    &v,
                    domain,
                    limits,
                )?;
            }
        }
        for (address, value) in out
            .balances
            .iter()
            .map(|(a, v)| (*a, v.clone()))
            .collect::<Vec<_>>()
        {
            let mut v = value;
            v.rename_fresh(map, limits)?;
            out.balances.insert(address, v);
        }
        for (address, value) in out
            .nonces
            .iter()
            .map(|(a, v)| (*a, v.clone()))
            .collect::<Vec<_>>()
        {
            let mut v = value;
            v.rename_fresh(map, limits)?;
            out.nonces.insert(address, v);
        }
        out.balance_default.rename_fresh(map, limits)?;
        for (key, log) in out
            .possible_logs
            .iter()
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect::<Vec<_>>()
        {
            let mut log = log;
            for topic in &mut log.topics {
                topic.rename_fresh(map, limits)?;
            }
            log.data.rename_fresh_with(map, limits, domain)?;
            out.possible_logs.insert(key, log);
        }
        *self = out;
        Ok(())
    }
    /// A retained state or scalar failed symbolic construction within its policy.
    pub fn symbolic_limit_reached(&self) -> bool {
        let mut failed = self.persistent.failure.is_some() || self.transient.failure.is_some();
        self.visit_values(&mut |value| failed |= value.symbolic_limit_reached());
        failed
    }
}

impl EvmStore<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<(), StoreError> {
        self.rename_fresh_with(
            map,
            limits,
            crate::domain::ValueAlgebra::new(crate::domain::Domain::default()).borrowed(),
        )
    }
}

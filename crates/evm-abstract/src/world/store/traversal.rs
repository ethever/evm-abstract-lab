//! Summary renaming covers slot bases, account roots, and event byte views.

use super::{EvmStore, OrderedMap, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::EvmNumericAlgebra;
use crate::domain::WordElement;
use crate::domain::{
    EvmValue,
    symbolic::{ExprId, ExprLimits},
};
use std::collections::{BTreeMap, BTreeSet};

fn try_update_values<K: Ord + Clone, V: Clone>(
    values: &mut OrderedMap<K, V>,
    mut update: impl FnMut(&mut V) -> Result<(), StoreError>,
) -> Result<(), StoreError> {
    // Collect only keys so fallible mutation uses either backend's COW-aware API.
    let keys: Vec<_> = values.keys().cloned().collect();
    for key in keys {
        update(values.get_mut(&key).expect("collected key remains present"))?;
    }
    Ok(())
}

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
        let rename_value = |value: &mut EvmValue<N>| {
            value.rename_fresh(map, limits)?;
            Ok(())
        };
        for plane in [&mut out.persistent, &mut out.transient] {
            plane.base = plane.base.rename_fresh(map, limits)?;
            try_update_values(&mut plane.roots, |root| {
                *root = root.rename_fresh(map, limits)?;
                Ok(())
            })?;
            try_update_values(&mut plane.slots, rename_value)?;
            try_update_values(&mut plane.defaults, rename_value)?;
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
            plane.conditional = plane.conditional.map(domain, &mut |expression, value| {
                let mut value = value.cloned();
                if let Some(value) = &mut value {
                    value.rename_fresh(map, limits)?;
                }
                Ok((expression.rename_fresh(map, limits)?, value))
            })?;
            plane.conditional.check(limits)?;
        }
        try_update_values(&mut out.balances, rename_value)?;
        try_update_values(&mut out.nonces, rename_value)?;
        out.balance_default.rename_fresh(map, limits)?;
        try_update_values(&mut out.possible_logs, |log| {
            for topic in &mut log.topics {
                rename_value(topic)?;
            }
            log.data.rename_fresh_with(map, limits, domain)?;
            Ok(())
        })?;
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

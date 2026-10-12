//! Summary renaming covers slot bases, account roots, and event byte views.

use super::{Store, StoreError};
use crate::domain::{
    AbstractValue,
    symbolic::{ExprId, ExprLimits},
};
use std::collections::{BTreeMap, BTreeSet};

fn rename(
    value: &mut AbstractValue,
    map: &BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
) -> Result<(), StoreError> {
    if let Some(expression) = value.expression() {
        *value = value
            .clone()
            .with_expression(expression.rename_fresh(map, limits)?);
    }
    Ok(())
}
impl Store {
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
    pub fn rename_fresh(
        &mut self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
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
                rename(&mut value, map, limits)?;
                plane.slots.insert(key, value);
            }
            for (key, value) in plane
                .defaults
                .iter()
                .map(|(key, value)| (*key, value.clone()))
                .collect::<Vec<_>>()
            {
                let mut value = value;
                rename(&mut value, map, limits)?;
                plane.defaults.insert(key, value);
            }
            rename(&mut plane.global_default, map, limits)?;
            plane.operands = plane
                .operands
                .iter()
                .map(|(e, v)| {
                    let mut v = v.clone();
                    rename(&mut v, map, limits)?;
                    Ok((e.rename_fresh(map, limits)?, v))
                })
                .collect::<Result<_, StoreError>>()?;
        }
        for (address, value) in out
            .balances
            .iter()
            .map(|(a, v)| (*a, v.clone()))
            .collect::<Vec<_>>()
        {
            let mut v = value;
            rename(&mut v, map, limits)?;
            out.balances.insert(address, v);
        }
        for (address, value) in out
            .nonces
            .iter()
            .map(|(a, v)| (*a, v.clone()))
            .collect::<Vec<_>>()
        {
            let mut v = value;
            rename(&mut v, map, limits)?;
            out.nonces.insert(address, v);
        }
        rename(&mut out.balance_default, map, limits)?;
        for (key, log) in out
            .possible_logs
            .iter()
            .map(|(k, l)| (k.clone(), l.clone()))
            .collect::<Vec<_>>()
        {
            let mut log = log;
            for topic in &mut log.topics {
                rename(topic, map, limits)?;
            }
            log.data.rename_fresh(map, limits)?;
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

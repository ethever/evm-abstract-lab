//! Ordered slot roots and numeric upper bounds, partitioned by actual account.

use super::{Plane, Store, StoreError};
use crate::domain::{
    AbstractValue, Domain,
    symbolic::{ExprId, ExprLimits, SlotStateId, SlotStateKind, SourceId, StateBinding},
};
use crate::world::bytes::{numeric, operands};
use alloy_primitives::{Address, U256};
use std::collections::{BTreeMap, BTreeSet};

fn limits(domain: Domain) -> ExprLimits {
    domain.spec().relations().expressions()
}
fn expression(value: &AbstractValue) -> Result<ExprId, StoreError> {
    crate::world::bytes::expr(value).map_err(|error| match error {
        crate::world::RangeError::Expression(error) => StoreError::Expression(error),
        crate::world::RangeError::State(error) => StoreError::State(error),
        error => StoreError::Bytes(error),
    })
}
fn account_base(root: &SlotStateId, address: Address) -> Result<SlotStateId, StoreError> {
    Ok(match root.kind() {
        SlotStateKind::Input { source, .. } => SlotStateId::input(SourceId::scoped(
            source.token().clone(),
            U256::from_be_slice(address.as_slice()) << 8 | source.namespace(),
        )),
        SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } => account_base(then_state, address)?.join_guarded(
            &account_base(else_state, address)?,
            condition.clone(),
            ExprLimits {
                max_nodes: root.nodes(),
                max_depth: root.depth(),
            }
            .into(),
        )?,
        _ => root.clone(),
    })
}
fn key_disjoint(a: &AbstractValue, b: &AbstractValue) -> bool {
    let (alo, ahi) = a.interval().unsigned_bounds();
    let (blo, bhi) = b.interval().unsigned_bounds();
    ahi < blo
        || bhi < alo
        || match (a.constants(), b.constants()) {
            (Some(a), Some(b)) => a.is_disjoint(b),
            _ => false,
        }
}
pub(super) fn read_numeric(
    root: &SlotStateId,
    key: &ExprId,
    facts: &BTreeMap<ExprId, AbstractValue>,
    domain: Domain,
) -> AbstractValue {
    let domain = numeric::numeric_domain(domain);
    match root.kind() {
        SlotStateKind::Zero => numeric::scalar(&ExprId::constant(U256::ZERO), facts, domain),
        SlotStateKind::Input { bindings, .. } => {
            let key = numeric::scalar(key, facts, domain);
            if let Some(keys) = key.constants() {
                let mut out = None;
                for k in keys {
                    let value = bindings.iter().find(|b| b.index == *k).map_or_else(
                        AbstractValue::top,
                        |b| {
                            numeric::scalar(&b.value, facts, domain).with_numeric(b.numeric.clone())
                        },
                    );
                    out = Some(out.map_or_else(|| value.clone(), |old| domain.join(&old, &value)));
                }
                out.unwrap_or_else(AbstractValue::top)
            } else {
                AbstractValue::top()
            }
        }
        SlotStateKind::Store {
            previous,
            key: written,
            value,
        } => {
            if key == written {
                return numeric::scalar(value, facts, domain);
            }
            let requested = numeric::scalar(key, facts, domain);
            let stored = numeric::scalar(written, facts, domain);
            if let (Some(requested), Some(stored)) = (requested.singleton(), stored.singleton()) {
                return if requested == stored {
                    numeric::scalar(value, facts, domain)
                } else {
                    read_numeric(previous, key, facts, domain)
                };
            }
            if key_disjoint(&requested, &stored) {
                read_numeric(previous, key, facts, domain)
            } else {
                domain.join(
                    &read_numeric(previous, key, facts, domain),
                    &numeric::scalar(value, facts, domain),
                )
            }
        }
        SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } => {
            let c = numeric::scalar(condition, facts, domain);
            if !c.may_be_zero() {
                read_numeric(then_state, key, facts, domain)
            } else if !c.may_be_nonzero() {
                read_numeric(else_state, key, facts, domain)
            } else {
                domain.join(
                    &read_numeric(then_state, key, facts, domain),
                    &read_numeric(else_state, key, facts, domain),
                )
            }
        }
    }
}

pub(super) fn expressions(root: &SlotStateId) -> Vec<ExprId> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![root.clone()];
    while let Some(root) = pending.pop() {
        if !seen.insert(root.id()) {
            continue;
        }
        match root.kind() {
            SlotStateKind::Zero => {}
            SlotStateKind::Input { source, bindings } => {
                out.push(source.token().clone());
                out.extend(bindings.iter().map(|b| b.value.clone()));
            }
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => {
                pending.push(previous.clone());
                out.extend([key.clone(), value.clone()]);
            }
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => {
                pending.extend([then_state.clone(), else_state.clone()]);
                out.push(condition.clone());
            }
        }
    }
    out
}
impl Plane {
    pub(super) fn root_at(&self, address: Address) -> Result<SlotStateId, StoreError> {
        self.roots
            .get(&address)
            .cloned()
            .map_or_else(|| account_base(&self.base, address), Ok)
    }
    pub(super) fn initialize_roots(&mut self, limits: ExprLimits) {
        for address in self.defaults.keys().copied().collect::<Vec<_>>() {
            let complete = self.default_at(address).singleton() == Some(U256::ZERO);
            let mut root = if complete {
                SlotStateId::zero()
            } else {
                match self.root_at(address) {
                    Ok(root) => root,
                    Err(error) => {
                        self.failure = Some(error);
                        return;
                    }
                }
            };
            let mut bindings = Vec::new();
            for ((_, key), value) in self
                .slots
                .range((address, U256::ZERO)..=(address, U256::MAX))
            {
                let value_expr = match expression(value) {
                    Ok(e) => e,
                    Err(e) => {
                        self.failure = Some(e);
                        return;
                    }
                };
                if let Err(error) = operands::remember(
                    &mut self.operands,
                    value_expr.clone(),
                    value,
                    Domain::default(),
                    limits,
                ) {
                    self.failure = Some(error.into());
                    return;
                }
                if complete {
                    // A zero baseline still needs actual numeric constraints on
                    // symbolic observed values. An auxiliary observed cell binds
                    // the value; it does not turn unobserved complete slots unknown.
                    let value_expr = if value_expr.as_constant().is_none() {
                        let source = match account_base(&self.base, address) {
                            Ok(base) => match base.kind() {
                                SlotStateKind::Input { source, .. } => source.clone(),
                                _ => unreachable!("initial persistent input base"),
                            },
                            Err(error) => {
                                self.failure = Some(error);
                                return;
                            }
                        };
                        let observed = match SlotStateId::observed(
                            source,
                            vec![StateBinding {
                                index: *key,
                                value: value_expr,
                                numeric: value.numeric().clone(),
                            }],
                            limits.into(),
                        ) {
                            Ok(root) => root,
                            Err(error) => {
                                self.failure = Some(error.into());
                                return;
                            }
                        };
                        match ExprId::slot_read(observed, ExprId::constant(*key), limits) {
                            Ok(expression) => {
                                if let Err(error) = operands::remember(
                                    &mut self.operands,
                                    expression.clone(),
                                    value,
                                    Domain::default(),
                                    limits,
                                ) {
                                    self.failure = Some(error.into());
                                    return;
                                }
                                expression
                            }
                            Err(error) => {
                                self.failure = Some(error.into());
                                return;
                            }
                        }
                    } else {
                        value_expr
                    };
                    root = match root.store(ExprId::constant(*key), value_expr, limits.into()) {
                        Ok(root) => root,
                        Err(e) => {
                            self.failure = Some(e.into());
                            return;
                        }
                    };
                } else {
                    bindings.push(StateBinding {
                        index: *key,
                        value: value_expr,
                        numeric: value.numeric().clone(),
                    });
                }
            }
            if !complete && let SlotStateKind::Input { source, .. } = root.kind() {
                root = match SlotStateId::observed(source.clone(), bindings, limits.into()) {
                    Ok(root) => root,
                    Err(e) => {
                        self.failure = Some(e.into());
                        return;
                    }
                };
            }
            self.roots.insert(address, root);
        }
    }
    pub(super) fn read_checked(
        &self,
        address: Address,
        key: &AbstractValue,
        domain: Domain,
    ) -> Result<AbstractValue, StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let root = self.root_at(address)?;
        let key_expr = expression(key)?;
        let mut facts = self.operands.clone();
        operands::remember(&mut facts, key_expr.clone(), key, domain, limits(domain))?;
        let expr = ExprId::slot_read(root.clone(), key_expr.clone(), limits(domain))?;
        let numeric = read_numeric(&root, &key_expr, &facts, domain);
        Ok(numeric::attach(numeric, expr))
    }
    pub(super) fn read(
        &self,
        address: Address,
        key: &AbstractValue,
        domain: Domain,
    ) -> AbstractValue {
        self.read_checked(address, key, domain).unwrap_or_else(|_| {
            self.read_numeric_cache(address, key, domain)
                .with_symbolic_limit()
        })
    }
    fn write_checked(
        &mut self,
        address: Address,
        key: &AbstractValue,
        value: &AbstractValue,
        domain: Domain,
    ) -> Result<(), StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let key_expr = expression(key)?;
        let value_expr = expression(value)?;
        let root = self.root_at(address)?.store(
            key_expr.clone(),
            value_expr.clone(),
            limits(domain).into(),
        )?;
        let mut facts = self.operands.clone();
        operands::remember(&mut facts, key_expr, key, domain, limits(domain))?;
        operands::remember(&mut facts, value_expr, value, domain, limits(domain))?;
        self.roots.insert(address, root);
        self.operands = facts;
        self.write_numeric_cache(address, key, value, domain);
        Ok(())
    }
    fn check_existing(&self, limits: ExprLimits) -> Result<(), StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        for root in std::iter::once(&self.base).chain(self.roots.values()) {
            root.join_guarded(root, ExprId::constant(U256::ZERO), limits.into())?;
        }
        for value in self
            .slots
            .values()
            .chain(self.defaults.values())
            .chain(self.operands.values())
            .chain([&self.global_default])
        {
            crate::world::bytes::check_value(value, limits)?;
        }
        Ok(())
    }
    fn join_guarded(
        &self,
        other: &Self,
        domain: Domain,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, StoreError> {
        self.check_existing(limits)?;
        other.check_existing(limits)?;
        if self == other {
            return Ok(self.clone());
        }
        if let Some(e) = self.failure.as_ref().or(other.failure.as_ref()) {
            return Err(e.clone());
        }
        let mut out = self.join_numeric_cache(other, domain);
        out.base = self
            .base
            .join_guarded(&other.base, selector.clone(), limits.into())?;
        out.operands = self.operands.clone();
        for (e, v) in &other.operands {
            out.operands
                .entry(e.clone())
                .and_modify(|old| *old = domain.join(old, v))
                .or_insert_with(|| v.clone());
        }
        let addresses = self
            .roots
            .keys()
            .chain(other.roots.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        out.roots.clear();
        for address in addresses {
            out.roots.insert(
                address,
                self.root_at(address)?.join_guarded(
                    &other.root_at(address)?,
                    selector.clone(),
                    limits.into(),
                )?,
            );
        }
        Ok(out)
    }
}

impl Store {
    /// Persistent slot root at an actual storage owner, including unknown accounts.
    pub fn slot_root(&self, address: Address, transient: bool) -> Result<SlotStateId, StoreError> {
        if transient {
            self.transient.root_at(address)
        } else {
            self.persistent.root_at(address)
        }
    }
    /// Read persistent storage with an immutable state term or typed failure.
    pub fn read_checked(
        &self,
        address: Address,
        key: &AbstractValue,
        domain: Domain,
    ) -> Result<AbstractValue, StoreError> {
        self.persistent.read_checked(address, key, domain)
    }
    /// Read transient storage with its independently scoped state term.
    pub fn read_transient_checked(
        &self,
        address: Address,
        key: &AbstractValue,
        domain: Domain,
    ) -> Result<AbstractValue, StoreError> {
        self.transient.read_checked(address, key, domain)
    }
    /// Typed construction failure leaves all transaction state unchanged.
    pub fn write_checked(
        &mut self,
        address: Address,
        key: &AbstractValue,
        value: &AbstractValue,
        domain: Domain,
    ) -> Result<(), StoreError> {
        self.persistent.write_checked(address, key, value, domain)
    }
    /// Append an atomic transaction-local transient slot write.
    pub fn write_transient_checked(
        &mut self,
        address: Address,
        key: &AbstractValue,
        value: &AbstractValue,
        domain: Domain,
    ) -> Result<(), StoreError> {
        self.transient.write_checked(address, key, value, domain)
    }
    pub(crate) fn check_existing(&self, limits: ExprLimits) -> Result<(), StoreError> {
        self.persistent.check_existing(limits)?;
        self.transient.check_existing(limits)?;
        let mut result: Result<(), StoreError> = Ok(());
        self.visit_values(&mut |value| {
            if result.is_ok() {
                result = crate::world::bytes::check_value(value, limits).map_err(Into::into);
            }
        });
        result?;
        for log in self.possible_logs.values() {
            log.data.check_existing(limits)?;
        }
        Ok(())
    }
    /// A single selector pairs both storage planes, scalar accounts, and logs.
    pub fn join_guarded(
        &self,
        other: &Self,
        domain: Domain,
        selector: ExprId,
        limits: ExprLimits,
    ) -> Result<Self, StoreError> {
        self.check_existing(limits)?;
        other.check_existing(limits)?;
        if self == other {
            return Ok(self.clone());
        }
        let mut out = self.join_numeric_cache(other, domain);
        out.persistent =
            self.persistent
                .join_guarded(&other.persistent, domain, selector.clone(), limits)?;
        out.transient =
            self.transient
                .join_guarded(&other.transient, domain, selector.clone(), limits)?;
        for address in self.addresses().union(&other.addresses()).copied() {
            out.balances.insert(
                address,
                self.read_balance(address).join_guarded(
                    &other.read_balance(address),
                    domain,
                    selector.clone(),
                    limits,
                )?,
            );
            out.nonces.insert(
                address,
                self.nonce(address).join_guarded(
                    &other.nonce(address),
                    domain,
                    selector.clone(),
                    limits,
                )?,
            );
        }
        out.balance_default = self.balance_default.join_guarded(
            &other.balance_default,
            domain,
            selector.clone(),
            limits,
        )?;
        for (key, left) in &self.possible_logs {
            if let Some(right) = other.possible_logs.get(key)
                && left.topics.len() == right.topics.len()
            {
                let topics = left
                    .topics
                    .iter()
                    .zip(&right.topics)
                    .map(|(a, b)| a.join_guarded(b, domain, selector.clone(), limits))
                    .collect::<Result<_, _>>()?;
                out.possible_logs.insert(
                    key.clone(),
                    super::AbstractLog {
                        topics,
                        data: left.data.join_guarded(
                            &right.data,
                            domain,
                            selector.clone(),
                            limits,
                        )?,
                    },
                );
            }
        }
        Ok(out)
    }
    /// Sound union of complete stores; identical stores keep their existing root.
    pub fn join(&self, other: &Self, domain: Domain) -> Self {
        if self == other {
            return self.clone();
        }
        let numeric = self.join_numeric_cache(other, domain);
        let result = expression(&AbstractValue::top())
            .and_then(|guard| self.join_guarded(other, domain, guard, limits(domain)));
        result.unwrap_or_else(|error| {
            let mut out = numeric;
            out.persistent.failure = Some(error.clone());
            out.transient.failure = Some(error);
            out
        })
    }
}

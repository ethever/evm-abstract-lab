//! Ordered slot roots and numeric upper bounds, partitioned by actual account.

use super::{EvmStore, Plane, StoreError};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    symbolic::{ExprId, ExprLimits, SlotStateId, SlotStateKind, SourceId, StateBinding},
};
use crate::world::bytes::{
    conditional::{ConditionalFacts, Context},
    numeric, operands,
};
use alloy_primitives::{Address, U256};
use std::collections::BTreeSet;

fn limits<D: EvmNumericAlgebra>(domain: AlgebraRef<'_, D>) -> ExprLimits {
    domain.spec().relations().expressions()
}
fn expression<N: WordElement>(value: &EvmValue<N>) -> Result<ExprId, StoreError> {
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
fn key_disjoint<N: WordElement>(a: &EvmValue<N>, b: &EvmValue<N>) -> bool {
    let (alo, ahi) = crate::world::unsigned_bounds(a);
    let (blo, bhi) = crate::world::unsigned_bounds(b);
    ahi < blo
        || bhi < alo
        || match (a.constants(), b.constants()) {
            (Some(a), Some(b)) => a.is_disjoint(b),
            _ => false,
        }
}
pub(super) fn read_numeric_in<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    root: &SlotStateId,
    key: &ExprId,
    context: &Context<'_, N, D>,
    domain: AlgebraRef<'_, D>,
) -> EvmValue<N> {
    let domain = numeric::numeric_domain(domain);
    match root.kind() {
        SlotStateKind::Zero => numeric::scalar_in(&ExprId::constant(U256::ZERO), context, domain),
        SlotStateKind::Input { bindings, .. } => {
            let key = numeric::scalar_in(key, context, domain);
            if let Some(keys) = crate::world::candidates(&key) {
                let mut out = None;
                for k in keys.iter() {
                    let value = bindings.iter().find(|b| b.index == *k).map_or_else(
                        EvmValue::<N>::top,
                        |b| {
                            let value = numeric::scalar_in(&b.value, context, domain);
                            let retained = context.has_binding_value(&b.value);
                            if retained && !context.pay_binding(value.numeric(), &b.numeric) {
                                return EvmValue::<N>::top();
                            }
                            numeric::bound(value, retained, &b.numeric)
                        },
                    );
                    out = Some(out.map_or_else(|| value.clone(), |old| domain.join(&old, &value)));
                }
                out.unwrap_or_else(EvmValue::<N>::top)
            } else {
                EvmValue::<N>::top()
            }
        }
        SlotStateKind::Store {
            previous,
            key: written,
            value,
        } => {
            if key == written {
                return numeric::scalar_in(value, context, domain);
            }
            let requested = numeric::scalar_in(key, context, domain);
            let stored = numeric::scalar_in(written, context, domain);
            if let (Some(requested), Some(stored)) = (requested.singleton(), stored.singleton()) {
                return if requested == stored {
                    numeric::scalar_in(value, context, domain)
                } else {
                    read_numeric_in(previous, key, context, domain)
                };
            }
            if key_disjoint(&requested, &stored) {
                read_numeric_in(previous, key, context, domain)
            } else {
                domain.join(
                    &read_numeric_in(previous, key, context, domain),
                    &numeric::scalar_in(value, context, domain),
                )
            }
        }
        SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } => {
            let c = numeric::scalar_in(condition, context, domain);
            if !c.may_be_zero() {
                read_numeric_in(
                    then_state,
                    key,
                    &context.branch_when_scoped(condition, true),
                    domain,
                )
            } else if !c.may_be_nonzero() {
                read_numeric_in(
                    else_state,
                    key,
                    &context.branch_when_scoped(condition, false),
                    domain,
                )
            } else {
                domain.join(
                    &read_numeric_in(
                        then_state,
                        key,
                        &context.branch_when_scoped(condition, true),
                        domain,
                    ),
                    &read_numeric_in(
                        else_state,
                        key,
                        &context.branch_when_scoped(condition, false),
                        domain,
                    ),
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
impl<N: WordElement> Plane<N> {
    pub(super) fn root_at(&self, address: Address) -> Result<SlotStateId, StoreError> {
        self.roots
            .get(&address)
            .cloned()
            .map_or_else(|| account_base(&self.base, address), Ok)
    }
    pub(super) fn initialize_roots<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        limits: ExprLimits,
        domain: AlgebraRef<'_, D>,
    ) {
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
                    domain,
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
                                numeric: value.numeric().observation(),
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
                                    domain,
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
                        numeric: value.numeric().observation(),
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
    pub(super) fn read_checked<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        key: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmValue<N>, StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let root = self.root_at(address)?;
        let key_expr = expression(key)?;
        let mut facts = self.operands.clone();
        operands::remember(&mut facts, key_expr.clone(), key, domain, limits(domain))?;
        let expr = ExprId::slot_read(root.clone(), key_expr.clone(), limits(domain))?;
        let mut value = read_numeric_in(
            &root,
            &key_expr,
            &Context::new(&facts, Some(&self.conditional), domain),
            domain,
        );
        if crate::world::candidates(key).is_some() && value.identity().input().is_none() {
            // The pointwise cache keeps possible sources from either branch,
            // even when A1 drops a one-sided unconditional operand guarantee.
            // Only merge metadata: every numeric component comes from this
            // root evaluation. An identified input already has its retained
            // operand metadata; do not erase that identity with a wider cache.
            // Finite keys use cache lookups, without another root traversal.
            let numeric = value.numeric().clone();
            value = numeric::numeric_domain(domain)
                .join(&value, &self.read_numeric_cache(address, key, domain))
                .with_numeric(numeric);
        }
        Ok(numeric::attach(value, expr))
    }
    pub(super) fn read<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        key: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> EvmValue<N> {
        self.read_checked(address, key, domain).unwrap_or_else(|_| {
            self.read_numeric_cache(address, key, domain)
                .with_symbolic_limit()
        })
    }
    fn write_checked<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        key: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
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
        if !self.conditional.has_no_scopes() {
            self.conditional.check(limits)?;
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
    fn join_guarded<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
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
        out.operands = operands::join_common(&self.operands, &other.operands, domain);
        out.conditional = ConditionalFacts::join_retained(
            (&self.operands, &self.conditional),
            (&other.operands, &other.conditional),
            &out.operands,
            selector.clone(),
            limits,
        )?;
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
        // The pointwise numeric-cache union above already covers both inputs.
        // Do not rewalk each whole root after dropping path-local guarantees.
        Ok(out)
    }
}

impl<N: WordElement> EvmStore<N> {
    /// Persistent slot root at an actual storage owner, including unknown accounts.
    pub fn slot_root(&self, address: Address, transient: bool) -> Result<SlotStateId, StoreError> {
        if transient {
            self.transient.root_at(address)
        } else {
            self.persistent.root_at(address)
        }
    }
    /// Read persistent storage with an immutable state term or typed failure.
    pub fn read_checked<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        key: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmValue<N>, StoreError> {
        self.persistent.read_checked(address, key, domain)
    }
    /// Read transient storage with its independently scoped state term.
    pub fn read_transient_checked<D: EvmNumericAlgebra<Element = N>>(
        &self,
        address: Address,
        key: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<EvmValue<N>, StoreError> {
        self.transient.read_checked(address, key, domain)
    }
    /// Typed construction failure leaves all transaction state unchanged.
    pub fn write_checked<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        key: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), StoreError> {
        self.persistent.write_checked(address, key, value, domain)
    }
    /// Append an atomic transaction-local transient slot write.
    pub fn write_transient_checked<D: EvmNumericAlgebra<Element = N>>(
        &mut self,
        address: Address,
        key: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
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
    pub fn join_guarded<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
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
                    super::EvmLog {
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
    pub fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        other: &Self,
        domain: AlgebraRef<'_, D>,
    ) -> Self {
        if self == other {
            return self.clone();
        }
        let numeric = self.join_numeric_cache(other, domain);
        let result = expression(&EvmValue::<N>::top())
            .and_then(|guard| self.join_guarded(other, domain, guard, limits(domain)));
        result.unwrap_or_else(|error| {
            let mut out = numeric;
            out.persistent.failure = Some(error.clone());
            out.transient.failure = Some(error);
            out
        })
    }
}

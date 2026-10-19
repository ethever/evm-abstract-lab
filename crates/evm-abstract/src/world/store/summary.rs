//! Cartesian numerical procedure summaries with independently rebuilt roots.
//!
//! Root/operand evaluation precedes projection. Losing path, alias and source
//! correlations is intentional here and never changes ordinary Store execution.

use super::{
    AbstractLog, InitialIndependence, LogKey, OrderedMap, Plane, Store, StoreError, lifecycle,
    sparse,
};
use crate::{
    domain::{
        AbstractValue, Domain,
        symbolic::{ExprId, SlotStateId, SlotStateKind, SourceId, StateBinding},
    },
    world::{
        Code, Existence,
        bytes::{
            numeric,
            summary::{
                ByteArraySummary, instantiate_scalar, join_scalar, scalar, scalar_expression_work,
            },
        },
    },
};
use alloy_primitives::{Address, U256};
use std::collections::{BTreeMap, BTreeSet};

mod layout;

#[derive(Clone, Debug, PartialEq, Eq)]
struct SummaryPlane {
    slots: OrderedMap<(Address, U256), AbstractValue>,
    defaults: OrderedMap<Address, AbstractValue>,
    global_default: AbstractValue,
    initial_independence: InitialIndependence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SummaryLog {
    topics: Vec<AbstractValue>,
    data: ByteArraySummary,
}

/// Complete canonical procedure payload. Symbolic history and input-output
/// correlations are forgotten; independent instantiations recover executable
/// roots constrained by every retained numerical fact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreSummary {
    persistent: SummaryPlane,
    transient: SummaryPlane,
    balances: OrderedMap<Address, AbstractValue>,
    balance_default: AbstractValue,
    codes: OrderedMap<Address, Code>,
    nonces: OrderedMap<Address, AbstractValue>,
    existence: OrderedMap<Address, Existence>,
    created: OrderedMap<Address, Option<bool>>,
    created_default: Option<bool>,
    pending_destruction: OrderedMap<Address, Option<bool>>,
    pending_destruction_default: Option<bool>,
    touched: OrderedMap<Address, Option<bool>>,
    touched_default: Option<bool>,
    possible_logs: OrderedMap<LogKey, SummaryLog>,
    logs_unknown: bool,
}

/// Concrete keys may occur in a root even when no presentation cache retained
/// them. Their numeric facts must still participate in the summary abstraction.
fn constant_keys(plane: &Plane, root: &SlotStateId, domain: Domain) -> BTreeSet<U256> {
    let mut pending = vec![root.clone()];
    let mut seen = BTreeSet::new();
    let mut keys = BTreeSet::new();
    while let Some(root) = pending.pop() {
        if !seen.insert(root.id()) {
            continue;
        }
        match root.kind() {
            SlotStateKind::Zero => {}
            SlotStateKind::Input { bindings, .. } => {
                keys.extend(bindings.iter().map(|binding| binding.index));
            }
            SlotStateKind::Store { previous, key, .. } => {
                if let Some(constants) = numeric::scalar(key, &plane.operands, domain).constants() {
                    keys.extend(constants);
                }
                pending.push(previous.clone());
            }
            SlotStateKind::Ite {
                then_state,
                else_state,
                ..
            } => pending.extend([then_state.clone(), else_state.clone()]),
        }
    }
    keys
}

/// Bound unlisted cells separately: writes whose entire key set is explicit
/// cannot affect the remaining default. Unknown aliases can affect it.
fn remaining_upper(
    plane: &Plane,
    root: &SlotStateId,
    keys: &BTreeSet<U256>,
    domain: Domain,
    memo: &mut BTreeMap<SlotStateId, AbstractValue>,
) -> Result<AbstractValue, StoreError> {
    if analysis_control::current_cancellation().is_cancelled() {
        return Err(crate::domain::symbolic::ExprError::Cancelled.into());
    }
    if let Some(value) = memo.get(root) {
        return Ok(value.clone());
    }
    let value = match root.kind() {
        SlotStateKind::Zero => scalar(&AbstractValue::constant(U256::ZERO), domain),
        SlotStateKind::Input { .. } => AbstractValue::top(),
        SlotStateKind::Store {
            previous,
            key,
            value,
        } => {
            let old = remaining_upper(plane, previous, keys, domain, memo)?;
            let key = numeric::scalar(key, &plane.operands, domain);
            if key
                .constants()
                .is_some_and(|written| written.is_subset(keys))
            {
                old
            } else {
                join_scalar(
                    &old,
                    &scalar(&numeric::scalar(value, &plane.operands, domain), domain),
                    domain,
                    false,
                )
            }
        }
        SlotStateKind::Ite {
            condition,
            then_state,
            else_state,
        } => {
            let truth = numeric::scalar(condition, &plane.operands, domain);
            if !truth.may_be_nonzero() {
                remaining_upper(plane, else_state, keys, domain, memo)?
            } else if !truth.may_be_zero() {
                remaining_upper(plane, then_state, keys, domain, memo)?
            } else {
                join_scalar(
                    &remaining_upper(plane, then_state, keys, domain, memo)?,
                    &remaining_upper(plane, else_state, keys, domain, memo)?,
                    domain,
                    false,
                )
            }
        }
    };
    memo.insert(root.clone(), value.clone());
    Ok(value)
}

// The foundation has zero and unconstrained default arrays. A nonzero bounded
// uniform default is conservatively forgotten, never encoded as a fake zero.
fn representable_default(value: &AbstractValue, domain: Domain) -> AbstractValue {
    if value.singleton() == Some(U256::ZERO) {
        scalar(value, domain)
    } else {
        AbstractValue::top()
    }
}

impl Plane {
    fn summary_projection(&self, domain: Domain) -> Result<SummaryPlane, StoreError> {
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let global_default = representable_default(
            &remaining_upper(
                self,
                &self.base,
                &BTreeSet::new(),
                domain,
                &mut BTreeMap::new(),
            )?,
            domain,
        );
        let mut out = SummaryPlane {
            slots: OrderedMap::new(),
            defaults: OrderedMap::new(),
            global_default,
            initial_independence: self.initial_independence.clone(),
        };
        let addresses: BTreeSet<_> = self
            .roots
            .keys()
            .chain(self.defaults.keys())
            .copied()
            .chain(self.slots.keys().map(|(owner, _)| *owner))
            .collect();
        for address in addresses {
            if analysis_control::current_cancellation().is_cancelled() {
                return Err(crate::domain::symbolic::ExprError::Cancelled.into());
            }
            let root = self.root_at(address)?;
            let mut keys = constant_keys(self, &root, domain);
            keys.extend(
                self.slots
                    .range((address, U256::ZERO)..=(address, U256::MAX))
                    .map(|((_, key), _)| *key),
            );
            let default = representable_default(
                &remaining_upper(self, &root, &keys, domain, &mut BTreeMap::new())?,
                domain,
            );
            if default != out.global_default {
                out.defaults.insert(address, default.clone());
            }
            for key in keys {
                let value = scalar(
                    &super::slots::read_numeric(
                        &root,
                        &ExprId::constant(key),
                        &self.operands,
                        domain,
                    ),
                    domain,
                );
                if value != default {
                    out.slots.insert((address, key), value);
                }
            }
        }
        Ok(out)
    }
}

impl SummaryPlane {
    fn default_at(&self, owner: Address) -> &AbstractValue {
        self.defaults.get(&owner).unwrap_or(&self.global_default)
    }

    fn at(&self, owner: Address, key: U256) -> &AbstractValue {
        self.slots
            .get(&(owner, key))
            .unwrap_or_else(|| self.default_at(owner))
    }

    fn work_size(&self) -> usize {
        let numeric = self
            .slots
            .values()
            .chain(self.defaults.values())
            .chain([&self.global_default])
            .fold(self.initial_independence.work_size(), |work, value| {
                work.saturating_add(value.numeric().work_size())
            });
        let owners = self
            .defaults
            .keys()
            .copied()
            .chain(self.slots.keys().map(|(owner, _)| *owner))
            .collect::<BTreeSet<_>>();
        owners.into_iter().fold(
            numeric.saturating_mul(2).saturating_add(2),
            |work, owner| {
                let values = self
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|(_, value)| value)
                    .collect::<Vec<_>>();
                let zero = self.default_at(owner).singleton() == Some(U256::ZERO);
                let observed = values.iter().fold(2usize, |nodes, value| {
                    nodes
                        .saturating_add(if value.singleton().is_some() { 1 } else { 4 })
                        .saturating_add(value.numeric().work_size())
                });
                let root = if zero {
                    values.iter().fold(1usize, |nodes, value| {
                        nodes
                            .saturating_add(2)
                            .saturating_add(if value.singleton().is_some() {
                                1
                            } else {
                                value.numeric().work_size().saturating_add(8)
                            })
                    })
                } else {
                    observed
                };
                values
                    .iter()
                    .fold(work.saturating_add(root), |work, value| {
                        if !zero || value.singleton().is_some() {
                            return work;
                        }
                        let numeric = value.numeric().work_size();
                        let expression = numeric.saturating_add(8);
                        work.saturating_add(numeric).saturating_add(expression)
                    })
            },
        )
    }

    fn join(&self, other: &Self, domain: Domain, widen: bool) -> Self {
        let (defaults, global_default) = sparse::combine(
            (&self.defaults, &self.global_default),
            (&other.defaults, &other.global_default),
            |left, right| join_scalar(left, right, domain, widen),
        );
        let mut out = Self {
            slots: OrderedMap::new(),
            defaults,
            global_default,
            initial_independence: self.initial_independence.join(&other.initial_independence),
        };
        out.slots = sparse::union_with(&self.slots, &other.slots, |&(owner, key)| {
            let value = join_scalar(self.at(owner, key), other.at(owner, key), domain, widen);
            (value != *out.default_at(owner)).then_some(value)
        });
        if widen {
            let owners = out
                .slots
                .keys()
                .map(|(owner, _)| *owner)
                .collect::<BTreeSet<_>>();
            for owner in owners {
                let previous = self
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), _)| *key)
                    .collect::<BTreeSet<_>>();
                let joined = out
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), _)| *key)
                    .collect::<BTreeSet<_>>();
                if joined.len() > domain.capacity() && !joined.is_subset(&previous) {
                    // A growing family of concrete locations becomes an open
                    // default. Retain established cells (such as busy/x), while
                    // all subsequently discovered locations are covered by Top.
                    if out.global_default.numeric().is_top() {
                        out.defaults.remove(&owner);
                    } else {
                        out.defaults.insert(owner, AbstractValue::top());
                    }
                    out.slots.retain(|(address, key), value| {
                        *address != owner || (previous.contains(key) && !value.numeric().is_top())
                    });
                    out.initial_independence
                        .slots
                        .retain(|(address, key), _| *address != owner || previous.contains(key));
                }
            }
        }
        out
    }

    fn instantiate(&self, domain: Domain) -> Result<Plane, StoreError> {
        let limits = domain.spec().relations().expressions();
        let mut out = Plane::new(self.global_default.clone(), true);
        out.slots = self.slots.clone();
        out.defaults = self.defaults.clone();
        out.initial_independence = self.initial_independence.clone();
        let owners: BTreeSet<_> = self
            .defaults
            .keys()
            .copied()
            .chain(self.slots.keys().map(|(owner, _)| *owner))
            .collect();
        for owner in owners {
            let source = SourceId::fresh()?;
            let plain = SlotStateId::input(source.clone());
            let root = if self.default_at(owner).singleton() == Some(U256::ZERO) {
                let mut root = SlotStateId::zero();
                for ((_, key), value) in self.slots.range((owner, U256::ZERO)..=(owner, U256::MAX))
                {
                    let expression = if let Some(constant) = value.singleton() {
                        ExprId::constant(constant)
                    } else {
                        let observed = SlotStateId::observed(
                            source.clone(),
                            vec![StateBinding {
                                index: *key,
                                value: ExprId::slot_read(
                                    plain.clone(),
                                    ExprId::constant(*key),
                                    limits,
                                )?,
                                numeric: value.numeric().clone(),
                            }],
                            limits.into(),
                        )?;
                        let expression =
                            ExprId::slot_read(observed, ExprId::constant(*key), limits)?;
                        // Ordered stores evaluate this value expression. Their
                        // numeric cache must retain its complete upper bound.
                        out.operands.insert(
                            expression.clone(),
                            value.clone().with_expression(expression.clone()),
                        );
                        expression
                    };
                    root = root.store(ExprId::constant(*key), expression, limits.into())?;
                }
                root
            } else {
                let bindings = self
                    .slots
                    .range((owner, U256::ZERO)..=(owner, U256::MAX))
                    .map(|((_, key), value)| {
                        Ok(StateBinding {
                            index: *key,
                            value: if let Some(constant) = value.singleton() {
                                ExprId::constant(constant)
                            } else {
                                ExprId::slot_read(plain.clone(), ExprId::constant(*key), limits)?
                            },
                            numeric: value.numeric().clone(),
                        })
                    })
                    .collect::<Result<Vec<_>, StoreError>>()?;
                // The observed root already carries these numeric constraints;
                // caching an outer read for every cell duplicates that root.
                SlotStateId::observed(source, bindings, limits.into())?
            };
            out.roots.insert(owner, root);
        }
        Ok(out)
    }
}

impl Store {
    /// Sound whole-payload projection for unknown procedure fixed points.
    pub fn summary_projection(&self, domain: Domain) -> Result<StoreSummary, StoreError> {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(crate::domain::symbolic::ExprError::Cancelled.into());
        }
        // Numerical projection consumes old roots without retaining new read
        // terms for them. Replay checks its rebuilt roots against this policy.
        let possible_logs = self
            .possible_logs
            .iter()
            .map(|(key, log)| {
                Ok((
                    key.clone(),
                    SummaryLog {
                        topics: log
                            .topics
                            .iter()
                            .map(|topic| scalar(topic, domain))
                            .collect(),
                        data: log.data.summary_projection(domain)?,
                    },
                ))
            })
            .collect::<Result<_, StoreError>>()?;
        let balance_default = scalar(&self.balance_default, domain);
        Ok(StoreSummary {
            persistent: self.persistent.summary_projection(domain)?,
            transient: self.transient.summary_projection(domain)?,
            balances: sparse::map((&self.balances, &balance_default), |value| {
                scalar(value, domain)
            }),
            balance_default,
            codes: sparse::map((&self.codes, &Code::Unknown), Clone::clone),
            nonces: sparse::map((&self.nonces, &AbstractValue::top()), |value| {
                scalar(value, domain)
            }),
            existence: sparse::map((&self.existence, &Existence::Unknown), |value| *value),
            created: sparse::map((&self.created, &self.created_default), |value| *value),
            created_default: self.created_default,
            pending_destruction: sparse::map(
                (&self.pending_destruction, &self.pending_destruction_default),
                |value| *value,
            ),
            pending_destruction_default: self.pending_destruction_default,
            touched: sparse::map((&self.touched, &self.touched_default), |value| *value),
            touched_default: self.touched_default,
            possible_logs: if self.logs_unknown {
                OrderedMap::new()
            } else {
                possible_logs
            },
            logs_unknown: self.logs_unknown,
        })
    }
}

impl StoreSummary {
    /// Retained numerical payload plus a conservative independent replay bound.
    /// No cost from discarded source identities or write history survives reuse.
    pub fn work_size(&self) -> usize {
        let scalar = self.balances.values().chain(self.nonces.values()).fold(
            self.balance_default.numeric().work_size().saturating_mul(2),
            |work, value| {
                work.saturating_add(value.numeric().work_size().saturating_mul(2))
                    .saturating_add(scalar_expression_work(value))
            },
        );
        let fields = self
            .codes
            .values()
            .fold(scalar, |work, code| {
                work.saturating_add(
                    match code {
                        Code::Runtime(program) => program.byte_len(),
                        _ => 1,
                    }
                    .saturating_mul(2),
                )
            })
            .saturating_add(
                self.existence
                    .len()
                    .saturating_add(self.created.len())
                    .saturating_add(self.pending_destruction.len())
                    .saturating_add(self.touched.len())
                    .saturating_add(4)
                    .saturating_mul(2),
            );
        self.possible_logs.values().fold(
            fields
                .saturating_add(self.persistent.work_size())
                .saturating_add(self.transient.work_size()),
            |work, log| {
                log.topics
                    .iter()
                    .fold(work.saturating_add(log.data.work_size()), |work, value| {
                        work.saturating_add(value.numeric().work_size().saturating_mul(2))
                            .saturating_add(scalar_expression_work(value))
                    })
            },
        )
    }

    /// Union all state components; fresh identities cannot create new entries.
    pub fn join(&self, other: &Self, domain: Domain, widen: bool) -> Self {
        let codes = self
            .codes
            .iter()
            .filter_map(|(owner, code)| {
                (other.codes.get(owner) == Some(code)).then_some((*owner, code.clone()))
            })
            .collect();
        let existence = self
            .existence
            .iter()
            .filter_map(|(owner, value)| {
                (other.existence.get(owner) == Some(value)).then_some((*owner, *value))
            })
            .collect();
        let mut logs_unknown = self.logs_unknown || other.logs_unknown;
        let mut logs = self.possible_logs.clone();
        if !logs_unknown {
            for (key, incoming) in &other.possible_logs {
                if let Some(previous) = logs.get_mut(key) {
                    if previous.topics.len() != incoming.topics.len() {
                        logs_unknown = true;
                        break;
                    }
                    for (topic, value) in previous.topics.iter_mut().zip(&incoming.topics) {
                        *topic = join_scalar(topic, value, domain, widen);
                    }
                    previous.data = previous.data.join(&incoming.data, domain, widen);
                } else {
                    logs.insert(key.clone(), incoming.clone());
                }
            }
        }
        if logs_unknown {
            logs.clear();
        }
        let join_numeric =
            |left: &AbstractValue, right: &AbstractValue| join_scalar(left, right, domain, widen);
        let (balances, balance_default) = sparse::combine(
            (&self.balances, &self.balance_default),
            (&other.balances, &other.balance_default),
            join_numeric,
        );
        let (nonces, _) = sparse::combine(
            (&self.nonces, &AbstractValue::top()),
            (&other.nonces, &AbstractValue::top()),
            join_numeric,
        );
        let (created, created_default) = sparse::combine(
            (&self.created, &self.created_default),
            (&other.created, &other.created_default),
            lifecycle::join,
        );
        let (pending_destruction, pending_destruction_default) = sparse::combine(
            (&self.pending_destruction, &self.pending_destruction_default),
            (
                &other.pending_destruction,
                &other.pending_destruction_default,
            ),
            lifecycle::join,
        );
        let (touched, touched_default) = sparse::combine(
            (&self.touched, &self.touched_default),
            (&other.touched, &other.touched_default),
            lifecycle::join,
        );
        Self {
            persistent: self.persistent.join(&other.persistent, domain, widen),
            transient: self.transient.join(&other.transient, domain, widen),
            balances,
            balance_default,
            codes,
            nonces,
            existence,
            created,
            created_default,
            pending_destruction,
            pending_destruction_default,
            touched,
            touched_default,
            possible_logs: logs,
            logs_unknown,
        }
    }

    /// Whole-payload subsumption, including defaults, lifecycle and events.
    pub fn covers(&self, other: &Self, domain: Domain) -> bool {
        self.join(other, domain, false) == *self
    }

    /// Apply the only persistent bookkeeping effect of a static invocation.
    /// Data and immutable read roots retain the caller's exact identities.
    /// Existing must-touches cannot be undone by a widened callee summary.
    pub fn apply_static_touches(&self, input: &Store) -> Store {
        let mut out = input.clone();
        (out.touched, out.touched_default) = sparse::combine(
            (&input.touched, &input.touched_default),
            (&self.touched, &self.touched_default),
            |left, right| lifecycle::or(*left, *right),
        );
        out
    }

    /// Independent symbolic state for one procedure execution or result replay.
    /// Instantiating twice never equates either result with historical reads.
    pub fn instantiate(&self, domain: Domain) -> Result<Store, StoreError> {
        let instantiate_map = |values: &OrderedMap<Address, AbstractValue>| {
            values
                .iter()
                .map(|(owner, value)| Ok((*owner, instantiate_scalar(value, domain)?)))
                .collect::<Result<_, StoreError>>()
        };
        let store = Store {
            persistent: self.persistent.instantiate(domain)?,
            transient: self.transient.instantiate(domain)?,
            balances: instantiate_map(&self.balances)?,
            // An unspecified account default denotes independent balances for
            // different addresses, never one shared symbolic balance word.
            balance_default: self.balance_default.clone(),
            codes: self.codes.clone(),
            nonces: instantiate_map(&self.nonces)?,
            existence: self.existence.clone(),
            created: self.created.clone(),
            created_default: self.created_default,
            pending_destruction: self.pending_destruction.clone(),
            pending_destruction_default: self.pending_destruction_default,
            touched: self.touched.clone(),
            touched_default: self.touched_default,
            possible_logs: self
                .possible_logs
                .iter()
                .map(|(key, log)| {
                    Ok((
                        key.clone(),
                        AbstractLog {
                            topics: log
                                .topics
                                .iter()
                                .map(|topic| instantiate_scalar(topic, domain))
                                .collect::<Result<_, _>>()?,
                            data: log.data.instantiate(domain)?,
                        },
                    ))
                })
                .collect::<Result<_, StoreError>>()?,
            logs_unknown: self.logs_unknown,
        };
        store.check_existing(domain.spec().relations().expressions())?;
        Ok(store)
    }
}

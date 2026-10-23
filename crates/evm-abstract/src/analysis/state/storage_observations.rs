//! Finite persistent reads at active stack positions, attached to the summary Store.
//!
//! This optional precision domain recognizes a latest literal write or an existing
//! direct numeric guarantee for exactly the current immutable read. It never
//! traverses another owner's storage or copies the Store's complete operand map.

use crate::domain::{EvmNumericAlgebra, WordElement};

use super::{
    Address, AlgebraRef, EvmMachinePayload, EvmValue, FrameCode, MachineError,
    stack_alias::StackAliases,
};
use crate::{
    domain::{
        relational::{Constraint, QueryReason},
        symbolic::{ExprError, ExprId, ExprKind, SlotStateId, SlotStateKind, StateError},
    },
    world::{RangeError, StoreError},
};

mod payment;
#[cfg(test)]
mod tests;

// These are precision caps, not permission to omit transaction effects. A read
// outside the caps simply contributes no guarantee to this optional template.
const MAX_POSITIONS: usize = 16;
const MAX_ROOT_WORK: usize = 64;
const MAX_KEY_WORK: usize = 32;
const MAX_FACT_WORK: usize = 512;
const MAX_MEMBERS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Observation<N: WordElement> {
    owner: Address,
    stack_position: usize,
    numeric: N,
}

/// No expression, root or historical payload survives in this template.
/// Persistent is the only plane, so it is implicit in every stable key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::analysis) struct EvmStorageObservations<N: WordElement>(Vec<Observation<N>>);

// slot_read may compare the requested key while traversing a bounded root. This
// local product covers those structural comparisons; unrelated code and accounts
// are not storage-read work. Root lookup is covered by the caller's payload fee.
fn read_work(root: usize, key: usize) -> usize {
    root.saturating_add(1)
        .saturating_mul(key.saturating_add(1))
        .saturating_mul(16)
        .saturating_add(
            root.saturating_add(key)
                .saturating_add(1)
                .saturating_mul(16),
        )
}

fn in_scope<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    payload: &EvmMachinePayload<N>,
    domain: AlgebraRef<'_, D>,
) -> bool {
    let frame = payload.active();
    frame.code != FrameCode::Unknown
        && domain.spec().relations().enabled
        // The internal namespace can represent symbolic logical owners. This
        // bounded domain deliberately covers only the concrete logical case.
        && frame.key.address_value.as_concrete() == Some(frame.key.address)
}

fn key_work<N: WordElement>(key: &EvmValue<N>) -> Option<usize> {
    // This optional extractor uses retained source evidence only. A numerical
    // singleton proof has an independent tariff and is not needed here.
    let work = key.expression()?.nodes();
    (work <= MAX_KEY_WORK).then_some(work)
}

fn key_expression<N: WordElement>(key: &EvmValue<N>) -> Option<ExprId> {
    key_work(key)?;
    key.expression().cloned()
}

fn same_key<N: WordElement>(expression: &ExprId, key: &EvmValue<N>) -> bool {
    key.expression().is_some_and(|retained| {
        retained.immutable_node_key() == expression.immutable_node_key()
            || matches!((retained.as_constant(), expression.as_constant()), (Some(a), Some(b)) if a == b)
    })
}

fn latest_literal<N: WordElement>(
    root: &SlotStateId,
    key: &EvmValue<N>,
) -> Option<alloy_primitives::U256> {
    let SlotStateKind::Store {
        key: written,
        value,
        ..
    } = root.kind()
    else {
        return None;
    };
    same_key(written, key)
        .then(|| value.as_constant())
        .flatten()
}

enum ReadFact<'a> {
    Literal(alloy_primitives::U256),
    Members(&'a std::collections::BTreeSet<alloy_primitives::U256>),
}

impl ReadFact<'_> {
    fn numeric<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
        &self,
        domain: AlgebraRef<'_, D>,
    ) -> N {
        match self {
            Self::Literal(value) => N::constant(*value),
            Self::Members(values) => values
                .iter()
                .copied()
                .map(EvmValue::constant)
                .reduce(|left, right| domain.join(&left, &right))
                .expect("nonempty finite read fact")
                .numeric()
                .clone(),
        }
    }
}

fn matches_read<N: WordElement>(
    expression: &ExprId,
    root: &SlotStateId,
    key: &EvmValue<N>,
) -> bool {
    matches!(expression.kind(), ExprKind::SlotRead { state, key: read_key }
        if state.id() == root.id() && same_key(read_key, key))
}

// These are already retained guarantees. No solver rule is added: require the
// exact immutable snapshot and immutable key identity (or the same literal key).
fn read_fact<'a, N: WordElement>(
    payload: &'a EvmMachinePayload<N>,
    root: &SlotStateId,
    key: &EvmValue<N>,
) -> Option<ReadFact<'a>> {
    // The caller checked the aggregate fact cap once, before this bounded scan.
    payload
        .relations
        .constraints()
        .find_map(|constraint| match constraint {
            Constraint::Equal { left, right } if matches_read(left, root, key) => {
                right.as_constant().map(ReadFact::Literal)
            }
            Constraint::Equal { left, right } if matches_read(right, root, key) => {
                left.as_constant().map(ReadFact::Literal)
            }
            Constraint::Members { expression, values }
                if !values.is_empty()
                    && values.len() <= MAX_MEMBERS
                    && matches_read(expression, root, key) =>
            {
                Some(ReadFact::Members(values))
            }
            _ => None,
        })
}

// Cancellation is typed even when a store carries a previously recorded failure.
fn drop_read_error(error: StoreError) -> Result<(), MachineError> {
    if matches!(
        &error,
        StoreError::Expression(ExprError::Cancelled)
            | StoreError::State(StateError::Cancelled)
            | StoreError::Bytes(RangeError::Expression(ExprError::Cancelled))
            | StoreError::Bytes(RangeError::State(StateError::Cancelled))
    ) {
        return Err(MachineError::Store(error));
    }
    MachineError::check_cancelled()
}

/// Prepay bounded key/root comparisons and, only when relevant, local fact import.
pub(in crate::analysis) fn form_work<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    payload: &EvmMachinePayload<N>,
    domain: AlgebraRef<'_, D>,
) -> usize {
    if !in_scope(payload, domain) {
        return 0;
    }
    let Ok(root) = payload.store.slot_root(payload.active().key.address, false) else {
        return 0;
    };
    if root.nodes() > MAX_ROOT_WORK {
        return 0;
    }
    let facts = payload.relations.work_size();
    payload
        .active()
        .stack
        .iter()
        .skip(payload.active().stack.len().saturating_sub(MAX_POSITIONS))
        .fold(0usize, |work, key| {
            if key_work(key).is_none() {
                return work;
            }
            let check = 64;
            if latest_literal(&root, key).is_some() {
                work.saturating_add(check).saturating_add(256)
            } else if !payload.relations.is_empty() && facts <= MAX_FACT_WORK {
                // Do not scan for a match before reserve. A bounded eight-value
                // import is prepaid even when the subsequent scan finds none.
                let import = MAX_MEMBERS
                    .saturating_add(1)
                    .saturating_pow(2)
                    .saturating_mul(256);
                work.saturating_add(check)
                    .saturating_add(facts.saturating_mul(32))
                    .saturating_add(import)
            } else {
                work.saturating_add(check)
            }
        })
}

impl<N: WordElement> EvmStorageObservations<N> {
    pub(in crate::analysis) fn form<D: EvmNumericAlgebra<Element = N>>(
        payload: &EvmMachinePayload<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Result<Self, MachineError> {
        MachineError::check_cancelled()?;
        if !in_scope(payload, domain) {
            return Ok(Self::default());
        }
        let frame = payload.active();
        let root = match payload.store.slot_root(frame.key.address, false) {
            Ok(root) if root.nodes() <= MAX_ROOT_WORK => root,
            Ok(_) => return Ok(Self::default()),
            Err(error) => {
                drop_read_error(error)?;
                return Ok(Self::default());
            }
        };
        let use_facts = payload.relations.work_size() <= MAX_FACT_WORK;
        let mut observations = Vec::new();
        for (stack_position, key) in frame
            .stack
            .iter()
            .enumerate()
            .skip(frame.stack.len().saturating_sub(MAX_POSITIONS))
        {
            MachineError::check_cancelled()?;
            if key_work(key).is_none() {
                continue;
            }
            let numeric = if let Some(value) = latest_literal(&root, key) {
                N::constant(value)
            } else if let Some(fact) = use_facts.then(|| read_fact(payload, &root, key)).flatten() {
                fact.numeric(domain)
            } else {
                continue;
            };
            if !numeric.is_top() {
                observations.push(Observation {
                    owner: frame.key.address,
                    stack_position,
                    numeric,
                });
            }
        }
        MachineError::check_cancelled()?;
        Ok(Self(observations))
    }

    pub(in crate::analysis) fn join<D: EvmNumericAlgebra<Element = N>>(
        &self,
        next: &Self,
        domain: AlgebraRef<'_, D>,
        widen: bool,
    ) -> Result<Self, MachineError> {
        MachineError::check_cancelled()?;
        let mut observations = Vec::new();
        let mut right = next.0.iter().peekable();
        for left in &self.0 {
            MachineError::check_cancelled()?;
            let key = (left.owner, left.stack_position);
            while right
                .peek()
                .is_some_and(|r| (r.owner, r.stack_position) < key)
            {
                right.next();
            }
            let Some(other) = right.peek().filter(|r| (r.owner, r.stack_position) == key) else {
                continue;
            };
            let old = EvmValue::from_numeric(left.numeric.clone());
            let new = EvmValue::from_numeric(other.numeric.clone());
            let joined = domain.join(&old, &new);
            let numeric = if widen {
                domain.widen_procedure(&old, &joined)
            } else {
                joined
            };
            if !numeric.numeric().is_top() {
                observations.push(Observation {
                    owner: left.owner,
                    stack_position: left.stack_position,
                    numeric: numeric.numeric().clone(),
                });
            }
        }
        MachineError::check_cancelled()?;
        Ok(Self(observations))
    }

    // Only the current partition proves equality. Keep every positional record
    // for future joins; differing read bounds in one class still install both.
    // Form caps the records at MAX_POSITIONS, and join can only remove records.
    fn needs_install(&self, index: usize, aliases: &StackAliases) -> bool {
        let observation = &self.0[index];
        !self.0[..index].iter().any(|earlier| {
            earlier.owner == observation.owner
                && aliases.same_class(earlier.stack_position, observation.stack_position)
                && earlier.numeric == observation.numeric
        })
    }

    /// Structural retention and shared-expression bound. Native equality,
    /// cloning, observation export and installation are paid independently.
    pub(in crate::analysis) fn work_size(
        &self,
        stack: &[EvmValue<N>],
        aliases: &StackAliases,
    ) -> usize {
        // One retained read expression per owner/alias class. Different native
        // observations on that same expression still retain and price every
        // numeric record below; only its shared graph is counted once. Native
        // equality and repeated installation work belong to the paid installer.
        let installations: Vec<_> = (0..self.0.len())
            .filter(|&index| {
                let current = &self.0[index];
                !self.0[..index].iter().any(|earlier| {
                    earlier.owner == current.owner
                        && aliases.same_class(earlier.stack_position, current.stack_position)
                })
            })
            .collect();
        // Keep the existing per-position retained-template bound, even when
        // aliases share a read expression. This structural size is not a quote
        // for an opaque native equality, copy or discovery hook.
        let retained = self.0.iter().fold(0usize, |work, observation| {
            work.saturating_add(
                observation
                    .numeric
                    .work_size()
                    .saturating_add(1)
                    .saturating_mul(MAX_POSITIONS.saturating_add(2)),
            )
        });
        let count = installations.len().saturating_add(1);
        installations.into_iter().fold(retained, |work, index| {
            let observation = &self.0[index];
            // Instantiation only reads roots within this cap. SummaryPlane is
            // deliberately opaque here; no world traversal is needed to bound it.
            let root = MAX_ROOT_WORK;
            let key = stack.get(observation.stack_position).map_or(1, |k| {
                k.expression().map_or(1, ExprId::nodes).min(MAX_KEY_WORK)
            });
            work.saturating_add(read_work(root, key)).saturating_add(
                root.saturating_add(key)
                    .saturating_add(observation.numeric.work_size())
                    .saturating_add(1)
                    .saturating_mul(count)
                    .saturating_mul(16),
            )
        })
    }

    pub(in crate::analysis) fn install<D: EvmNumericAlgebra<Element = N>>(
        &self,
        payload: &mut EvmMachinePayload<N>,
        aliases: &StackAliases,
        domain: AlgebraRef<'_, D>,
    ) -> Result<(), MachineError> {
        for (index, observation) in self.0.iter().enumerate() {
            MachineError::check_cancelled()?;
            // Each scan is bounded by MAX_POSITIONS and prepaid numeric work.
            if !self.needs_install(index, aliases) {
                continue;
            }
            MachineError::check_cancelled()?;
            let frame = payload.active();
            if frame.key.address != observation.owner {
                return Err(MachineError::Shape);
            }
            let key = frame
                .stack
                .get(observation.stack_position)
                .ok_or(MachineError::Shape)?;
            let Some(key) = key_expression(key) else {
                continue;
            };
            let root = match payload.store.slot_root(observation.owner, false) {
                Ok(root) if root.nodes() <= MAX_ROOT_WORK => root,
                Ok(_) => continue,
                Err(error) => {
                    drop_read_error(error)?;
                    continue;
                }
            };
            let expression =
                match ExprId::slot_read(root, key, domain.spec().relations().expressions()) {
                    Ok(expression) => expression,
                    Err(error) => {
                        drop_read_error(error.into())?;
                        continue;
                    }
                };
            // The fact is attached only to this immutable instantiated root. A
            // later store replacement does not reinstall it on the new root.
            let mut relations = payload.relations.clone();
            match relations.observe(
                &expression,
                &observation.numeric,
                &domain.spec().relations(),
            ) {
                Ok(()) => payload.relations = relations,
                Err(QueryReason::Cancelled) => {
                    return Err(MachineError::Relations(QueryReason::Cancelled));
                }
                Err(_) => {}
            }
        }
        MachineError::check_cancelled()?;
        Ok(())
    }
}

#[cfg(test)]
pub(in crate::analysis) type StorageObservations =
    EvmStorageObservations<crate::domain::NumericValue>;

impl<N: WordElement> Default for EvmStorageObservations<N> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

//! Finite persistent reads at active stack positions, attached to the summary Store.
//!
//! This optional precision domain recognizes a latest literal write or an existing
//! direct numeric guarantee for exactly the current immutable read. It never
//! traverses another owner's storage or copies the Store's complete operand map.

use super::{
    AbstractValue, Address, Domain, FrameCode, MachineError, MachinePayload, NumericValue,
};
use crate::{
    domain::{
        relational::{Constraint, QueryReason},
        symbolic::{ExprError, ExprId, ExprKind, SlotStateId, SlotStateKind, StateError},
    },
    world::{RangeError, StoreError},
};

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
struct Observation {
    owner: Address,
    stack_position: usize,
    numeric: NumericValue,
}

/// No expression, root or historical payload survives in this template.
/// Persistent is the only plane, so it is implicit in every stable key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct StorageObservations(Vec<Observation>);

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

fn in_scope(payload: &MachinePayload, domain: Domain) -> bool {
    let frame = payload.active();
    frame.code != FrameCode::Unknown
        && domain.spec().relations().enabled
        // The internal namespace can represent symbolic logical owners. This
        // first prototype deliberately covers only the concrete logical case.
        && frame.key.address_value.as_concrete() == Some(frame.key.address)
}

fn key_work(key: &AbstractValue) -> Option<usize> {
    let work = key
        .expression()
        .map_or_else(|| key.singleton().map(|_| 1), |e| Some(e.nodes()))?;
    (work <= MAX_KEY_WORK).then_some(work)
}

fn key_expression(key: &AbstractValue) -> Option<ExprId> {
    key_work(key)?;
    key.expression()
        .cloned()
        .or_else(|| key.singleton().map(ExprId::constant))
}

fn same_key(expression: &ExprId, key: &AbstractValue) -> bool {
    key.expression()
        .is_some_and(|e| e.immutable_node_key() == expression.immutable_node_key())
        || matches!((key.singleton(), expression.as_constant()), (Some(a), Some(b)) if a == b)
}

fn latest_literal(root: &SlotStateId, key: &AbstractValue) -> Option<alloy_primitives::U256> {
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
    fn numeric(&self, domain: Domain) -> NumericValue {
        match self {
            Self::Literal(value) => NumericValue::constant(*value),
            Self::Members(values) => values
                .iter()
                .copied()
                .map(AbstractValue::constant)
                .reduce(|left, right| domain.join(&left, &right))
                .expect("nonempty finite read fact")
                .numeric()
                .clone(),
        }
    }
}

fn matches_read(expression: &ExprId, root: &SlotStateId, key: &AbstractValue) -> bool {
    matches!(expression.kind(), ExprKind::SlotRead { state, key: read_key }
        if state.id() == root.id() && same_key(read_key, key))
}

// These are already retained guarantees. No solver rule is added: require the
// exact immutable snapshot and immutable key identity (or the same literal key).
fn read_fact<'a>(
    payload: &'a MachinePayload,
    root: &SlotStateId,
    key: &AbstractValue,
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

// Cancellation is typed even when a Store carries a previously recorded failure.
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
pub(super) fn form_work(payload: &MachinePayload, domain: Domain) -> usize {
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

impl StorageObservations {
    pub(super) fn form(payload: &MachinePayload, domain: Domain) -> Result<Self, MachineError> {
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
                NumericValue::constant(value)
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

    pub(super) fn join(
        &self,
        next: &Self,
        domain: Domain,
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
            let old = AbstractValue::from_numeric(left.numeric.clone());
            let new = AbstractValue::from_numeric(other.numeric.clone());
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

    /// Prepay reconstruction, numeric copies and comparison with earlier facts.
    pub(super) fn work_size(&self, stack: &[AbstractValue]) -> usize {
        let count = self.0.len().saturating_add(1);
        self.0.iter().fold(0usize, |work, observation| {
            // Instantiation only reads roots within this cap. SummaryPlane is
            // deliberately opaque here; no World traversal is needed to bound it.
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

    pub(super) fn install(
        &self,
        payload: &mut MachinePayload,
        domain: Domain,
    ) -> Result<(), MachineError> {
        for observation in &self.0 {
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
            // later Store replacement does not reinstall it on the new root.
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

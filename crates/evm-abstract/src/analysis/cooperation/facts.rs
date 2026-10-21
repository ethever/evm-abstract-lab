use crate::domain::{WordObservation, facts::FactError};
use analysis_kernel::{Capability, RefinePolicy};
use std::collections::BTreeMap;

/// Stack guarantees proved for every concrete execution reaching one Subject
/// under its frozen program/world/context assumptions, with explicit contradiction.
/// They need not be derivable from the EVM abstract value representation alone.
/// Indices count from the bottom of the active frame's stack. A missing entry
/// contributes no extra guarantee; pending computation uses kernel Availability.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StackFacts {
    /// Conjunction of valid provider guarantees for the same scoped executions.
    Reachable(BTreeMap<usize, WordObservation>),
    /// A registered rule proved this universe has no concrete input valuation.
    Unreachable,
}

impl StackFacts {
    /// No additional guarantee; this is ready semantic Top, not Pending.
    pub fn none() -> Self {
        Self::Reachable(BTreeMap::new())
    }

    /// One proved guarantee. Publishing requires Registered reduction authority.
    pub fn at(slot: usize, observation: WordObservation) -> Self {
        if observation.is_top() {
            Self::none()
        } else {
            Self::Reachable(BTreeMap::from([(slot, observation)]))
        }
    }

    /// Bound for cloning/comparing the complete feedback payload.
    pub fn work_size(&self) -> usize {
        match self {
            Self::Unreachable => 1,
            Self::Reachable(facts) => facts.values().fold(1usize, |work, fact| {
                work.saturating_add(fact.work_size()).saturating_add(2)
            }),
        }
    }
}

impl Default for StackFacts {
    fn default() -> Self {
        Self::none()
    }
}

/// Production interpreter's generation-local feedback capability.
pub struct Refinements;

impl Capability for Refinements {
    type Key = ();
    type Value = StackFacts;

    fn key_work(_: &()) -> usize {
        1
    }

    fn value_work(value: &StackFacts) -> usize {
        value.work_size()
    }
}

/// Canonical conjunction of independently published scoped stack facts.
#[derive(Clone, Copy, Debug, Default)]
pub struct StackMeet;

impl RefinePolicy for StackMeet {
    type Value = StackFacts;
    type Error = FactError;

    fn work(&self, old: &StackFacts, incoming: &StackFacts) -> usize {
        // Includes map traversal, complete finite intersections and comparison.
        // No numerical candidate enumeration or cloning happens while quoting.
        old.work_size()
            .saturating_add(incoming.work_size())
            .saturating_mul(4096)
    }

    fn refine(&self, old: &StackFacts, incoming: &StackFacts) -> Result<StackFacts, FactError> {
        let (StackFacts::Reachable(old), StackFacts::Reachable(incoming)) = (old, incoming) else {
            return Ok(StackFacts::Unreachable);
        };
        let mut next = old.clone();
        for (slot, observation) in incoming {
            if observation.is_top() {
                continue;
            }
            let value = match next.get(slot) {
                Some(previous) => match previous.meet(observation) {
                    Ok(value) => value,
                    Err(FactError::Contradiction { .. }) => return Ok(StackFacts::Unreachable),
                    Err(error) => return Err(error),
                },
                None => observation.clone(),
            };
            next.insert(*slot, value);
        }
        Ok(StackFacts::Reachable(next))
    }

    fn equivalent(&self, left: &StackFacts, right: &StackFacts) -> bool {
        left == right
    }
}

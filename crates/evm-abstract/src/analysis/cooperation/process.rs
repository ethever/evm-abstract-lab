//! Typed guarantees for a single immutable Process body input universe.

use crate::analysis::ProcessAction;
use analysis_kernel::{Capability, RefinePolicy};
use std::{collections::BTreeSet, convert::Infallible};

/// Trusted guarantees holding for every execution of this Subject. These
/// narrow detached transfer capabilities, never the ascending Store/input.
/// Publish only for executable Process Body inputs (namespace 1). Contributions
/// and native inputs reject nonempty Process feedback explicitly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessFacts {
    /// The process itself performs no storage/transient/log/lifecycle writes.
    /// Accepted only when every remaining primitive action is excluded; the
    /// current broad envelope also covers value/lifecycle effects of actions.
    /// Combining this with possible actions fails explicitly.
    pub local_readonly: bool,
    /// Primitive actions impossible for any represented execution. Excluding an
    /// action requires a coverage proof, not absence in an open discovery set.
    pub excluded_actions: BTreeSet<ProcessAction>,
}

impl ProcessFacts {
    /// Ready Top: no additional guarantee, distinct from a Pending producer.
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether no additional guarantee was supplied.
    pub fn is_empty(&self) -> bool {
        !self.local_readonly && self.excluded_actions.is_empty()
    }

    /// Complete clone/comparison envelope; the action vocabulary is finite.
    pub fn work_size(&self) -> usize {
        self.excluded_actions
            .len()
            .saturating_mul(4)
            .saturating_add(8)
    }
}

/// Generation-local Process feedback, separate from native stack observations.
pub struct ProcessRefinements;

impl Capability for ProcessRefinements {
    type Key = ();
    type Value = ProcessFacts;
    fn key_work(_: &()) -> usize {
        1
    }
    fn value_work(value: &Self::Value) -> usize {
        value.work_size()
    }
}

/// Conjunction of independently proved Process guarantees.
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcessMeet;

impl RefinePolicy for ProcessMeet {
    type Value = ProcessFacts;
    type Error = Infallible;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        old.work_size()
            .saturating_add(incoming.work_size())
            .saturating_mul(32)
    }
    fn refine(&self, old: &Self::Value, incoming: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(ProcessFacts {
            local_readonly: old.local_readonly || incoming.local_readonly,
            excluded_actions: old
                .excluded_actions
                .union(&incoming.excluded_actions)
                .copied()
                .collect(),
        })
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        left == right
    }
}

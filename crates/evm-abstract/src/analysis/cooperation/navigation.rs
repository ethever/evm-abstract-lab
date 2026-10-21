//! Typed navigation from real interpreter origins to native relative entries.
use super::{Point, ProcedureId};
use crate::analysis::FrameKey;
use analysis_kernel::{Capability, JoinPolicy};
use std::{collections::BTreeSet, convert::Infallible, sync::Arc};

/// Open relation from an actual World/Body call origin to its relative callees.
/// Ordinary known calls also remain visible as native [`super::Topology`] edges.
pub struct Callees;
impl Capability for Callees {
    type Key = Point;
    type Value = BTreeSet<ProcedureId>;
    fn key_work(_: &Point) -> usize {
        4
    }
    fn value_work(value: &Self::Value) -> usize {
        value.len().saturating_mul(2).saturating_add(1)
    }
}

/// Structural relative-procedure entry and captured frame/code context.
/// Resolve `entry` through [`super::current_input`] for its actual current native
/// payload. Registration context is descriptive; it is not a substitute for
/// that tracked live input or a proof that an open call relation is complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcedureInfo {
    /// Structural entry point in the production point/generation protocol.
    pub entry: Point,
    /// Code, execution/storage owner, caller and finite structural frame shape.
    pub frame: FrameKey,
}
impl ProcedureInfo {
    pub(crate) fn work_size(&self) -> usize {
        self.frame.jump_history.len().saturating_add(64)
    }
}

/// Stable procedure descriptors, queryable after discovering keys or callees.
pub struct ProcedureEntries;
impl Capability for ProcedureEntries {
    type Key = ProcedureId;
    type Value = Arc<ProcedureInfo>;
    fn key_work(_: &ProcedureId) -> usize {
        2
    }
    fn value_work(_: &Self::Value) -> usize {
        2
    }
    fn drop_work(value: &Self::Value) -> usize {
        value.work_size()
    }
}

/// Each descriptor is registered once. Input growth is selected through its
/// entry's stable GenerationEntry, never by mutating this structural mapping.
pub struct DescriptorPolicy;
impl JoinPolicy for DescriptorPolicy {
    type Value = Arc<ProcedureInfo>;
    type Error = Infallible;
    fn work(&self, old: &Self::Value, incoming: &Self::Value) -> usize {
        old.work_size()
            .saturating_add(incoming.work_size())
            .saturating_add(8)
    }
    fn join(&self, old: &Self::Value, _: &Self::Value) -> Result<Self::Value, Infallible> {
        Ok(Arc::clone(old))
    }
    fn equivalent(&self, left: &Self::Value, right: &Self::Value) -> bool {
        left == right
    }
}

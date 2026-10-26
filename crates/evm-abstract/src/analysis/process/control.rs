//! Source control used by a Process macrostep, never Native execution evidence.

use super::{ProcessLocation, ProcessSource};
use crate::analysis::MachineEdgeKind;
use alloy_primitives::B256;
use serde::Serialize;
use std::sync::Arc;

/// One immutable decoded MAY relation, independent of numerical world payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProcessControlRelation {
    /// Frozen captured-source catalogue used by every reference to this relation.
    pub epoch: B256,
    /// Exact code source, including its fork and captured hash.
    pub source: ProcessSource,
    /// Dense decoded block directory. Merely appearing here does not imply reachability.
    pub blocks: Vec<ProcessControlBlock>,
    /// Shared union of all legal decoded JUMPDEST block indices.
    pub jump_targets: Vec<usize>,
}

/// A decoded block's covering local transfer under arbitrary erased private data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProcessControlBlock {
    /// Normal within-frame successors; invalid taken jumps have no normal edge.
    pub successors: [Option<(ProcessLocation, MachineEdgeKind)>; 2],
    /// Continuation after a call/create; its actual delivery stays in the table.
    pub continuation: Option<ProcessLocation>,
    /// Effects, calls, creation and exits require a numerical boundary transfer.
    pub boundary: bool,
}

/// Actual composition of effect-free control for one retained Process input.
/// Its unchanged Store/invocation bound covers every location in `through`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ProcessControlRegion {
    /// Immutable source-qualified code relation, shared rather than rebuilt per body.
    pub relation: Arc<ProcessControlRelation>,
    /// This macrostep's admitted source location.
    pub entry: ProcessLocation,
    /// Pure MAY locations traversed without constructing numerical Body states.
    /// Their relation edges retain branches, loops and unknown-jump coverage.
    pub through: Vec<ProcessLocation>,
    /// Reachable effect/call/create/exit boundaries, still requiring Body transfer.
    pub boundaries: Vec<ProcessLocation>,
}

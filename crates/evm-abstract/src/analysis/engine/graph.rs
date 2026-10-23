//! Source adjacency borrowed from the canonical observed-edge set.
//!
//! No derived topology survives a query, so replay clones, commit replacement
//! and abandoned imports need no separate index restoration.

use crate::analysis::{EdgeKind, MachineEdge, MachineEdgeKind};
use std::collections::BTreeSet;

pub(super) fn outgoing(
    edges: &BTreeSet<MachineEdge>,
    from: usize,
) -> impl Iterator<Item = &MachineEdge> {
    // Derived ordering compares source, destination, then kind. Both enum
    // variants below are their first variants, so this includes destination
    // zero of every kind. An open upper bound also handles usize::MAX sources.
    let first = MachineEdge {
        from,
        to: 0,
        kind: MachineEdgeKind::Intraprocedural(EdgeKind::Fallthrough),
    };
    edges
        .range(first..)
        .take_while(move |edge| edge.from == from)
}

//! Native exports retain operand guarantees as bounded typed entries.

use crate::domain::WordElement;
use crate::domain::{
    EvmValue,
    symbolic::{ExprId, GraphLimits, SymbolicGraph, SymbolicGraphNode},
};
use serde::{
    Serialize, Serializer,
    ser::{Error, SerializeSeq},
};
use std::{collections::BTreeMap, fmt::Display};

#[derive(Serialize)]
struct Fact<'a, N: WordElement> {
    expression: &'a ExprId,
    value: &'a EvmValue<N>,
}

pub(in crate::world) fn serialize_facts<S: Serializer, N: WordElement>(
    facts: &BTreeMap<ExprId, EvmValue<N>>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    let mut remaining = GraphLimits::default();
    let mut sequence = serializer.serialize_seq(Some(facts.len()))?;
    for (expression, value) in facts {
        let scalar_work = value.numeric().work_size().saturating_add(1);
        if scalar_work > remaining.max_nodes {
            return Err(S::Error::custom(
                "symbolic operand facts exceed native metadata policy",
            ));
        }
        remaining.max_nodes -= scalar_work;
        for root in std::iter::once(expression).chain(value.expression()) {
            let graph = SymbolicGraph::expression(root, remaining).map_err(S::Error::custom)?;
            remaining.max_nodes = remaining
                .max_nodes
                .saturating_sub(root.nodes().max(graph.nodes.len()));
            let exact_bytes = graph.nodes.iter().fold(0usize, |size, node| {
                size.saturating_add(match node {
                    SymbolicGraphNode::BytesExact(bytes) => bytes.len(),
                    _ => 0,
                })
            });
            remaining.max_exact_bytes = remaining.max_exact_bytes.saturating_sub(exact_bytes);
        }
        sequence.serialize_element(&Fact { expression, value })?;
    }
    sequence.end()
}

/// Failed compatibility operations must remain visible in native snapshots;
/// reports must not interpret the conservative fallback as a completed state.
pub(in crate::world) fn serialize_failure<S: Serializer, E: Display>(
    failure: &Option<E>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    failure
        .as_ref()
        .map(ToString::to_string)
        .serialize(serializer)
}

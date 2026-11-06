//! Local failure-rank and occurrence certificates, independent of discovery.
use super::{Graph, Rule, source::Source};
use crate::{
    EntityId, Ir, LiftedGuard, LiftedGuardPayload, LiftedProgram, StructuredKind, Terminator,
};

/// Independent certificate admission: no calls to discover or its worklist.
/// Absence/completeness is additionally checked by the normal-form certificate.
pub(in crate::lift) fn verify(
    ir: &Ir,
    program: &LiftedProgram,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<bool> {
    let graph = Graph::build(ir, program, pay)?;
    if !pay(ir
        .blocks
        .len()
        .saturating_add(program.failures.len())
        .saturating_add(1))
    {
        return None;
    }
    let mut ranks = vec![None; ir.blocks.len()];
    let mut previous = None;
    for fact in &program.failures {
        let Some(index) = graph.blocks.get(&fact.block).copied() else {
            return Some(false);
        };
        if previous.is_some_and(|p| p >= fact.block) || ranks[index].replace(fact.rank).is_some() {
            return Some(false);
        }
        previous = Some(fact.block);
    }
    for (index, rank) in ranks.iter().enumerate() {
        let Some(rank) = rank else { continue };
        if !pay(graph.successors[index].len().saturating_add(1)) {
            return None;
        }
        let valid = match graph.rules[index] {
            Rule::Empty => *rank == 0,
            Rule::Transparent => {
                let mut max = 0;
                let mut valid = *rank > 0 && !graph.successors[index].is_empty();
                for next in &graph.successors[index] {
                    match ranks[*next] {
                        Some(child) if child < *rank => max = max.max(child),
                        _ => valid = false,
                    }
                }
                valid && max.checked_add(1) == Some(*rank)
            }
            Rule::Refused => false,
        };
        if !valid {
            return Some(false);
        }
    }
    let source = Source::build(ir, &graph, &ranks, pay)?;
    for control in &program.controls {
        if !pay(1) {
            return None;
        }
        if let Some(actual) = &control.guard {
            if !verify_guard(ir, &graph, &ranks, &source, control.node, actual) {
                return Some(false);
            }
            let Some(high) = graph.high.get(&actual.block) else {
                return Some(false);
            };
            if high.condition != Some(control.condition) {
                return Some(false);
            }
        }
    }
    Some(true)
}

/// Check the supplied overlay field by field. This does not call the overlay
/// constructor: a discovery mistake cannot serve as its own certificate.
fn verify_guard(
    ir: &Ir,
    graph: &Graph<'_>,
    ranks: &[Option<u32>],
    source: &Source<'_>,
    node: EntityId,
    proof: &LiftedGuard,
) -> bool {
    let Some(syntax) = source.nodes.get(&node) else {
        return false;
    };
    let StructuredKind::If {
        condition,
        then_branch,
        else_branch,
    } = &syntax.kind
    else {
        return false;
    };
    let Some(index) = graph.blocks.get(&proof.block).copied() else {
        return false;
    };
    let Terminator::Branch {
        condition: raw_condition,
        then_edges,
        else_edge: Some(other),
        unresolved: false,
        invalid_targets,
        ..
    } = &ir.blocks[index].terminator
    else {
        return false;
    };
    if condition != raw_condition
        || then_edges.len() != 1
        || !invalid_targets.is_empty()
        || proof.payload != LiftedGuardPayload::EmptyRevert
    {
        return false;
    }
    let (pass, failed, pass_edge, failed_edge) = if proof.pass_nonzero {
        (then_branch, else_branch, then_edges[0], *other)
    } else {
        (else_branch, then_branch, *other, then_edges[0])
    };
    if proof.pass_node != pass.id
        || proof.failure_node != failed.id
        || proof.pass_edge != pass_edge
        || proof.failure_edge != failed_edge
    {
        return false;
    }
    let Some(pass_raw) = graph.edges.get(&pass_edge) else {
        return false;
    };
    let Some(failed_raw) = graph.edges.get(&failed_edge) else {
        return false;
    };
    if pass_raw.from != proof.block
        || failed_raw.from != proof.block
        || !graph
            .blocks
            .get(&failed_raw.to)
            .is_some_and(|i| ranks[*i].is_some())
        || !graph
            .blocks
            .get(&pass_raw.to)
            .is_some_and(|i| ranks[*i].is_none())
    {
        return false;
    }
    let Some(pass_summary) = source.summaries.get(&pass.id) else {
        return false;
    };
    let Some(failed_summary) = source.summaries.get(&failed.id) else {
        return false;
    };
    if pass_summary.first_edge != Some(pass_edge)
        || failed_summary.first_edge != Some(failed_edge)
        || !failed_summary.failure_only
    {
        return false;
    }
    if !failed_summary.falls_through {
        return proof.continuation.is_none();
    }
    let Some((sequence, start)) = source.follows.get(&node).copied() else {
        return false;
    };
    let Some(container) = source.nodes.get(&sequence) else {
        return false;
    };
    let StructuredKind::Sequence(children) = &container.kind else {
        return false;
    };
    let Some(suffix) = source.suffixes.get(&(sequence, start)) else {
        return false;
    };
    if start >= children.len()
        || !suffix.failure_only
        || suffix.falls_through
        || failed_summary.end.is_none()
        || failed_summary.end != suffix.entry
    {
        return false;
    }
    let immediate = source
        .positions
        .get(&node)
        .is_some_and(|(parent, position)| *parent == sequence && *position + 1 == start);
    if !immediate {
        // Inheriting a closed failure continuation proves only the guard. The
        // ancestor's suffix belongs to its other arms too and must stay visible.
        return proof.continuation.is_none();
    }
    proof.continuation.as_ref().is_some_and(|tail| {
        tail.sequence == sequence
            && tail.start == start
            && tail.end == children.len()
            && tail.pass_falls_through == pass_summary.falls_through
    })
}

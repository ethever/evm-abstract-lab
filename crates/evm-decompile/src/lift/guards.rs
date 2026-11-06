//! Certified occurrence overlays over the unchanged raw control tree.
//!
//! Discovery uses a reverse least fixed point. Verification checks supplied local
//! ranks without running that fixed point. Source summaries and suffixes are
//! indexed once; no guard owns a copied failure closure or rescans a subtree.
use crate::{
    EntityId, Ir, LiftedFailure, LiftedGuard, LiftedGuardContinuation, LiftedGuardPayload,
    LiftedProgram, StructuredKind, Terminator,
};
use std::collections::VecDeque;

mod certificate;
mod graph;
pub(super) use certificate::verify;
use graph::{Graph, Rule};

mod source;
use source::Source;

#[cfg(test)]
mod tests;

pub(super) fn discover(
    ir: &Ir,
    program: &mut LiftedProgram,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<()> {
    let graph = Graph::build(ir, program, pay)?;
    if !pay(ir.blocks.len().saturating_mul(4).saturating_add(1)) {
        return None;
    }
    let mut ranks = vec![None; ir.blocks.len()];
    let mut remaining: Vec<_> = graph.successors.iter().map(Vec::len).collect();
    let mut largest = vec![0_u32; ir.blocks.len()];
    let mut ready = VecDeque::new();
    for (i, rule) in graph.rules.iter().enumerate() {
        if *rule == Rule::Empty {
            ranks[i] = Some(0);
            ready.push_back(i);
        }
    }
    while let Some(done) = ready.pop_front() {
        if !pay(graph.predecessors[done].len().saturating_add(1)) {
            return None;
        }
        for predecessor in &graph.predecessors[done] {
            if graph.rules[*predecessor] != Rule::Transparent {
                continue;
            }
            remaining[*predecessor] -= 1;
            largest[*predecessor] = largest[*predecessor].max(ranks[done]?);
            if remaining[*predecessor] == 0 {
                ranks[*predecessor] = Some(largest[*predecessor].checked_add(1)?);
                ready.push_back(*predecessor);
            }
        }
    }
    let source = Source::build(ir, &graph, &ranks, pay)?;
    if !pay(program.controls.len().saturating_add(1)) {
        return None;
    }
    let mut overlays = Vec::new();
    for control in &program.controls {
        if !pay(1) {
            return None;
        }
        overlays.push(guard(ir, &graph, &ranks, &source, control.node));
    }
    for (control, overlay) in program.controls.iter_mut().zip(overlays) {
        control.guard = overlay;
    }
    program.failures = ir
        .blocks
        .iter()
        .zip(ranks)
        .filter_map(|(block, rank)| {
            rank.map(|rank| LiftedFailure {
                block: block.id,
                rank,
            })
        })
        .collect();
    program.failures.sort_by_key(|f| f.block);
    Some(())
}

fn guard(
    ir: &Ir,
    graph: &Graph<'_>,
    ranks: &[Option<u32>],
    source: &Source<'_>,
    node: EntityId,
) -> Option<LiftedGuard> {
    let syntax = source.nodes.get(&node)?;
    let StructuredKind::If {
        condition,
        then_branch,
        else_branch,
    } = &syntax.kind
    else {
        return None;
    };
    let then_summary = source.summaries.get(&then_branch.id)?;
    let else_summary = source.summaries.get(&else_branch.id)?;
    let true_edge = graph.edges.get(&then_summary.first_edge?)?;
    let false_edge = graph.edges.get(&else_summary.first_edge?)?;
    if true_edge.from != false_edge.from {
        return None;
    }
    let block = *graph.blocks.get(&true_edge.from)?;
    let Terminator::Branch {
        condition: raw_condition,
        then_edges,
        else_edge: Some(other),
        unresolved: false,
        invalid_targets,
        ..
    } = &ir.blocks[block].terminator
    else {
        return None;
    };
    if condition != raw_condition
        || then_edges.as_slice() != [true_edge.id]
        || *other != false_edge.id
        || !invalid_targets.is_empty()
    {
        return None;
    }
    let then_failure = ranks[*graph.blocks.get(&true_edge.to)?].is_some();
    let else_failure = ranks[*graph.blocks.get(&false_edge.to)?].is_some();
    // A guard has one certified failed route and one continuing route. A block
    // whose every route fails remains an ordinary conditional in the raw tree.
    if then_failure == else_failure {
        return None;
    }
    let (pass, failure, pass_summary, failure_summary, pass_edge, failure_edge) = if then_failure {
        (
            else_branch,
            then_branch,
            else_summary,
            then_summary,
            false_edge.id,
            true_edge.id,
        )
    } else {
        (
            then_branch,
            else_branch,
            then_summary,
            else_summary,
            true_edge.id,
            false_edge.id,
        )
    };
    if !failure_summary.failure_only {
        return None;
    }
    let continuation = if failure_summary.falls_through {
        let (sequence, start) = *source.follows.get(&node)?;
        let suffix = source.suffixes.get(&(sequence, start))?;
        let StructuredKind::Sequence(children) = &source.nodes.get(&sequence)?.kind else {
            return None;
        };
        if start >= children.len()
            || !suffix.failure_only
            || suffix.falls_through
            || failure_summary.end.is_none()
            || failure_summary.end != suffix.entry
        {
            return None;
        }
        // An inherited suffix is still needed by other arms of its enclosing
        // If. Certify the guard, but never project a deletion into that ancestor.
        source
            .positions
            .get(&node)
            .filter(|(parent, position)| *parent == sequence && *position + 1 == start)
            .map(|_| LiftedGuardContinuation {
                sequence,
                start,
                end: children.len(),
                pass_falls_through: pass_summary.falls_through,
            })
    } else {
        None
    };
    Some(LiftedGuard {
        block: true_edge.from,
        pass_nonzero: !then_failure,
        pass_edge,
        failure_edge,
        pass_node: pass.id,
        failure_node: failure.id,
        payload: LiftedGuardPayload::EmptyRevert,
        continuation,
    })
}

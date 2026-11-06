//! Bottom-up source-occurrence and local fallthrough certificates.
//! A loop is conservatively allowed to return to its containing Sequence;
//! break/continue close only the fragment in their own loop-body scope.
use super::Graph;
use crate::{EntityId, Ir, StructuredKind, StructuredNode, Terminator};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
pub(super) struct Summary {
    pub first_edge: Option<EntityId>,
    pub entry: Option<EntityId>,
    pub end: Option<EntityId>,
    pub falls_through: bool,
    pub failure_only: bool,
    empty: bool,
}
impl Summary {
    const EMPTY: Self = Self {
        first_edge: None,
        entry: None,
        end: None,
        falls_through: true,
        failure_only: true,
        empty: true,
    };
    fn then(self, right: Self) -> Self {
        if self.empty {
            return right;
        }
        if right.empty {
            return self;
        }
        let connected = !self.falls_through || (self.end.is_some() && self.end == right.entry);
        Self {
            first_edge: self.first_edge,
            entry: self.entry,
            end: if self.falls_through { right.end } else { None },
            falls_through: self.falls_through && right.falls_through,
            failure_only: self.failure_only && right.failure_only && connected,
            empty: false,
        }
    }
}
pub(super) struct Source<'a> {
    pub nodes: BTreeMap<EntityId, &'a StructuredNode>,
    pub summaries: BTreeMap<EntityId, Summary>,
    pub positions: BTreeMap<EntityId, (EntityId, usize)>,
    /// Next nonempty Sequence suffix on normal return from this occurrence.
    /// If arms inherit it; loop boundaries deliberately stop inheritance.
    pub follows: BTreeMap<EntityId, (EntityId, usize)>,
    pub suffixes: BTreeMap<(EntityId, usize), Summary>,
}
impl<'a> Source<'a> {
    pub fn build(
        ir: &'a Ir,
        graph: &Graph<'_>,
        ranks: &[Option<u32>],
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<Self> {
        if !pay(1) {
            return None;
        }
        let root = &ir.structured.as_ref()?.root;
        let mut result = Self {
            nodes: BTreeMap::new(),
            summaries: BTreeMap::new(),
            positions: BTreeMap::new(),
            follows: BTreeMap::new(),
            suffixes: BTreeMap::new(),
        };
        let mut pending = vec![(root, None)];
        let mut order = Vec::new();
        while let Some((node, follow)) = pending.pop() {
            let children = match &node.kind {
                StructuredKind::Sequence(children) => children.len(),
                StructuredKind::If { .. } | StructuredKind::While { .. } => 2,
                StructuredKind::Indirect { arms, .. } => arms.len(),
                _ => 0,
            };
            if result.nodes.len() == crate::MAX_STRUCTURED_WIRE_NODES
                || !pay(children.saturating_mul(4).saturating_add(3))
            {
                return None;
            }
            if result.nodes.insert(node.id, node).is_some() {
                return None;
            }
            if let Some(follow) = follow {
                result.follows.insert(node.id, follow);
            }
            order.push(node);
            match &node.kind {
                StructuredKind::Sequence(children) => {
                    for (i, child) in children.iter().enumerate() {
                        result.positions.insert(child.id, (node.id, i));
                        let next = if i + 1 < children.len() {
                            Some((node.id, i + 1))
                        } else {
                            follow
                        };
                        pending.push((child, next));
                    }
                }
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    pending.push((then_branch, follow));
                    pending.push((else_branch, follow));
                }
                StructuredKind::While { header, body, .. } => {
                    pending.push((header, None));
                    pending.push((body, None));
                }
                StructuredKind::Indirect { arms, .. } => {
                    pending.extend(arms.iter().map(|a| (&a.body, follow)))
                }
                _ => {}
            }
        }
        let failure = |block: EntityId| {
            graph
                .blocks
                .get(&block)
                .is_some_and(|index| ranks[*index].is_some())
        };
        for node in order.into_iter().rev() {
            if !pay(2) {
                return None;
            }
            let ordinary = Summary {
                empty: false,
                failure_only: false,
                ..Summary::EMPTY
            };
            let summary = match &node.kind {
                StructuredKind::Sequence(children) => {
                    if !pay(children.len().saturating_mul(4).saturating_add(1)) {
                        return None;
                    }
                    let mut suffix = Summary::EMPTY;
                    result.suffixes.insert((node.id, children.len()), suffix);
                    for (position, child) in children.iter().enumerate().rev() {
                        suffix = result.summaries.get(&child.id)?.then(suffix);
                        result.suffixes.insert((node.id, position), suffix);
                    }
                    suffix
                }
                StructuredKind::Block { block } => Summary {
                    entry: Some(*block),
                    end: Some(*block),
                    failure_only: failure(*block),
                    ..ordinary
                },
                StructuredKind::Transfer { edge } => {
                    let edge = graph.edges.get(edge)?;
                    Summary {
                        first_edge: Some(edge.id),
                        entry: Some(edge.from),
                        end: Some(edge.to),
                        failure_only: failure(edge.to),
                        ..ordinary
                    }
                }
                StructuredKind::Exit { block } => Summary {
                    entry: Some(*block),
                    falls_through: !graph.blocks.get(block).is_some_and(|index| {
                        matches!(
                            ir.blocks[*index].terminator,
                            Terminator::Exit { .. } | Terminator::Trap { .. }
                        )
                    }),
                    failure_only: failure(*block)
                        && graph.blocks.get(block).is_some_and(|index| {
                            matches!(
                                ir.blocks[*index].terminator,
                                Terminator::Exit {
                                    kind: crate::ExitKind::Revert,
                                    ..
                                }
                            )
                        }),
                    ..ordinary
                },
                StructuredKind::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    let first = *result.summaries.get(&then_branch.id)?;
                    let second = *result.summaries.get(&else_branch.id)?;
                    let branch = first.first_edge.and_then(|edge| graph.edges.get(&edge))
                        .zip(second.first_edge.and_then(|edge| graph.edges.get(&edge)))
                        .filter(|(a,b)| a.from == b.from)
                        .filter(|(a,b)| graph.blocks.get(&a.from).is_some_and(|index|
                            matches!(&ir.blocks[*index].terminator, Terminator::Branch {
                                condition: c, then_edges, else_edge: Some(e), unresolved: false,
                                invalid_targets, .. } if c == condition && then_edges.as_slice() == [a.id]
                                    && *e == b.id && invalid_targets.is_empty())))
                        .map(|(a,_)| a.from);
                    let end = match (first.falls_through, second.falls_through) {
                        (true, true) if first.end == second.end => first.end,
                        (true, false) => first.end,
                        (false, true) => second.end,
                        _ => None,
                    };
                    Summary {
                        entry: branch,
                        end,
                        falls_through: first.falls_through || second.falls_through,
                        failure_only: first.failure_only
                            && second.failure_only
                            && branch.is_some_and(failure),
                        ..ordinary
                    }
                }
                StructuredKind::Break | StructuredKind::Continue => Summary {
                    falls_through: false,
                    ..ordinary
                },
                StructuredKind::PartialExit { .. } => ordinary,
                StructuredKind::Indirect {
                    block,
                    arms,
                    unresolved,
                } => {
                    if !pay(arms.len().saturating_mul(3).saturating_add(1)) {
                        return None;
                    }
                    let mut falls = *unresolved || arms.is_empty();
                    let mut end = None;
                    let mut same_end = true;
                    let mut pure = !*unresolved && !arms.is_empty() && failure(*block);
                    let expected = graph.blocks.get(block).and_then(|index| {
                        match &ir.blocks[*index].terminator {
                            Terminator::Jump {
                                edges,
                                unresolved: false,
                                invalid_targets,
                                ..
                            } if invalid_targets.is_empty() => Some(edges),
                            _ => None,
                        }
                    });
                    pure &= expected.is_some_and(|edges| edges.len() == arms.len());
                    for (position, arm) in arms.iter().enumerate() {
                        let body = *result.summaries.get(&arm.body.id)?;
                        pure &= body.failure_only
                            && expected.is_some_and(|edges| {
                                body.first_edge == edges.get(position).copied()
                            });
                        if body.falls_through {
                            if falls && end != body.end {
                                same_end = false;
                            }
                            end = body.end;
                            falls = true;
                        }
                    }
                    Summary {
                        entry: Some(*block),
                        end: if same_end { end } else { None },
                        falls_through: falls,
                        failure_only: pure,
                        ..ordinary
                    }
                }
                // Internal loop transfers do not prove that the loop as a whole
                // cannot reach its parent's following suffix. Dispatcher states
                // likewise need a separate proof, outside this shape contract.
                StructuredKind::While { .. } | StructuredKind::Dispatcher { .. } => ordinary,
            };
            result.summaries.insert(node.id, summary);
        }
        Some(result)
    }
}

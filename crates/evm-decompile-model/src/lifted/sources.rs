//! Lightweight source ownership admission, independent of semantic failure proof.
//!
//! Guard overlays may select only their own raw If arms. Suffix suppression is
//! checked against the containing Sequence and raw normal fallthrough; recorded
//! validation status is not evidence for these render-safety properties.
use super::{LiftedBlock, LiftedExpression, LiftedModelError, LiftedProgram, LiftedType};
use crate::{Edge, EdgeKind, EntityId, Ir, StructuredKind, StructuredNode, Terminator};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct GuardSources<'a> {
    pub nodes: BTreeMap<EntityId, &'a StructuredNode>,
    pub omissions: BTreeMap<(EntityId, usize), usize>,
    pub error: Option<LiftedModelError>,
}
#[derive(Clone, Copy)]
struct Shape {
    first_edge: Option<EntityId>,
    falls_through: bool,
    empty: bool,
}
impl Shape {
    const EMPTY: Self = Self {
        first_edge: None,
        falls_through: true,
        empty: true,
    };
    const ORDINARY: Self = Self {
        empty: false,
        ..Self::EMPTY
    };
}
impl<'a> GuardSources<'a> {
    pub fn new(
        program: &LiftedProgram,
        ir: &'a Ir,
        expressions: &BTreeMap<EntityId, &LiftedExpression>,
        blocks: &BTreeMap<EntityId, &LiftedBlock>,
        edges: &BTreeMap<EntityId, &Edge>,
    ) -> Self {
        let mut result = Self {
            nodes: BTreeMap::new(),
            omissions: BTreeMap::new(),
            error: None,
        };
        if let Err(error) = result.check(program, ir, expressions, blocks, edges) {
            // Publication is all-or-nothing: a forged guard cannot leave an
            // earlier suffix omission active in this diagnostic/raw fallback.
            result.omissions.clear();
            result.error = Some(error);
        }
        result
    }
    fn check(
        &mut self,
        program: &LiftedProgram,
        ir: &'a Ir,
        expressions: &BTreeMap<EntityId, &LiftedExpression>,
        blocks: &BTreeMap<EntityId, &LiftedBlock>,
        edges: &BTreeMap<EntityId, &Edge>,
    ) -> Result<(), LiftedModelError> {
        let guarded = program.controls.iter().any(|c| c.guard.is_some());
        let root = ir.structured.as_ref().map(|s| &s.root);
        if guarded && (root.is_none() || program.record_count() > super::MAX_LIFTED_RECORDS) {
            return Err(LiftedModelError::GuardSource(
                root.map_or(EntityId(0), |n| n.id),
            ));
        }
        let mut order = Vec::new();
        let mut positions = BTreeMap::new();
        let mut pending: Vec<_> = root.into_iter().map(|n| (n, 0_usize, 1_usize)).collect();
        while let Some((node, loops, depth)) = pending.pop() {
            let child_count = match &node.kind {
                StructuredKind::Sequence(children) => children.len(),
                StructuredKind::If { .. } | StructuredKind::While { .. } => 2,
                StructuredKind::Indirect { arms, .. } => arms.len(),
                _ => 0,
            };
            if depth > crate::MAX_STRUCTURED_WIRE_DEPTH
                || order
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(child_count)
                    .saturating_add(1)
                    > crate::MAX_STRUCTURED_WIRE_NODES
            {
                return Err(LiftedModelError::GuardSource(node.id));
            }
            if node.id.0 == 0 || self.nodes.insert(node.id, node).is_some() {
                return Err(LiftedModelError::GuardSource(node.id));
            }
            order.push(node);
            match &node.kind {
                StructuredKind::Sequence(children) => {
                    for (position, child) in children.iter().enumerate() {
                        positions.insert(child.id, (node.id, position));
                        pending.push((child, loops, depth + 1));
                    }
                }
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => {
                    pending.push((then_branch, loops, depth + 1));
                    pending.push((else_branch, loops, depth + 1));
                }
                StructuredKind::While { header, body, .. } => {
                    pending.push((header, loops + 1, depth + 1));
                    pending.push((body, loops + 1, depth + 1));
                }
                StructuredKind::Indirect { arms, .. } => {
                    pending.extend(arms.iter().map(|a| (&a.body, loops, depth + 1)))
                }
                StructuredKind::Break | StructuredKind::Continue if guarded && loops == 0 => {
                    return Err(LiftedModelError::GuardSource(node.id));
                }
                _ => {}
            }
        }
        if !guarded {
            return Ok(());
        }
        let raw: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
        let values: BTreeSet<_> = ir.values.iter().map(|v| v.id).collect();
        if raw.len() != ir.blocks.len()
            || edges.len() != ir.edges.len()
            || expressions.len() != program.expressions.len()
            || blocks.len() != program.blocks.len()
        {
            return Err(LiftedModelError::GuardSource(
                root.map_or(EntityId(0), |n| n.id),
            ));
        }
        let mut shapes = BTreeMap::<EntityId, Shape>::new();
        for node in order.into_iter().rev() {
            let shape = match &node.kind {
                StructuredKind::Sequence(children) => {
                    let mut result = Shape::EMPTY;
                    for child in children {
                        let child = shapes[&child.id];
                        if result.empty && !child.empty {
                            result.first_edge = child.first_edge;
                        }
                        result.empty &= child.empty;
                        result.falls_through &= child.falls_through;
                    }
                    result
                }
                StructuredKind::Transfer { edge } => Shape {
                    first_edge: Some(*edge),
                    ..Shape::ORDINARY
                },
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => Shape {
                    falls_through: shapes[&then_branch.id].falls_through
                        || shapes[&else_branch.id].falls_through,
                    ..Shape::ORDINARY
                },
                StructuredKind::Exit { block } => Shape {
                    falls_through: !raw.get(block).is_some_and(|b| {
                        matches!(
                            b.terminator,
                            Terminator::Exit { .. } | Terminator::Trap { .. }
                        )
                    }),
                    ..Shape::ORDINARY
                },
                StructuredKind::Break | StructuredKind::Continue => Shape {
                    falls_through: false,
                    ..Shape::ORDINARY
                },
                StructuredKind::Indirect {
                    arms, unresolved, ..
                } => Shape {
                    falls_through: *unresolved
                        || arms.is_empty()
                        || arms.iter().any(|a| shapes[&a.body.id].falls_through),
                    ..Shape::ORDINARY
                },
                // Internal loop/dispatcher transfers close their own scope only.
                // Unknown exits are not used to suppress a following suffix.
                _ => Shape::ORDINARY,
            };
            shapes.insert(node.id, shape);
        }
        let mut controls = BTreeSet::new();
        for control in &program.controls {
            let fail = || LiftedModelError::GuardSource(control.node);
            if !controls.insert(control.node) {
                return Err(fail());
            }
            let Some(guard) = &control.guard else {
                continue;
            };
            let node = self.nodes.get(&control.node).ok_or_else(fail)?;
            let StructuredKind::If {
                condition,
                then_branch,
                else_branch,
            } = &node.kind
            else {
                return Err(fail());
            };
            let (pass, failure) = if guard.pass_nonzero {
                (then_branch, else_branch)
            } else {
                (else_branch, then_branch)
            };
            if guard.pass_node != pass.id || guard.failure_node != failure.id {
                return Err(fail());
            }
            let branch = raw.get(&guard.block).ok_or_else(fail)?;
            let Terminator::Branch {
                condition: source_condition,
                then_edges,
                else_edge: Some(other),
                unresolved: false,
                invalid_targets,
                ..
            } = &branch.terminator
            else {
                return Err(fail());
            };
            if condition != source_condition
                || !values.contains(condition)
                || then_edges.len() != 1
                || !invalid_targets.is_empty()
            {
                return Err(fail());
            }
            let (pass_edge, failure_edge) = if guard.pass_nonzero {
                (then_edges[0], *other)
            } else {
                (*other, then_edges[0])
            };
            if guard.pass_edge != pass_edge
                || guard.failure_edge != failure_edge
                || shapes[&pass.id].first_edge != Some(pass_edge)
                || shapes[&failure.id].first_edge != Some(failure_edge)
            {
                return Err(fail());
            }
            for (edge, kind) in [(then_edges[0], EdgeKind::True), (*other, EdgeKind::False)] {
                if !edges.get(&edge).is_some_and(|e| {
                    e.from == guard.block && e.kind == kind && raw.contains_key(&e.to)
                }) {
                    return Err(fail());
                }
            }
            if !blocks
                .get(&guard.block)
                .is_some_and(|b| b.condition == Some(control.condition))
                || !expressions
                    .get(&control.condition)
                    .is_some_and(|e| e.block == guard.block && e.ty == LiftedType::Predicate)
            {
                return Err(fail());
            }
            if let Some(tail) = &guard.continuation {
                let (parent, position) = positions.get(&control.node).copied().ok_or_else(fail)?;
                let StructuredKind::Sequence(children) = &self.nodes[&parent].kind else {
                    return Err(fail());
                };
                if tail.sequence != parent
                    || tail.start != position + 1
                    || tail.end != children.len()
                    || tail.start >= tail.end
                    || !shapes[&failure.id].falls_through
                    || tail.pass_falls_through != shapes[&pass.id].falls_through
                {
                    return Err(fail());
                }
                if !tail.pass_falls_through
                    && self
                        .omissions
                        .insert((parent, tail.start), tail.end)
                        .is_some()
                {
                    return Err(fail());
                }
            }
        }
        Ok(())
    }
}

impl LiftedProgram {
    /// Check guard source ownership/normal fallthrough without re-proving failure
    /// semantics. Applies regardless of an artifact's recorded validation status.
    pub fn validate_guard_sources(&self, ir: &Ir) -> Result<(), LiftedModelError> {
        if self.controls.iter().all(|c| c.guard.is_none()) {
            return Ok(());
        }
        let expressions = self.expressions.iter().map(|e| (e.id, e)).collect();
        let blocks = self.blocks.iter().map(|b| (b.block, b)).collect();
        let edges = ir.edges.iter().map(|e| (e.id, e)).collect();
        match GuardSources::new(self, ir, &expressions, &blocks, &edges).error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

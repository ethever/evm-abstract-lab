//! Original CFG control transitions. This module does not consult region facts.

use super::{PlanError, paid};
use crate::{Block, Edge, EntityId, Ir, Terminator};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Cursor {
    Empty,
    Body(EntityId),
    Terminator(EntityId),
    Target(EntityId),
    Edge(EntityId),
    EndOfCode(EntityId),
}
impl Cursor {
    pub(super) fn entity(self) -> EntityId {
        match self {
            Self::Empty => EntityId(0),
            Self::Body(id)
            | Self::Terminator(id)
            | Self::Target(id)
            | Self::Edge(id)
            | Self::EndOfCode(id) => id,
        }
    }
}

pub(super) struct Source<'a> {
    pub(super) ir: &'a Ir,
    pub(super) blocks: BTreeMap<EntityId, &'a Block>,
    pub(super) edges: BTreeMap<EntityId, &'a Edge>,
    pub(super) entities: BTreeSet<EntityId>,
}
impl<'a> Source<'a> {
    pub(super) fn new(ir: &'a Ir, pay: &mut impl FnMut(usize) -> bool) -> Result<Self, PlanError> {
        paid(
            pay,
            ir.blocks
                .len()
                .saturating_add(ir.edges.len())
                .saturating_add(ir.values.len()),
        )?;
        let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
        let edges: BTreeMap<_, _> = ir.edges.iter().map(|e| (e.id, e)).collect();
        if blocks.len() != ir.blocks.len() || edges.len() != ir.edges.len() {
            return Err(PlanError::Invalid(EntityId(0)));
        }
        let mut entities: BTreeSet<_> = blocks.keys().chain(edges.keys()).copied().collect();
        entities.extend(ir.values.iter().map(|v| v.id));
        for block in &ir.blocks {
            paid(pay, block.operations.len().saturating_add(1))?;
            entities.extend(block.operations.iter().map(|o| o.id));
            let mut seen = BTreeSet::new();
            for id in block.terminator.edges() {
                paid(pay, 1)?;
                let edge = edges.get(&id).ok_or(PlanError::Invalid(id))?;
                if !seen.insert(id) || edge.from != block.id || !blocks.contains_key(&edge.to) {
                    return Err(PlanError::Invalid(id));
                }
            }
        }
        if ir.entry.is_some_and(|entry| !blocks.contains_key(&entry)) {
            return Err(PlanError::Invalid(ir.entry.unwrap()));
        }
        Ok(Self {
            ir,
            blocks,
            edges,
            entities,
        })
    }
    pub(super) fn entry(&self) -> Cursor {
        self.ir.entry.map_or(Cursor::Empty, Cursor::Body)
    }
    pub(super) fn edge(&self, id: EntityId) -> Result<&'a Edge, PlanError> {
        self.edges.get(&id).copied().ok_or(PlanError::Invalid(id))
    }
    fn block(&self, id: EntityId) -> Result<&'a Block, PlanError> {
        self.blocks.get(&id).copied().ok_or(PlanError::Invalid(id))
    }
    pub(super) fn reachable(
        &self,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<BTreeSet<EntityId>, PlanError> {
        let mut result = BTreeSet::new();
        let mut pending: Vec<_> = self.ir.entry.into_iter().collect();
        while let Some(id) = pending.pop() {
            paid(pay, 1)?;
            if result.insert(id) {
                for edge in self.block(id)?.terminator.edges() {
                    paid(pay, 1)?;
                    pending.push(self.edge(edge)?.to);
                }
            }
        }
        Ok(result)
    }
    pub(super) fn transfer(&self, cursor: Cursor, edge: EntityId) -> Result<(), PlanError> {
        let actual = self.edge(edge)?;
        let selected = match cursor {
            Cursor::Edge(expected) => expected == edge,
            Cursor::Terminator(block) => match &self.block(block)?.terminator {
                Terminator::Fallthrough { edge: expected } => *expected == edge,
                Terminator::Jump {
                    edges,
                    unresolved: false,
                    invalid_targets,
                    ..
                } => edges.as_slice() == [edge] && invalid_targets.is_empty(),
                _ => false,
            },
            Cursor::Target(block) => match &self.block(block)?.terminator {
                Terminator::Branch {
                    then_edges,
                    unresolved: false,
                    invalid_targets,
                    ..
                } => then_edges.as_slice() == [edge] && invalid_targets.is_empty(),
                _ => false,
            },
            _ => false,
        };
        let from = match cursor {
            Cursor::Terminator(id) | Cursor::Target(id) => Some(id),
            Cursor::Edge(id) => Some(self.edge(id)?.from),
            _ => None,
        };
        if !selected || from != Some(actual.from) {
            return Err(PlanError::Invalid(edge));
        }
        Ok(())
    }
    pub(super) fn condition(
        &self,
        cursor: Cursor,
        condition: EntityId,
    ) -> Result<(Cursor, Cursor), PlanError> {
        let Cursor::Terminator(block) = cursor else {
            return Err(PlanError::Invalid(condition));
        };
        let Terminator::Branch {
            condition: expected,
            else_edge,
            ..
        } = &self.block(block)?.terminator
        else {
            return Err(PlanError::Invalid(block));
        };
        if *expected != condition {
            return Err(PlanError::Invalid(condition));
        }
        Ok((
            Cursor::Target(block),
            else_edge.map_or(Cursor::EndOfCode(block), Cursor::Edge),
        ))
    }
    pub(super) fn exit(
        &self,
        cursor: Cursor,
        block: EntityId,
        partial: bool,
    ) -> Result<(), PlanError> {
        let valid = match cursor {
            Cursor::EndOfCode(expected) => expected == block && !partial,
            Cursor::Terminator(expected) if expected == block => {
                match &self.block(block)?.terminator {
                    Terminator::Exit { .. } | Terminator::Trap { .. } => !partial,
                    Terminator::Pending => partial,
                    Terminator::Jump {
                        edges, unresolved, ..
                    } => edges.is_empty() && partial == *unresolved,
                    Terminator::Branch {
                        then_edges,
                        else_edge: None,
                        unresolved,
                        ..
                    } => then_edges.is_empty() && partial == *unresolved,
                    _ => false,
                }
            }
            Cursor::Target(expected) if expected == block => match &self.block(block)?.terminator {
                Terminator::Branch {
                    then_edges,
                    unresolved,
                    ..
                } => then_edges.is_empty() && partial == *unresolved,
                _ => false,
            },
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(PlanError::Invalid(block))
        }
    }
    pub(super) fn indirect(
        &self,
        cursor: Cursor,
        block: EntityId,
        arms: &[(usize, usize)],
        unresolved: bool,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Vec<(usize, Cursor)>, PlanError> {
        paid(pay, arms.len().saturating_mul(3).saturating_add(1))?;
        let (edges, expected) = match (cursor, &self.block(block)?.terminator) {
            (
                Cursor::Terminator(id),
                Terminator::Jump {
                    edges, unresolved, ..
                },
            ) if id == block => (edges, *unresolved),
            (
                Cursor::Target(id),
                Terminator::Branch {
                    then_edges,
                    unresolved,
                    ..
                },
            ) if id == block => (then_edges, *unresolved),
            _ => return Err(PlanError::Invalid(block)),
        };
        if unresolved != expected || edges.len() != arms.len() {
            return Err(PlanError::Invalid(block));
        }
        let mut destinations = BTreeMap::new();
        for edge in edges {
            let to = self.edge(*edge)?.to;
            let pc = self.block(to)?.source.ok_or(PlanError::Invalid(to))?.pc;
            if destinations.insert(pc, *edge).is_some() {
                return Err(PlanError::Invalid(block));
            }
        }
        let mut result = Vec::new();
        for (pc, next) in arms {
            let edge = destinations.remove(pc).ok_or(PlanError::Invalid(block))?;
            result.push((*next, Cursor::Edge(edge)));
        }
        if !destinations.is_empty() {
            return Err(PlanError::Invalid(block));
        }
        Ok(result)
    }
    pub(super) fn dispatch(
        &self,
        cursor: Cursor,
        block: EntityId,
        transitions: &[(EntityId, usize)],
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), PlanError> {
        if cursor != Cursor::Terminator(block) {
            return Err(PlanError::Invalid(block));
        }
        paid(pay, transitions.len().saturating_mul(3).saturating_add(1))?;
        // A Dispatcher performs this original block's choose() itself. Normal,
        // trap, pending, unknown/invalid-target and EOF outcomes are retained by
        // that exact source identity, including cases without known edges.
        let expected: BTreeSet<_> = self.block(block)?.terminator.edges().into_iter().collect();
        let supplied: BTreeSet<_> = transitions.iter().map(|(edge, _)| *edge).collect();
        if expected != supplied || supplied.len() != transitions.len() {
            return Err(PlanError::Invalid(block));
        }
        Ok(())
    }
}

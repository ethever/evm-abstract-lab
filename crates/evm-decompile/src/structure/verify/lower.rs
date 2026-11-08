//! Interpret plan syntax as control actions, retaining lexical escape behavior.
use super::source::Source;
use super::{MAX_STATES, Plan, PlanError, paid};
use crate::{EntityId, StructuredKind as Kind};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(super) enum Action {
    Go(usize),
    End,
    Body {
        block: EntityId,
        next: usize,
    },
    Transfer {
        edge: EntityId,
        next: usize,
    },
    Condition {
        condition: EntityId,
        nonzero: usize,
        zero: usize,
    },
    Exit {
        block: EntityId,
        partial: bool,
    },
    Indirect {
        block: EntityId,
        arms: Vec<(usize, usize)>,
        unresolved: bool,
    },
    Dispatch {
        block: EntityId,
        transitions: Vec<(EntityId, usize)>,
    },
}
pub(super) struct Machine {
    pub(super) entry: usize,
    pub(super) actions: Vec<Action>,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Escape {
    Root,
    Sequence {
        anchor: Option<EntityId>,
        next: usize,
        outer: usize,
    },
    LoopBody {
        entry: Option<EntityId>,
        repeat: usize,
        next: usize,
    },
}
#[derive(Clone, Copy)]
struct LoopScope {
    leave: usize,
    repeat: usize,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    node: usize,
    next: usize,
    scope: Option<usize>,
    escape: usize,
}

struct Lower<'a, 'b, P> {
    plan: &'a Plan,
    source: &'a Source<'b>,
    pay: P,
    first: Vec<Option<EntityId>>,
    empty_sequences: Vec<bool>,
    actions: Vec<Option<Action>>,
    scopes: Vec<LoopScope>,
    escapes: Vec<Escape>,
    escape_ids: BTreeMap<Escape, usize>,
    scheduled: BTreeMap<Key, usize>,
    todo: Vec<(Key, usize)>,
}

impl Machine {
    pub(super) fn lower(
        plan: &Plan,
        source: &Source<'_>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, PlanError> {
        let (first, empty_sequences) = admit(plan, source, pay)?;
        let mut lower = Lower {
            plan,
            source,
            pay,
            first,
            empty_sequences,
            actions: vec![Some(Action::End)],
            scopes: Vec::new(),
            escapes: vec![Escape::Root],
            escape_ids: BTreeMap::from([(Escape::Root, 0)]),
            scheduled: BTreeMap::new(),
            todo: Vec::new(),
        };
        let entry = lower.schedule(plan.root.0, 0, None, 0)?;
        while let Some((key, slot)) = lower.todo.pop() {
            lower.build(key, slot)?;
        }
        let actions = lower
            .actions
            .into_iter()
            .map(|action| action.ok_or(PlanError::Invalid(EntityId(0))))
            .collect::<Result<Vec<_>, _>>()?;
        // Coinduction on the product is valid only when plan cycles make a
        // source action. Otherwise an empty infinite loop could hide all work.
        let mut done = vec![false; actions.len()];
        let mut visiting = vec![0; actions.len()];
        for start in 0..actions.len() {
            let mut path = Vec::new();
            let mut position = start;
            while !done[position] {
                paid(lower.pay, 1)?;
                if visiting[position] == start + 1 {
                    return Err(PlanError::Invalid(EntityId(0)));
                }
                visiting[position] = start + 1;
                path.push(position);
                let Action::Go(next) = actions[position] else {
                    break;
                };
                position = next;
            }
            for position in path {
                done[position] = true;
            }
        }
        Ok(Self { entry, actions })
    }
}

fn admit(
    plan: &Plan,
    source: &Source<'_>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<(Vec<Option<EntityId>>, Vec<bool>), PlanError> {
    paid(pay, plan.nodes.len().saturating_add(plan.regions.len()))?;
    if plan.nodes.len() > MAX_STATES {
        return Err(PlanError::Limit);
    }
    if plan.root.0 >= plan.nodes.len() {
        return Err(PlanError::Invalid(EntityId(0)));
    }
    let mut first: Vec<Option<EntityId>> = Vec::with_capacity(plan.nodes.len());
    let mut empty_sequences: Vec<bool> = Vec::with_capacity(plan.nodes.len());
    for (index, node) in plan.nodes.iter().enumerate() {
        paid(pay, node.sources.len().saturating_add(1))?;
        if node.sources.iter().any(|id| !source.entities.contains(id)) {
            return Err(PlanError::Invalid(EntityId(0)));
        }
        let count = match &node.kind {
            Kind::Sequence(children) => children.len(),
            Kind::If { .. } | Kind::While { .. } => 2,
            Kind::Indirect { arms, .. } => arms.len(),
            _ => 0,
        };
        paid(pay, count.saturating_add(1))?;
        let children: Vec<_> = match &node.kind {
            Kind::Sequence(children) => children.iter().map(|id| id.0).collect(),
            Kind::If {
                then_branch,
                else_branch,
                ..
            } => vec![then_branch.0, else_branch.0],
            Kind::While { header, body, .. } => vec![header.0, body.0],
            Kind::Indirect { arms, .. } => arms.iter().map(|arm| arm.body.0).collect(),
            _ => Vec::new(),
        };
        if children.iter().any(|child| *child >= index) {
            return Err(PlanError::Invalid(EntityId(0)));
        }
        let entry = match &node.kind {
            Kind::Block { block }
            | Kind::Exit { block }
            | Kind::PartialExit { block }
            | Kind::Indirect { block, .. } => Some(*block),
            Kind::Transfer { edge } => Some(source.edge(*edge)?.from),
            Kind::Sequence(children) => children.iter().find_map(|child| first[child.0]),
            Kind::While { header, body, .. } => first[header.0].or(first[body.0]),
            Kind::Dispatcher { entry, .. } => Some(*entry),
            _ => None,
        };
        if entry.is_some_and(|block| !source.blocks.contains_key(&block)) {
            return Err(PlanError::Invalid(entry.unwrap()));
        }
        first.push(entry);
        empty_sequences.push(matches!(&node.kind, Kind::Sequence(children) if children.iter().all(|child| empty_sequences[child.0])));
    }
    // Region annotations are not executable certificates or permission to
    // omit source. Only the full contextual product grants that permission.
    for region in &plan.regions {
        if !source.blocks.contains_key(&region.entry)
            || region
                .continuation
                .is_some_and(|id| !source.blocks.contains_key(&id))
            || region
                .loop_header
                .is_some_and(|id| !source.blocks.contains_key(&id))
            || region.body.0 >= plan.nodes.len()
            || first[region.body.0].is_some_and(|entry| entry != region.entry)
        {
            return Err(PlanError::Invalid(region.entry));
        }
    }
    Ok((first, empty_sequences))
}

impl<P: FnMut(usize) -> bool> Lower<'_, '_, P> {
    fn sequence_children(
        &mut self,
        children: Vec<super::super::plan::PlanId>,
    ) -> Result<Vec<super::super::plan::PlanId>, PlanError> {
        // Materialization flattens nested Sequences. Check that exact grammar,
        // since Dispatcher Escaped routing depends on each actual child anchor.
        // An exponentially shared empty subtree has no emitted children.
        let mut selected = Vec::new();
        let mut pending: Vec<_> = children.into_iter().rev().collect();
        while let Some(child) = pending.pop() {
            paid(&mut self.pay, 1)?;
            if self.empty_sequences[child.0] {
                continue;
            }
            if let Kind::Sequence(nested) = &self.plan.nodes[child.0].kind {
                if selected
                    .len()
                    .saturating_add(pending.len())
                    .saturating_add(nested.len())
                    > MAX_STATES
                {
                    return Err(PlanError::Limit);
                }
                paid(&mut self.pay, nested.len())?;
                pending.extend(nested.iter().rev().copied());
            } else {
                if selected.len() >= MAX_STATES {
                    return Err(PlanError::Limit);
                }
                selected.push(child);
            }
        }
        Ok(selected)
    }
    fn slot(&mut self) -> Result<usize, PlanError> {
        paid(&mut self.pay, 1)?;
        if self.actions.len() >= MAX_STATES {
            return Err(PlanError::Limit);
        }
        let slot = self.actions.len();
        self.actions.push(None);
        Ok(slot)
    }
    fn action(&mut self, action: Action) -> Result<usize, PlanError> {
        let slot = self.slot()?;
        self.actions[slot] = Some(action);
        Ok(slot)
    }
    fn schedule(
        &mut self,
        node: usize,
        next: usize,
        scope: Option<usize>,
        escape: usize,
    ) -> Result<usize, PlanError> {
        let key = Key {
            node,
            next,
            scope,
            escape,
        };
        if let Some(slot) = self.scheduled.get(&key) {
            return Ok(*slot);
        }
        let slot = self.slot()?;
        self.scheduled.insert(key, slot);
        self.todo.push((key, slot));
        Ok(slot)
    }
    fn handler(&mut self, escape: Escape) -> Result<usize, PlanError> {
        if let Some(id) = self.escape_ids.get(&escape) {
            return Ok(*id);
        }
        paid(&mut self.pay, 1)?;
        let id = self.escapes.len();
        self.escapes.push(escape);
        self.escape_ids.insert(escape, id);
        Ok(id)
    }
    fn escaped(&mut self, mut handler: usize, target: EntityId) -> Result<usize, PlanError> {
        loop {
            paid(&mut self.pay, 1)?;
            match self.escapes[handler] {
                Escape::Root => return Err(PlanError::Invalid(target)),
                Escape::Sequence {
                    anchor,
                    next,
                    outer,
                } => {
                    if anchor == Some(target) {
                        return Ok(next);
                    }
                    handler = outer;
                }
                Escape::LoopBody {
                    entry,
                    repeat,
                    next,
                } => return Ok(if entry == Some(target) { repeat } else { next }),
            }
        }
    }
    fn build(&mut self, key: Key, slot: usize) -> Result<(), PlanError> {
        let node = &self.plan.nodes[key.node];
        let work = match &node.kind {
            Kind::Sequence(children) => children.len(),
            Kind::Indirect { arms, .. } => arms.len(),
            Kind::Dispatcher { cases, .. } => cases.iter().fold(cases.len(), |n, case| {
                n.saturating_add(case.transitions.len())
            }),
            _ => 1,
        };
        paid(&mut self.pay, work.saturating_add(1))?;
        let kind = node.kind.clone();
        let action = match kind {
            Kind::Sequence(children) => {
                let mut next = key.next;
                let mut escape = key.escape;
                for child in self.sequence_children(children)?.into_iter().rev() {
                    let start = self.schedule(child.0, next, key.scope, escape)?;
                    escape = self.handler(Escape::Sequence {
                        anchor: self.first[child.0],
                        next: start,
                        outer: key.escape,
                    })?;
                    next = start;
                }
                Action::Go(next)
            }
            Kind::Block { block } => Action::Body {
                block,
                next: key.next,
            },
            Kind::Transfer { edge } => Action::Transfer {
                edge,
                next: key.next,
            },
            Kind::If {
                condition,
                then_branch,
                else_branch,
            } => Action::Condition {
                condition,
                nonzero: self.schedule(then_branch.0, key.next, key.scope, key.escape)?,
                zero: self.schedule(else_branch.0, key.next, key.scope, key.escape)?,
            },
            Kind::Exit { block } => Action::Exit {
                block,
                partial: false,
            },
            Kind::PartialExit { block } => Action::Exit {
                block,
                partial: true,
            },
            Kind::Break => {
                Action::Go(self.scopes[key.scope.ok_or(PlanError::Invalid(EntityId(0)))?].leave)
            }
            Kind::Continue => {
                Action::Go(self.scopes[key.scope.ok_or(PlanError::Invalid(EntityId(0)))?].repeat)
            }
            Kind::Indirect {
                block,
                arms,
                unresolved,
            } => {
                let mut targets = Vec::with_capacity(arms.len());
                for arm in arms {
                    targets.push((
                        arm.target_pc,
                        self.schedule(arm.body.0, key.next, key.scope, key.escape)?,
                    ));
                }
                Action::Indirect {
                    block,
                    arms: targets,
                    unresolved,
                }
            }
            Kind::While {
                header,
                condition,
                condition_nonzero,
                body,
                exit_edge,
            } => {
                let scope = self.scopes.len();
                self.scopes.push(LoopScope {
                    leave: key.next,
                    repeat: slot,
                });
                let body_escape = self.handler(Escape::LoopBody {
                    entry: self.first[header.0].or(self.first[body.0]),
                    repeat: slot,
                    next: key.next,
                })?;
                let body = self.schedule(body.0, slot, Some(scope), body_escape)?;
                let after_header = if let Some(condition) = condition {
                    let edge = exit_edge.ok_or(PlanError::Invalid(condition))?;
                    let exit = self.action(Action::Transfer {
                        edge,
                        next: key.next,
                    })?;
                    self.action(Action::Condition {
                        condition,
                        nonzero: if condition_nonzero { body } else { exit },
                        zero: if condition_nonzero { exit } else { body },
                    })?
                } else {
                    if exit_edge.is_some() {
                        return Err(PlanError::Invalid(EntityId(0)));
                    }
                    body
                };
                Action::Go(self.schedule(header.0, after_header, key.scope, key.escape)?)
            }
            Kind::Dispatcher { entry, cases } => {
                let mut slots = BTreeMap::new();
                for case in &cases {
                    if !self.source.blocks.contains_key(&case.block)
                        || slots.contains_key(&case.block)
                    {
                        return Err(PlanError::Invalid(case.block));
                    }
                    slots.insert(case.block, self.slot()?);
                }
                let start = *slots.get(&entry).ok_or(PlanError::Invalid(entry))?;
                for case in cases {
                    let expected = self.source.blocks[&case.block].terminator.edges();
                    let mut supplied = BTreeMap::new();
                    for transition in &case.transitions {
                        let raw = self.source.edge(transition.edge)?;
                        if raw.from != case.block
                            || raw.to != transition.target
                            || supplied
                                .insert(transition.edge, transition.target)
                                .is_some()
                        {
                            return Err(PlanError::Invalid(transition.edge));
                        }
                    }
                    if expected.len() != supplied.len()
                        || expected.iter().any(|edge| !supplied.contains_key(edge))
                    {
                        return Err(PlanError::Invalid(case.block));
                    }
                    let mut transitions = Vec::with_capacity(case.transitions.len());
                    for transition in case.transitions {
                        let next = match slots.get(&transition.target) {
                            Some(slot) => *slot,
                            None => self.escaped(key.escape, transition.target)?,
                        };
                        let transfer = self.action(Action::Transfer {
                            edge: transition.edge,
                            next,
                        })?;
                        transitions.push((transition.edge, transfer));
                    }
                    let control = self.action(Action::Dispatch {
                        block: case.block,
                        transitions,
                    })?;
                    self.actions[slots[&case.block]] = Some(Action::Body {
                        block: case.block,
                        next: control,
                    });
                }
                Action::Go(start)
            }
        };
        self.actions[slot] = Some(action);
        Ok(())
    }
}

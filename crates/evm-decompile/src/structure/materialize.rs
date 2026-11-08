//! Quote the expanded representation, then allocate owned source occurrences.
use super::plan::{MAX_SOURCE_REFERENCES, Plan, PlanError, PlanId, Quote, quote_node};
use crate::{
    EntityId, IndirectArm, Progress, StructuredKind as K, StructuredNode, artifact::Builder,
};
use std::collections::BTreeMap;

pub(super) struct Materialized {
    pub root: StructuredNode,
    pub parents: BTreeMap<EntityId, Vec<EntityId>>,
}

pub(super) fn materialize(
    plan: &Plan,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Result<Materialized, PlanError> {
    if !owner.pay(plan.nodes.len().saturating_mul(8)) {
        return Err(PlanError::Limit);
    }
    let mut quotes = Vec::with_capacity(plan.nodes.len());
    for node in &plan.nodes {
        quotes.push(quote_node(node, &plan.nodes, &quotes));
    }
    let quote: Quote = quotes[plan.root.0];
    if quote.nodes > crate::MAX_STRUCTURED_WIRE_NODES
        || quote.depth > crate::MAX_STRUCTURED_WIRE_DEPTH
        || quote.references > MAX_SOURCE_REFERENCES
    {
        return Err(PlanError::Limit);
    }
    if !owner.pay(
        quote
            .units()
            .saturating_mul(8)
            .saturating_add(quote.visits.saturating_mul(4)),
    ) {
        return Err(PlanError::Limit);
    }
    enum Task {
        Enter(PlanId),
        Leave(PlanId, usize),
    }
    let mut tasks = vec![Task::Enter(plan.root)];
    let mut built = Vec::<StructuredNode>::new();
    let mut parents = BTreeMap::new();
    while let Some(task) = tasks.pop() {
        if !owner.pay(0) {
            return Err(PlanError::Limit);
        }
        match task {
            Task::Enter(id) => {
                let mut children = match &plan.nodes[id.0].kind {
                    K::Sequence(v) => {
                        let mut selected = Vec::new();
                        let mut pending: Vec<_> = v.iter().rev().copied().collect();
                        while let Some(child) = pending.pop() {
                            if !owner.pay(0) {
                                return Err(PlanError::Limit);
                            }
                            if let K::Sequence(nested) = &plan.nodes[child.0].kind {
                                if quotes[child.0].nodes > 1 {
                                    pending.extend(nested.iter().rev().copied());
                                }
                            } else {
                                selected.push(child);
                            }
                        }
                        selected
                    }
                    K::If {
                        then_branch,
                        else_branch,
                        ..
                    } => vec![**then_branch, **else_branch],
                    K::While { header, body, .. } => vec![**header, **body],
                    K::Indirect { arms, .. } => arms.iter().map(|a| a.body).collect(),
                    _ => Vec::new(),
                };
                tasks.push(Task::Leave(id, children.len()));
                tasks.extend(children.drain(..).rev().map(Task::Enter));
            }
            Task::Leave(id, child_count) => {
                let children = built.split_off(built.len() - child_count);
                let mut children = children.into_iter();
                let node = &plan.nodes[id.0];
                let kind = match &node.kind {
                    K::Sequence(_) => K::Sequence(children.collect()),
                    K::Block { block } => K::Block { block: *block },
                    K::Transfer { edge } => K::Transfer { edge: *edge },
                    K::If { condition, .. } => K::If {
                        condition: *condition,
                        then_branch: Box::new(children.next().unwrap()),
                        else_branch: Box::new(children.next().unwrap()),
                    },
                    K::While {
                        condition,
                        condition_nonzero,
                        exit_edge,
                        ..
                    } => K::While {
                        header: Box::new(children.next().unwrap()),
                        condition: *condition,
                        condition_nonzero: *condition_nonzero,
                        body: Box::new(children.next().unwrap()),
                        exit_edge: *exit_edge,
                    },
                    K::Break => K::Break,
                    K::Continue => K::Continue,
                    K::Exit { block } => K::Exit { block: *block },
                    K::PartialExit { block } => K::PartialExit { block: *block },
                    K::Indirect {
                        block,
                        arms,
                        unresolved,
                    } => K::Indirect {
                        block: *block,
                        arms: arms
                            .iter()
                            .zip(children)
                            .map(|(arm, body)| IndirectArm {
                                target_pc: arm.target_pc,
                                body,
                            })
                            .collect(),
                        unresolved: *unresolved,
                    },
                    K::Dispatcher { entry, cases } => {
                        let mut cases = cases.clone();
                        for case in &mut cases {
                            for transition in &mut case.transitions {
                                transition.id = owner.id();
                                parents.insert(transition.id, vec![transition.edge]);
                            }
                        }
                        K::Dispatcher {
                            entry: *entry,
                            cases,
                        }
                    }
                };
                let entity = owner.id();
                parents.insert(entity, node.sources.clone());
                built.push(StructuredNode { id: entity, kind });
            }
        }
    }
    if built.len() != 1 {
        return Err(PlanError::Invalid(EntityId(0)));
    }
    Ok(Materialized {
        root: built.pop().unwrap(),
        parents,
    })
}

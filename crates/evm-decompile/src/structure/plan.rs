//! Shared immutable region plans, independent of owned AST occurrences.
use crate::{EntityId, StructuredKind};
mod build;
pub(super) use build::build;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct PlanId(pub usize);

pub(super) struct PlanNode {
    pub kind: StructuredKind<PlanId>,
    pub sources: Vec<EntityId>,
}

pub(super) struct Region {
    pub entry: EntityId,
    pub continuation: Option<EntityId>,
    pub loop_header: Option<EntityId>,
    pub body: PlanId,
}

pub(super) struct Plan {
    pub nodes: Vec<PlanNode>,
    pub root: PlanId,
    pub regions: Vec<Region>,
}
/// Reference slots consume retained ancestry even when syntax itself is small.
/// Keep this admission separate from the wire's count of syntax occurrences.
pub(super) const MAX_SOURCE_REFERENCES: usize = crate::MAX_STRUCTURED_WIRE_NODES;

impl Plan {
    pub fn has_dispatch(&self, pay: &mut impl FnMut(usize) -> bool) -> Result<bool, PlanError> {
        if !pay(self.nodes.len().saturating_mul(2)) {
            return Err(PlanError::Limit);
        }
        let mut seen = vec![false; self.nodes.len()];
        let mut todo = vec![self.root];
        while let Some(id) = todo.pop() {
            if seen[id.0] {
                continue;
            }
            seen[id.0] = true;
            match &self.nodes[id.0].kind {
                StructuredKind::Dispatcher { .. } => return Ok(true),
                StructuredKind::Sequence(v) => todo.extend(v),
                StructuredKind::If {
                    then_branch,
                    else_branch,
                    ..
                } => todo.extend([**then_branch, **else_branch]),
                StructuredKind::While { header, body, .. } => todo.extend([**header, **body]),
                StructuredKind::Indirect { arms, .. } => todo.extend(arms.iter().map(|a| a.body)),
                _ => {}
            }
        }
        Ok(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum PlanError {
    #[error("structure plan resource admission refused")]
    Limit,
    #[error("invalid structure plan witness {0:?}")]
    Invalid(EntityId),
}

/// Expanded retained occurrences, not number of arena entries. Nested Sequence
/// containers are flattened without ever allocating discarded AST identities.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Quote {
    pub nodes: usize,
    pub depth: usize,
    pub references: usize,
    pub transitions: usize,
    pub visits: usize,
}
impl Quote {
    pub fn units(self) -> usize {
        self.nodes
            .saturating_add(self.references)
            .saturating_add(self.transitions)
    }
}

pub(super) fn quote_node(node: &PlanNode, nodes: &[PlanNode], quotes: &[Quote]) -> Quote {
    let mut result = Quote {
        nodes: 1,
        depth: 1,
        references: node.sources.len(),
        transitions: 0,
        visits: 1,
    };
    let mut child = |id: PlanId, flatten: bool| {
        let q = quotes[id.0];
        result.nodes = result
            .nodes
            .saturating_add(q.nodes.saturating_sub(usize::from(flatten)));
        result.references = result
            .references
            .saturating_add(q.references.saturating_sub(if flatten {
                nodes[id.0].sources.len()
            } else {
                0
            }));
        result.transitions = result.transitions.saturating_add(q.transitions);
        if !(flatten && q.nodes == 1 && q.references == 0 && q.transitions == 0) {
            result.visits = result.visits.saturating_add(q.visits);
        }
        result.depth = result
            .depth
            .max(q.depth.saturating_add(usize::from(!flatten)));
    };
    match &node.kind {
        StructuredKind::Sequence(v) => {
            for id in v {
                child(*id, matches!(nodes[id.0].kind, StructuredKind::Sequence(_)));
            }
        }
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => {
            child(**then_branch, false);
            child(**else_branch, false);
        }
        StructuredKind::While { header, body, .. } => {
            child(**header, false);
            child(**body, false);
        }
        StructuredKind::Indirect { arms, .. } => {
            for arm in arms {
                child(arm.body, false);
            }
        }
        StructuredKind::Dispatcher { cases, .. } => {
            result.references = result.references.saturating_add(cases.len());
            result.transitions = cases
                .iter()
                .fold(0_usize, |n, c| n.saturating_add(c.transitions.len()));
        }
        _ => {}
    }
    result
}

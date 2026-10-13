//! Bounded immutable formulas for guarded joins. Common scalar facts remain in
//! RelationState; branch formulas preserve the alternatives seen by SMT.

use super::{
    Constraint, ExprError, ExprId, ExprLimits, QueryReason, RelationLimits, RelationState,
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// Read-only relation formula, selected by EVM nonzero=true guards.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub enum RelationPathKind {
    /// Unconstrained path.
    True,
    /// Proved impossible path.
    False,
    /// Conjunction of relation atoms.
    Atoms(Vec<Constraint>),
    /// Conjunction of shared subformulas.
    And(Vec<RelationPath>),
    /// One selector pairs the complete alternative formulas.
    Ite {
        /// Shared nonzero branch selector.
        selector: ExprId,
        /// Complete formula for the nonzero alternative.
        then_path: RelationPath,
        /// Complete formula for the zero alternative.
        else_path: RelationPath,
    },
}
#[derive(Debug, PartialEq, Eq, Serialize)]
struct Node {
    kind: RelationPathKind,
    nodes: usize,
    depth: usize,
    atoms: usize,
}
/// Immutable path formula with cached bounded retention work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelationPath(Arc<Node>);
impl Default for RelationPath {
    fn default() -> Self {
        Self::leaf(RelationPathKind::True)
    }
}
impl RelationPath {
    fn leaf(kind: RelationPathKind) -> Self {
        Self(Arc::new(Node {
            kind,
            nodes: 1,
            depth: 1,
            atoms: 0,
        }))
    }
    fn sized(kind: RelationPathKind) -> Self {
        let (nodes, depth, atoms) = match &kind {
            RelationPathKind::True | RelationPathKind::False => (1, 1, 0),
            RelationPathKind::Atoms(atoms) => (
                atoms
                    .iter()
                    .flat_map(Constraint::expressions)
                    .fold(1usize.saturating_add(atoms.len()), |n, e| {
                        n.saturating_add(e.nodes())
                    }),
                atoms
                    .iter()
                    .flat_map(Constraint::expressions)
                    .map(ExprId::depth)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1),
                atoms.len(),
            ),
            RelationPathKind::And(children) => (
                children
                    .iter()
                    .fold(1usize, |n, c| n.saturating_add(c.nodes())),
                children
                    .iter()
                    .map(Self::depth)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1),
                children
                    .iter()
                    .fold(0usize, |n, c| n.saturating_add(c.atoms())),
            ),
            RelationPathKind::Ite {
                selector,
                then_path,
                else_path,
            } => (
                selector
                    .nodes()
                    .saturating_add(then_path.nodes())
                    .saturating_add(else_path.nodes())
                    .saturating_add(1),
                selector
                    .depth()
                    .max(then_path.depth())
                    .max(else_path.depth())
                    .saturating_add(1),
                then_path.atoms().saturating_add(else_path.atoms()),
            ),
        };
        Self(Arc::new(Node {
            kind,
            nodes,
            depth,
            atoms,
        }))
    }
    pub(super) fn from_state(state: &RelationState) -> Self {
        if state.bottom {
            return Self::leaf(RelationPathKind::False);
        }
        if state.constraints.is_empty() {
            return state.path.clone();
        }
        let nodes = state
            .constraints
            .iter()
            .flat_map(Constraint::expressions)
            .fold(1usize, |n, e| n.saturating_add(e.nodes()));
        let depth = state
            .constraints
            .iter()
            .flat_map(Constraint::expressions)
            .map(ExprId::depth)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let atoms = state.constraints.len();
        let facts = Self(Arc::new(Node {
            kind: RelationPathKind::Atoms(state.constraints.iter().cloned().collect()),
            nodes,
            depth,
            atoms,
        }));
        if matches!(state.path.kind(), RelationPathKind::True) {
            facts
        } else {
            Self::and(state.path.clone(), facts)
        }
    }
    fn and(left: Self, right: Self) -> Self {
        if left == right {
            return left;
        }
        Self(Arc::new(Node {
            nodes: left.nodes().saturating_add(right.nodes()).saturating_add(1),
            depth: left.depth().max(right.depth()).saturating_add(1),
            atoms: left.atoms().saturating_add(right.atoms()),
            kind: RelationPathKind::And(vec![left, right]),
        }))
    }
    pub(super) fn guarded(
        left: Self,
        right: Self,
        selector: ExprId,
        limits: &RelationLimits,
    ) -> Result<Self, QueryReason> {
        if left == right {
            return Ok(left);
        }
        let nodes = left
            .nodes()
            .saturating_add(right.nodes())
            .saturating_add(selector.nodes())
            .saturating_add(1);
        let depth = left
            .depth()
            .max(right.depth())
            .max(selector.depth())
            .saturating_add(1);
        let atoms = left.atoms().saturating_add(right.atoms());
        if embedded_smt::current_cancellation().is_cancelled() {
            return Err(QueryReason::Cancelled);
        }
        if nodes > limits.max_nodes || depth > limits.max_depth {
            return Err(QueryReason::ExpressionLimit);
        }
        if atoms > limits.max_constraints {
            return Err(QueryReason::ConstraintLimit);
        }
        Ok(Self(Arc::new(Node {
            kind: RelationPathKind::Ite {
                selector,
                then_path: left,
                else_path: right,
            },
            nodes,
            depth,
            atoms,
        })))
    }
    /// Read-only formula shape for solver/report consumers.
    pub fn kind(&self) -> &RelationPathKind {
        &self.0.kind
    }
    /// Conservative formula and scalar work bound.
    pub fn nodes(&self) -> usize {
        self.0.nodes
    }
    /// Maximum formula/scalar dependency depth.
    pub fn depth(&self) -> usize {
        self.0.depth
    }
    /// Number of retained branch atoms, including alternatives.
    pub fn atoms(&self) -> usize {
        self.0.atoms
    }
    /// Process-private shared formula id, remapped by each report.
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }
    /// Full scalar/state expressions referenced by this formula.
    pub fn expressions(&self) -> Vec<&ExprId> {
        let mut stack = vec![self];
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        while let Some(path) = stack.pop() {
            if !seen.insert(Arc::as_ptr(&path.0)) {
                continue;
            }
            match path.kind() {
                RelationPathKind::Atoms(atoms) => {
                    out.extend(atoms.iter().flat_map(Constraint::expressions))
                }
                RelationPathKind::And(children) => stack.extend(children),
                RelationPathKind::Ite {
                    selector,
                    then_path,
                    else_path,
                } => {
                    out.push(selector);
                    stack.push(then_path);
                    stack.push(else_path);
                }
                _ => {}
            }
        }
        out
    }
    pub(super) fn rename(
        &self,
        map: &BTreeMap<ExprId, ExprId>,
        limits: ExprLimits,
    ) -> Result<Self, ExprError> {
        let mut memo = BTreeMap::<usize, Self>::new();
        let mut stack = vec![(self, false)];
        let mut work = 0usize;
        while let Some((path, ready)) = stack.pop() {
            let key = Arc::as_ptr(&path.0) as usize;
            if memo.contains_key(&key) {
                continue;
            }
            if !ready {
                work = work.saturating_add(1);
                crate::domain::symbolic::state_check(path.nodes().max(work), path.depth(), limits)?;
                stack.push((path, true));
                match path.kind() {
                    RelationPathKind::And(children) => {
                        stack.extend(children.iter().rev().map(|c| (c, false)))
                    }
                    RelationPathKind::Ite {
                        then_path,
                        else_path,
                        ..
                    } => {
                        stack.push((else_path, false));
                        stack.push((then_path, false));
                    }
                    _ => {}
                }
                continue;
            }
            let get = |p: &Self| {
                memo.get(&(Arc::as_ptr(&p.0) as usize))
                    .expect("postorder relation")
                    .clone()
            };
            let kind = match path.kind() {
                RelationPathKind::True => RelationPathKind::True,
                RelationPathKind::False => RelationPathKind::False,
                RelationPathKind::Atoms(atoms) => RelationPathKind::Atoms(
                    atoms
                        .iter()
                        .map(|a| a.rename(map, limits))
                        .collect::<Result<Vec<_>, _>>()?,
                ),
                RelationPathKind::And(children) => {
                    RelationPathKind::And(children.iter().map(get).collect())
                }
                RelationPathKind::Ite {
                    selector,
                    then_path,
                    else_path,
                } => RelationPathKind::Ite {
                    selector: selector.rename_fresh(map, limits)?,
                    then_path: get(then_path),
                    else_path: get(else_path),
                },
            };
            let renamed = Self::sized(kind);
            crate::domain::symbolic::state_check(renamed.nodes(), renamed.depth(), limits)?;
            memo.insert(key, renamed);
        }
        Ok(memo
            .remove(&(Arc::as_ptr(&self.0) as usize))
            .expect("renamed root"))
    }
}

impl Serialize for RelationPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        enum ExportNode<'a> {
            True,
            False,
            Atoms(&'a [Constraint]),
            And(Vec<usize>),
            Ite {
                selector: &'a ExprId,
                then_path: usize,
                else_path: usize,
            },
        }
        #[derive(Serialize)]
        struct Export<'a> {
            root: usize,
            nodes: Vec<ExportNode<'a>>,
        }
        let policy = crate::domain::symbolic::GraphLimits::default();
        crate::domain::symbolic::state_check(
            self.nodes(),
            self.depth(),
            ExprLimits {
                max_nodes: policy.max_nodes,
                max_depth: policy.max_depth,
            },
        )
        .map_err(serde::ser::Error::custom)?;
        let mut memo = BTreeMap::new();
        let mut stack = vec![(self, false)];
        let mut nodes = Vec::new();
        while let Some((path, ready)) = stack.pop() {
            if memo.contains_key(&path.id()) {
                continue;
            }
            if embedded_smt::current_cancellation().is_cancelled() {
                return Err(serde::ser::Error::custom(ExprError::Cancelled));
            }
            if !ready {
                stack.push((path, true));
                match path.kind() {
                    RelationPathKind::And(children) => {
                        stack.extend(children.iter().rev().map(|p| (p, false)))
                    }
                    RelationPathKind::Ite {
                        then_path,
                        else_path,
                        ..
                    } => {
                        stack.push((else_path, false));
                        stack.push((then_path, false));
                    }
                    _ => {}
                }
                continue;
            }
            let index = |p: &Self| *memo.get(&p.id()).expect("postorder path export");
            let node = match path.kind() {
                RelationPathKind::True => ExportNode::True,
                RelationPathKind::False => ExportNode::False,
                RelationPathKind::Atoms(atoms) => ExportNode::Atoms(atoms),
                RelationPathKind::And(children) => {
                    ExportNode::And(children.iter().map(index).collect())
                }
                RelationPathKind::Ite {
                    selector,
                    then_path,
                    else_path,
                } => ExportNode::Ite {
                    selector,
                    then_path: index(then_path),
                    else_path: index(else_path),
                },
            };
            memo.insert(path.id(), nodes.len());
            nodes.push(node);
        }
        Export {
            root: *memo.get(&self.id()).expect("exported path"),
            nodes,
        }
        .serialize(serializer)
    }
}

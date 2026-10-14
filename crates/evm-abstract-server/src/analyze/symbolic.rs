//! Report-local interning of immutable state and path graphs. Native keys stay
//! alive during projection; only typed local references cross the wire.
mod nodes;
mod paths;

use super::{checkpoint, value};
use evm_abstract::{
    analysis::control::Control,
    domain::{
        AbstractValue,
        relational::{Constraint, RelationPath, RelationPathKind, RelationState},
        symbolic::{
            ByteStateId, ByteStateKind, ByteView, ExprId, ExprKind, SlotStateId, SlotStateKind,
        },
    },
};
use evm_abstract_protocol as api;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Expression(ExprId),
    Bytes(ByteStateId),
    Slots(SlotStateId),
    View(ByteView),
    Path(usize),
}
#[derive(Clone)]
enum Root {
    Expression(ExprId),
    Bytes(ByteStateId),
    Slots(SlotStateId),
    View(ByteView),
    Path(RelationPath),
}
impl Root {
    fn key(&self) -> Key {
        match self {
            Self::Expression(v) => Key::Expression(v.clone()),
            Self::Bytes(v) => Key::Bytes(v.clone()),
            Self::Slots(v) => Key::Slots(v.clone()),
            Self::View(v) => Key::View(v.clone()),
            Self::Path(v) => Key::Path(v.id()),
        }
    }
    fn depth(&self) -> usize {
        match self {
            Self::Expression(v) => v.depth(),
            Self::Bytes(v) => v.depth(),
            Self::Slots(v) => v.depth(),
            Self::View(v) => v.depth(),
            Self::Path(v) => v.depth(),
        }
    }
    fn children(&self) -> Vec<Self> {
        let e = |v: &ExprId| Self::Expression(v.clone());
        match self {
            Self::Expression(v) => match v.kind() {
                ExprKind::Operation { args, .. } => args.iter().map(e).collect(),
                ExprKind::Ite {
                    condition,
                    then_value,
                    else_value,
                } => vec![e(condition), e(then_value), e(else_value)],
                ExprKind::ByteRead { state, index } => vec![Self::Bytes(state.clone()), e(index)],
                ExprKind::WordRead { view, offset } => vec![Self::View(view.clone()), e(offset)],
                ExprKind::SlotRead { state, key } => vec![Self::Slots(state.clone()), e(key)],
                ExprKind::Keccak { input } => vec![Self::View(input.clone())],
                ExprKind::Constant(_) | ExprKind::Input(_) | ExprKind::Fresh(_) => vec![],
            },
            Self::Bytes(v) => match v.kind() {
                ByteStateKind::Input {
                    source,
                    length,
                    bindings,
                } => std::iter::once(e(source.token()))
                    .chain(std::iter::once(e(length)))
                    .chain(bindings.iter().map(|b| e(&b.value)))
                    .collect(),
                ByteStateKind::WriteByte {
                    previous,
                    offset,
                    value,
                }
                | ByteStateKind::WriteWord {
                    previous,
                    offset,
                    value,
                } => vec![Self::Bytes(previous.clone()), e(offset), e(value)],
                ByteStateKind::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => vec![
                    Self::Bytes(previous.clone()),
                    e(destination),
                    Self::View(source.clone()),
                    e(length),
                ],
                ByteStateKind::Slice { source } => vec![Self::View(source.clone())],
                ByteStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => vec![
                    e(condition),
                    Self::Bytes(then_state.clone()),
                    Self::Bytes(else_state.clone()),
                ],
                ByteStateKind::Zero | ByteStateKind::Exact(_) => vec![],
            },
            Self::Slots(v) => match v.kind() {
                SlotStateKind::Input { source, bindings } => std::iter::once(e(source.token()))
                    .chain(bindings.iter().map(|b| e(&b.value)))
                    .collect(),
                SlotStateKind::Store {
                    previous,
                    key,
                    value,
                } => vec![Self::Slots(previous.clone()), e(key), e(value)],
                SlotStateKind::Ite {
                    condition,
                    then_state,
                    else_state,
                } => vec![
                    e(condition),
                    Self::Slots(then_state.clone()),
                    Self::Slots(else_state.clone()),
                ],
                SlotStateKind::Zero => vec![],
            },
            Self::View(v) => vec![Self::Bytes(v.state.clone()), e(&v.offset), e(&v.length)],
            Self::Path(v) => match v.kind() {
                RelationPathKind::True | RelationPathKind::False => vec![],
                RelationPathKind::Atoms(atoms) => {
                    paths::expressions(atoms).into_iter().map(e).collect()
                }
                RelationPathKind::And(paths) => paths.iter().cloned().map(Self::Path).collect(),
                RelationPathKind::Ite {
                    selector,
                    then_path,
                    else_path,
                } => vec![
                    e(selector),
                    Self::Path(then_path.clone()),
                    Self::Path(else_path.clone()),
                ],
            },
        }
    }
}

pub(super) struct Pool<'a> {
    control: &'a Control,
    memo: BTreeMap<Key, usize>,
    // Paths use Arc pointer identities; keeping them alive prevents pointer reuse.
    paths: Vec<RelationPath>,
    pub graph: api::SymbolicGraph,
    pub limits: api::ExportLimits,
    pub usage: api::ExportUsage,
    fresh: BTreeMap<u64, u64>,
    relations: BTreeMap<(usize, Vec<Constraint>, bool), api::PathRef>,
}
impl<'a> Pool<'a> {
    pub fn new(control: &'a Control) -> Self {
        Self {
            control,
            memo: BTreeMap::new(),
            paths: Vec::new(),
            graph: api::SymbolicGraph { nodes: vec![] },
            limits: api::ExportLimits {
                max_nodes: 65_536,
                max_depth: 256,
                max_exact_bytes: 16 * 1024 * 1024,
                max_output_bytes: evm_abstract::render::json::JsonLimits::default().max_bytes,
            },
            usage: api::ExportUsage {
                nodes: 0,
                exact_bytes: 0,
                work: 0,
            },
            fresh: BTreeMap::new(),
            relations: BTreeMap::new(),
        }
    }
    pub fn payload(&mut self, bytes: usize) -> Result<(), api::ApiError> {
        let required = self.usage.exact_bytes.saturating_add(bytes);
        if required > self.limits.max_exact_bytes {
            return Err(self.failure(api::ExportFailureKind::ExactBytes, required as u64));
        }
        self.charge(bytes)?;
        self.usage.exact_bytes = required;
        Ok(())
    }
    pub fn failure(&self, kind: api::ExportFailureKind, required: u64) -> api::ApiError {
        api::ApiError {
            code: if kind == api::ExportFailureKind::Cancelled {
                api::ApiErrorCode::Cancelled
            } else {
                api::ApiErrorCode::Export
            },
            message: format!("report export {kind:?} boundary (required {required})"),
            details: api::ErrorDetails::Export(Box::new(api::ExportFailure {
                kind,
                required,
                limits: self.limits,
                usage: self.usage,
                written: None,
            })),
        }
    }
    pub fn charge(&mut self, work: usize) -> Result<(), api::ApiError> {
        if checkpoint(self.control).is_err() {
            return Err(self.failure(api::ExportFailureKind::Cancelled, self.usage.work));
        }
        // Work is an informational total. Even after saturation every charge
        // remains a cancellation checkpoint.
        self.usage.work = self.usage.work.saturating_add(work as u64);
        Ok(())
    }
    fn index(&self, root: Root) -> usize {
        self.memo[&root.key()]
    }
    fn e(&self, v: &ExprId) -> api::ExpressionRef {
        api::ExpressionRef {
            node: self.index(Root::Expression(v.clone())),
        }
    }
    fn b(&self, v: &ByteStateId) -> api::ByteStateRef {
        api::ByteStateRef {
            node: self.index(Root::Bytes(v.clone())),
        }
    }
    fn s(&self, v: &SlotStateId) -> api::SlotStateRef {
        api::SlotStateRef {
            node: self.index(Root::Slots(v.clone())),
        }
    }
    fn v(&self, v: &ByteView) -> api::ByteViewRef {
        api::ByteViewRef {
            node: self.index(Root::View(v.clone())),
        }
    }
    fn p(&self, v: &RelationPath) -> api::PathRef {
        api::PathRef {
            node: self.index(Root::Path(v.clone())),
        }
    }
    fn intern(&mut self, root: Root) -> Result<usize, api::ApiError> {
        self.charge(1)?;
        if root.depth() > self.limits.max_depth {
            return Err(self.failure(api::ExportFailureKind::Depth, root.depth() as u64));
        }
        if let Some(index) = self.memo.get(&root.key()) {
            return Ok(*index);
        }
        let mut pending = vec![(root.clone(), false)];
        while let Some((item, ready)) = pending.pop() {
            self.charge(1)?;
            let key = item.key();
            if self.memo.contains_key(&key) {
                continue;
            }
            if !ready {
                let children = item.children();
                self.charge(children.len())?;
                pending.push((item, true));
                pending.extend(children.into_iter().rev().map(|child| (child, false)));
                continue;
            }
            let required = self.graph.nodes.len().saturating_add(1);
            if required > self.limits.max_nodes {
                return Err(self.failure(api::ExportFailureKind::Nodes, required as u64));
            }
            let node = self.node(&item)?;
            if let Root::Path(path) = item {
                self.paths.push(path);
            }
            let index = self.graph.nodes.len();
            self.graph.nodes.push(node);
            self.memo.insert(key, index);
            self.usage.nodes = self.graph.nodes.len();
        }
        Ok(self.index(root))
    }
    pub fn expression(&mut self, source: &ExprId) -> Result<api::ExpressionRef, api::ApiError> {
        Ok(api::ExpressionRef {
            node: self.intern(Root::Expression(source.clone()))?,
        })
    }
    pub fn slots(&mut self, source: &SlotStateId) -> Result<api::SlotStateRef, api::ApiError> {
        Ok(api::SlotStateRef {
            node: self.intern(Root::Slots(source.clone()))?,
        })
    }
    pub fn view(&mut self, source: &ByteView) -> Result<api::ByteViewRef, api::ApiError> {
        Ok(api::ByteViewRef {
            node: self.intern(Root::View(source.clone()))?,
        })
    }
    pub fn path(&mut self, source: &RelationPath) -> Result<api::PathRef, api::ApiError> {
        Ok(api::PathRef {
            node: self.intern(Root::Path(source.clone()))?,
        })
    }
    pub fn relations(&mut self, source: &RelationState) -> Result<api::PathRef, api::ApiError> {
        self.charge(source.len().saturating_add(1))?;
        let atoms: Vec<_> = source.constraints().cloned().collect();
        let key = (source.path().id(), atoms.clone(), source.is_bottom());
        if let Some(root) = self.relations.get(&key) {
            return Ok(*root);
        }
        let base = self.path(source.path())?;
        let root = if source.is_bottom() {
            self.append_path(api::Expression::PathFalse)?
        } else if atoms.is_empty() {
            base
        } else {
            let depth = source.path().depth().saturating_add(1);
            if depth > self.limits.max_depth {
                return Err(self.failure(api::ExportFailureKind::Depth, depth as u64));
            }
            for expr in paths::expressions(&atoms) {
                self.expression(expr)?;
            }
            let constraints = atoms
                .iter()
                .map(|atom| self.constraint(atom))
                .collect::<Result<_, _>>()?;
            let facts = self.append_path(api::Expression::PathAtoms(constraints))?;
            self.append_path(api::Expression::PathAnd(vec![base, facts]))?
        };
        self.relations.insert(key, root);
        Ok(root)
    }
    fn append_path(&mut self, node: api::Expression) -> Result<api::PathRef, api::ApiError> {
        self.charge(1)?;
        let required = self.graph.nodes.len().saturating_add(1);
        if required > self.limits.max_nodes {
            return Err(self.failure(api::ExportFailureKind::Nodes, required as u64));
        }
        let index = self.graph.nodes.len();
        self.graph.nodes.push(node);
        self.usage.nodes = self.graph.nodes.len();
        Ok(api::PathRef { node: index })
    }
    pub fn value(&mut self, source: &AbstractValue) -> Result<api::ValueInfo, api::ApiError> {
        self.charge(1 + source.constants().map_or(0, |v| v.len()))?;
        let mut info = value::info(source);
        info.expression = source
            .expression()
            .map(|expr| self.expression(expr))
            .transpose()?;
        Ok(info)
    }
}

//! Report-local interning of immutable state and path graphs. Native keys stay
//! alive during projection; only typed local references cross the wire.
mod control;
mod minimum;
mod nodes;
mod paths;

use super::{checkpoint, value};
use evm_abstract::{
    analysis::control::Control,
    domain::{
        EvmValue, WordElement,
        relational::{Constraint, RelationPath, RelationPathKind, RelationState},
        symbolic::{
            ByteStateId, ByteView, ExprId, SlotStateId,
            structure::{Handle, Postorder},
        },
    },
};
use evm_abstract_protocol as api;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Native(Handle),
    Path(usize),
}
#[derive(Clone)]
enum Root {
    Native(Handle),
    Path(RelationPath),
}
impl Root {
    fn key(&self) -> Key {
        match self {
            Self::Native(v) => Key::Native(v.clone()),
            Self::Path(v) => Key::Path(v.id()),
        }
    }
    fn depth(&self) -> usize {
        match self {
            Self::Native(v) => v.depth(),
            Self::Path(v) => v.depth(),
        }
    }
    fn visit_children(&self, mut action: impl FnMut(Self)) {
        match self {
            Self::Native(v) => v.visit_dependencies(|child| action(Self::Native(child))),
            Self::Path(v) => match v.kind() {
                RelationPathKind::True | RelationPathKind::False => {}
                RelationPathKind::Atoms(atoms) => {
                    for e in paths::expressions(atoms) {
                        action(Self::Native(Handle::Word(e.clone())));
                    }
                }
                RelationPathKind::And(paths) => {
                    for path in paths {
                        action(Self::Path(path.clone()));
                    }
                }
                RelationPathKind::Ite {
                    selector,
                    then_path,
                    else_path,
                } => {
                    action(Self::Native(Handle::Word(selector.clone())));
                    action(Self::Path(then_path.clone()));
                    action(Self::Path(else_path.clone()));
                }
            },
        }
    }
}

pub(super) struct Pool<'a> {
    control: &'a Control,
    // Minimum projection borrows the job's existing application allowance.
    // Rich export retains its established enclosing-operation accounting.
    minimum_budget: Option<&'a mut evm_abstract::resource::WorkBudget>,
    control_budget: Option<&'a mut evm_abstract::resource::WorkBudget>,
    control_directory: control::Directory,
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
            minimum_budget: None,
            control_budget: None,
            control_directory: control::Directory::default(),
            memo: BTreeMap::new(),
            paths: Vec::new(),
            graph: api::SymbolicGraph { nodes: vec![] },
            limits: api::ExportLimits {
                max_nodes: 65_536,
                max_depth: 256,
                max_exact_bytes: 16 * 1024 * 1024,
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
    pub fn minimum(
        control: &'a Control,
        budget: &'a mut evm_abstract::resource::WorkBudget,
    ) -> Self {
        Self {
            minimum_budget: Some(budget),
            ..Self::new(control)
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
        if let Some(budget) = self.minimum_budget.as_mut() {
            let refusal = if work == usize::MAX {
                Some(api::ExportFailureKind::Work)
            } else if !budget.charge(work) {
                Some(if budget.charge(0) {
                    api::ExportFailureKind::Work
                } else {
                    api::ExportFailureKind::Cancelled
                })
            } else {
                None
            };
            if let Some(kind) = refusal {
                return Err(self.failure(kind, work as u64));
            }
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
            node: self.index(Root::Native(Handle::Word(v.clone()))),
        }
    }
    fn b(&self, v: &ByteStateId) -> api::ByteStateRef {
        api::ByteStateRef {
            node: self.index(Root::Native(Handle::Bytes(v.clone()))),
        }
    }
    fn s(&self, v: &SlotStateId) -> api::SlotStateRef {
        api::SlotStateRef {
            node: self.index(Root::Native(Handle::Slots(v.clone()))),
        }
    }
    fn v(&self, v: &ByteView) -> api::ByteViewRef {
        api::ByteViewRef {
            node: self.index(Root::Native(Handle::View(v.clone()))),
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
        let mut pending = Postorder::new(root.clone());
        while let Some((item, ready)) = pending.next() {
            self.charge(1)?;
            let key = item.key();
            if self.memo.contains_key(&key) {
                continue;
            }
            if !ready {
                let children = pending.expand_with(item.clone(), |pending| {
                    item.visit_children(|child| pending.push(child));
                });
                self.charge(children)?;
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
            node: self.intern(Root::Native(Handle::Word(source.clone())))?,
        })
    }
    pub fn slots(&mut self, source: &SlotStateId) -> Result<api::SlotStateRef, api::ApiError> {
        Ok(api::SlotStateRef {
            node: self.intern(Root::Native(Handle::Slots(source.clone())))?,
        })
    }
    pub fn view(&mut self, source: &ByteView) -> Result<api::ByteViewRef, api::ApiError> {
        Ok(api::ByteViewRef {
            node: self.intern(Root::Native(Handle::View(source.clone())))?,
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
    pub fn value<N: WordElement>(
        &mut self,
        source: &EvmValue<N>,
    ) -> Result<api::ValueInfo, api::ApiError> {
        if self.minimum_budget.is_some() {
            return self.numeric_value(source);
        }
        self.charge(source.numeric().work_size().saturating_add(1))?;
        let mut info = value::info(source);
        info.expression = source
            .expression()
            .map(|expr| self.expression(expr))
            .transpose()?;
        Ok(info)
    }

    fn numeric_value<N: WordElement>(
        &mut self,
        source: &EvmValue<N>,
    ) -> Result<api::ValueInfo, api::ApiError> {
        let required = self.usage.nodes.saturating_add(1);
        if required > self.limits.max_nodes {
            return Err(self.failure(api::ExportFailureKind::Nodes, required as u64));
        }
        // O(1) capability: never silently discard a custom scalar predicate.
        if source.numeric().requires_retained_binding() {
            return Err(self.failure(api::ExportFailureKind::Numeric, required as u64));
        }
        self.charge(source.numeric().work_size().saturating_add(1))?;
        let observation = source.numeric().observation();
        self.charge(0)?;
        // Fund formatting/copying the neutral vocabulary after the paid native
        // observation and before materializing any strings or candidate list.
        self.charge(observation.work_size().saturating_mul(256))?;
        let mut info = value::observation(&observation);
        info.origins = source
            .provenance()
            .origins()
            .sources()
            .map(|sources| sources.iter().copied().map(value::origin).collect());
        info.code_address_role = source.provenance().is_code_address();
        info.identity = source
            .identity()
            .input()
            .map(|identity| api::InputIdentity {
                scope: api::DecimalU64(identity.scope()),
                symbol: value::symbol(identity.symbol()),
            });
        info.symbolic_limit = source.symbolic_limit_reached();
        self.usage.nodes = required;
        self.charge(0)?;
        Ok(info)
    }
}

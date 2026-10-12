//! Complete scalar/state graph traversal and capture-avoiding replay renaming.

use super::{
    ByteStateId, ByteStateKind, ByteView, ExprError, ExprId, ExprKind, ExprLimits, SlotStateId,
    SlotStateKind, StateBinding,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
enum Item {
    Expr(ExprId),
    Bytes(ByteStateId),
    Slots(SlotStateId),
    View(ByteView),
}
fn children(item: &Item) -> Vec<Item> {
    let expr = |e: &ExprId| Item::Expr(e.clone());
    match item {
        Item::Expr(e) => match e.kind() {
            ExprKind::Operation { args, .. } => args.iter().map(expr).collect(),
            ExprKind::Ite {
                condition,
                then_value,
                else_value,
            } => vec![expr(condition), expr(then_value), expr(else_value)],
            ExprKind::ByteRead { state, index } => vec![Item::Bytes(state.clone()), expr(index)],
            ExprKind::WordRead { view, offset } => vec![Item::View(view.clone()), expr(offset)],
            ExprKind::SlotRead { state, key } => vec![Item::Slots(state.clone()), expr(key)],
            ExprKind::Keccak { input } => vec![Item::View(input.clone())],
            _ => Vec::new(),
        },
        Item::Bytes(s) => match s.kind() {
            ByteStateKind::Zero | ByteStateKind::Exact(_) => Vec::new(),
            ByteStateKind::Input {
                source,
                length,
                bindings,
            } => std::iter::once(expr(source.token()))
                .chain(std::iter::once(expr(length)))
                .chain(bindings.iter().map(|b| expr(&b.value)))
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
            } => vec![Item::Bytes(previous.clone()), expr(offset), expr(value)],
            ByteStateKind::Copy {
                previous,
                destination,
                source,
                length,
            } => vec![
                Item::Bytes(previous.clone()),
                expr(destination),
                Item::View(source.clone()),
                expr(length),
            ],
            ByteStateKind::Slice { source } => vec![Item::View(source.clone())],
            ByteStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => vec![
                expr(condition),
                Item::Bytes(then_state.clone()),
                Item::Bytes(else_state.clone()),
            ],
        },
        Item::Slots(s) => match s.kind() {
            SlotStateKind::Zero => Vec::new(),
            SlotStateKind::Input { source, bindings } => std::iter::once(expr(source.token()))
                .chain(bindings.iter().map(|b| expr(&b.value)))
                .collect(),
            SlotStateKind::Store {
                previous,
                key,
                value,
            } => vec![Item::Slots(previous.clone()), expr(key), expr(value)],
            SlotStateKind::Ite {
                condition,
                then_state,
                else_state,
            } => vec![
                expr(condition),
                Item::Slots(then_state.clone()),
                Item::Slots(else_state.clone()),
            ],
        },
        Item::View(v) => vec![
            Item::Bytes(v.state.clone()),
            expr(&v.offset),
            expr(&v.length),
        ],
    }
}
fn walk(root: Item, action: &mut impl FnMut(&ExprId)) {
    let mut stack = vec![root];
    let mut expressions = BTreeSet::new();
    let mut states = BTreeSet::new();
    while let Some(item) = stack.pop() {
        let seen = match &item {
            Item::Expr(e) => {
                if !expressions.insert(e.clone()) {
                    true
                } else {
                    action(e);
                    false
                }
            }
            Item::Bytes(s) => !states.insert(s.id()),
            Item::Slots(s) => !states.insert(s.id()),
            Item::View(_) => false,
        };
        if !seen {
            stack.extend(children(&item));
        }
    }
}
pub(super) fn visit(root: &ExprId, action: &mut impl FnMut(&ExprId)) {
    walk(Item::Expr(root.clone()), action);
}
fn fresh(root: Item) -> BTreeSet<ExprId> {
    let mut out = BTreeSet::new();
    walk(root, &mut |e| {
        if matches!(e.kind(), ExprKind::Fresh(_)) {
            out.insert(e.clone());
        }
    });
    out
}
pub(super) fn state_fresh(root: &ByteStateId) -> BTreeSet<ExprId> {
    fresh(Item::Bytes(root.clone()))
}
pub(super) fn slots_fresh(root: &SlotStateId) -> BTreeSet<ExprId> {
    fresh(Item::Slots(root.clone()))
}
pub(super) fn view_fresh(root: &ByteView) -> BTreeSet<ExprId> {
    fresh(Item::View(root.clone()))
}

/// One memo table spans state/word/source operands during a graph rewrite.
pub(super) struct Renamer<'a> {
    replacements: &'a BTreeMap<ExprId, ExprId>,
    limits: ExprLimits,
    expressions: BTreeMap<ExprId, ExprId>,
    bytes: BTreeMap<ByteStateId, ByteStateId>,
    slots: BTreeMap<SlotStateId, SlotStateId>,
    work: usize,
}
impl<'a> Renamer<'a> {
    pub fn new(replacements: &'a BTreeMap<ExprId, ExprId>, limits: ExprLimits) -> Self {
        Self {
            replacements,
            limits,
            expressions: BTreeMap::new(),
            bytes: BTreeMap::new(),
            slots: BTreeMap::new(),
            work: 0,
        }
    }
    fn check(&mut self, nodes: usize, depth: usize) -> Result<(), ExprError> {
        self.work = self.work.saturating_add(1);
        super::state::check(nodes.max(self.work), depth, self.limits.into()).map_err(Into::into)
    }
    fn known(&self, item: &Item) -> bool {
        match item {
            Item::Expr(e) => self.expressions.contains_key(e),
            Item::Bytes(s) => self.bytes.contains_key(s),
            Item::Slots(s) => self.slots.contains_key(s),
            Item::View(v) => {
                self.bytes.contains_key(&v.state)
                    && self.expressions.contains_key(&v.offset)
                    && self.expressions.contains_key(&v.length)
            }
        }
    }
    /// Iterative postorder avoids call-stack recursion even with large caller limits.
    fn prepare(&mut self, root: Item) -> Result<(), ExprError> {
        let mut stack = vec![(root, false)];
        while let Some((item, ready)) = stack.pop() {
            if self.known(&item) {
                continue;
            }
            if !ready {
                let (nodes, depth) = match &item {
                    Item::Expr(e) => (e.nodes(), e.depth()),
                    Item::Bytes(s) => (s.nodes(), s.depth()),
                    Item::Slots(s) => (s.nodes(), s.depth()),
                    Item::View(v) => (v.nodes(), v.depth()),
                };
                self.check(nodes, depth)?;
                let deps = children(&item);
                stack.push((item, true));
                stack.extend(deps.into_iter().rev().map(|dep| (dep, false)));
                continue;
            }
            match item {
                Item::Expr(e) => {
                    let renamed = match e.kind() {
                        ExprKind::Constant(_) | ExprKind::Input(_) => e.clone(),
                        ExprKind::Fresh(_) => self
                            .replacements
                            .get(&e)
                            .cloned()
                            .unwrap_or_else(|| e.clone()),
                        ExprKind::Operation { opcode, args } => ExprId::operation(
                            *opcode,
                            &args.iter().map(|a| self.e(a)).collect::<Vec<_>>(),
                            self.limits,
                        )?,
                        ExprKind::Ite {
                            condition,
                            then_value,
                            else_value,
                        } => ExprId::ite(
                            self.e(condition),
                            self.e(then_value),
                            self.e(else_value),
                            self.limits,
                        )?,
                        ExprKind::ByteRead { state, index } => {
                            ExprId::byte_read(self.b(state), self.e(index), self.limits)?
                        }
                        ExprKind::WordRead { view, offset } => {
                            ExprId::word_read(self.v(view), self.e(offset), self.limits)?
                        }
                        ExprKind::SlotRead { state, key } => {
                            ExprId::slot_read(self.s(state), self.e(key), self.limits)?
                        }
                        ExprKind::Keccak { input } => ExprId::keccak(self.v(input), self.limits)?,
                    };
                    super::state::check(renamed.nodes(), renamed.depth(), self.limits.into())?;
                    self.expressions.insert(e, renamed);
                }
                Item::Bytes(s) => {
                    let limits = self.limits.into();
                    let renamed = match s.kind() {
                        ByteStateKind::Zero => ByteStateId::zero(),
                        ByteStateKind::Exact(bytes) => ByteStateId::exact(Arc::clone(bytes)),
                        ByteStateKind::Input {
                            source,
                            length,
                            bindings,
                        } => ByteStateId::observed(
                            source.renamed(self.e(source.token())),
                            self.e(length),
                            self.bindings(bindings),
                            limits,
                        )?,
                        ByteStateKind::WriteByte {
                            previous,
                            offset,
                            value,
                        } => self
                            .b(previous)
                            .write_byte(self.e(offset), self.e(value), limits)?,
                        ByteStateKind::WriteWord {
                            previous,
                            offset,
                            value,
                        } => self
                            .b(previous)
                            .write_word(self.e(offset), self.e(value), limits)?,
                        ByteStateKind::Copy {
                            previous,
                            destination,
                            source,
                            length,
                        } => self.b(previous).copy(
                            self.e(destination),
                            self.v(source),
                            self.e(length),
                            limits,
                        )?,
                        ByteStateKind::Slice { source } => {
                            let source = self.v(source);
                            source.state.slice(source.offset, source.length, limits)?
                        }
                        ByteStateKind::Ite {
                            condition,
                            then_state,
                            else_state,
                        } => self.b(then_state).join_guarded(
                            &self.b(else_state),
                            self.e(condition),
                            limits,
                        )?,
                    };
                    self.bytes.insert(s, renamed);
                }
                Item::Slots(s) => {
                    let limits = self.limits.into();
                    let renamed = match s.kind() {
                        SlotStateKind::Zero => SlotStateId::zero(),
                        SlotStateKind::Input { source, bindings } => SlotStateId::observed(
                            source.renamed(self.e(source.token())),
                            self.bindings(bindings),
                            limits,
                        )?,
                        SlotStateKind::Store {
                            previous,
                            key,
                            value,
                        } => self.s(previous).store(self.e(key), self.e(value), limits)?,
                        SlotStateKind::Ite {
                            condition,
                            then_state,
                            else_state,
                        } => self.s(then_state).join_guarded(
                            &self.s(else_state),
                            self.e(condition),
                            limits,
                        )?,
                    };
                    self.slots.insert(s, renamed);
                }
                Item::View(_) => {}
            }
        }
        Ok(())
    }
    fn e(&self, e: &ExprId) -> ExprId {
        self.expressions
            .get(e)
            .expect("postorder expression")
            .clone()
    }
    fn b(&self, b: &ByteStateId) -> ByteStateId {
        self.bytes.get(b).expect("postorder byte state").clone()
    }
    fn s(&self, s: &SlotStateId) -> SlotStateId {
        self.slots.get(s).expect("postorder slot state").clone()
    }
    fn v(&self, v: &ByteView) -> ByteView {
        ByteView::new(self.b(&v.state), self.e(&v.offset), self.e(&v.length)).canonicalize()
    }
    fn bindings(&self, bindings: &[StateBinding]) -> Vec<StateBinding> {
        bindings
            .iter()
            .map(|b| StateBinding {
                index: b.index,
                value: self.e(&b.value),
                numeric: b.numeric.clone(),
            })
            .collect()
    }
    pub fn expression(&mut self, e: &ExprId) -> Result<ExprId, ExprError> {
        self.prepare(Item::Expr(e.clone()))?;
        Ok(self.e(e))
    }
    pub fn bytes(&mut self, b: &ByteStateId) -> Result<ByteStateId, ExprError> {
        self.prepare(Item::Bytes(b.clone()))?;
        Ok(self.b(b))
    }
    pub fn slots(&mut self, s: &SlotStateId) -> Result<SlotStateId, ExprError> {
        self.prepare(Item::Slots(s.clone()))?;
        Ok(self.s(s))
    }
    pub fn view(&mut self, v: &ByteView) -> Result<ByteView, ExprError> {
        self.prepare(Item::View(v.clone()))?;
        Ok(self.v(v))
    }
}
use std::sync::Arc;

//! Complete scalar/state graph traversal and capture-avoiding replay renaming.

use super::{
    ByteStateId, ByteView, ExprError, ExprId, ExprKind, ExprLimits, InputAtom, SlotStateId,
    SourceId, StateBinding,
    structure::{Borrowed, Handle as Item, Mapper, Native, Node, Postorder},
};
use alloy_primitives::U256;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

fn walk(root: Item, action: &mut impl FnMut(&ExprId)) {
    let mut stack = vec![root];
    let mut expressions = BTreeSet::new();
    let mut states = BTreeSet::new();
    while let Some(item) = stack.pop() {
        let seen = match &item {
            Item::Word(e) => {
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
            stack.extend(item.dependencies());
        }
    }
}
pub(super) fn visit(root: &ExprId, action: &mut impl FnMut(&ExprId)) {
    walk(Item::Word(root.clone()), action);
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
            Item::Word(e) => self.expressions.contains_key(e),
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
        let mut stack = Postorder::new(root);
        while let Some((item, ready)) = stack.next() {
            if self.known(&item) {
                continue;
            }
            if !ready {
                let (nodes, depth) = (item.nodes(), item.depth());
                self.check(nodes, depth)?;
                let deps = item.dependencies();
                stack.expand(item, deps);
                continue;
            }
            let mapped = item.structure().try_map(self)?;
            // Structural mapping is shared; interpretation remains here so
            // folding, view canonicalization and state limits stay authoritative.
            let renamed = match mapped {
                Node::Constant(_) | Node::Input(_) => item.clone(),
                Node::Fresh(_) => {
                    let Item::Word(e) = &item else {
                        unreachable!("word leaf")
                    };
                    Item::Word(
                        self.replacements
                            .get(e)
                            .cloned()
                            .unwrap_or_else(|| e.clone()),
                    )
                }
                Node::Operation { opcode, args } => {
                    Item::Word(ExprId::operation(opcode, &args, self.limits)?)
                }
                Node::Ite {
                    condition,
                    then_value,
                    else_value,
                } => Item::Word(ExprId::ite(condition, then_value, else_value, self.limits)?),
                Node::ByteRead { state, index } => {
                    Item::Word(ExprId::byte_read(state, index, self.limits)?)
                }
                Node::WordRead { view, offset } => {
                    Item::Word(ExprId::word_read(view, offset, self.limits)?)
                }
                Node::SlotRead { state, key } => {
                    Item::Word(ExprId::slot_read(state, key, self.limits)?)
                }
                Node::Keccak { input } => Item::Word(ExprId::keccak(input, self.limits)?),
                Node::BytesZero => Item::Bytes(ByteStateId::zero()),
                Node::BytesExact(bytes) => Item::Bytes(ByteStateId::exact(bytes)),
                Node::BytesInput {
                    source,
                    length,
                    bindings,
                } => Item::Bytes(ByteStateId::observed(
                    source,
                    length,
                    bindings,
                    self.limits.into(),
                )?),
                Node::WriteByte {
                    previous,
                    offset,
                    value,
                } => Item::Bytes(previous.write_byte(offset, value, self.limits.into())?),
                Node::WriteWord {
                    previous,
                    offset,
                    value,
                } => Item::Bytes(previous.write_word(offset, value, self.limits.into())?),
                Node::Copy {
                    previous,
                    destination,
                    source,
                    length,
                } => Item::Bytes(previous.copy(destination, source, length, self.limits.into())?),
                Node::Slice { source } => Item::Bytes(source.state.slice(
                    source.offset,
                    source.length,
                    self.limits.into(),
                )?),
                Node::BytesIte {
                    condition,
                    then_state,
                    else_state,
                } => Item::Bytes(then_state.join_guarded(
                    &else_state,
                    condition,
                    self.limits.into(),
                )?),
                Node::SlotsZero => Item::Slots(SlotStateId::zero()),
                Node::SlotsInput { source, bindings } => {
                    Item::Slots(SlotStateId::observed(source, bindings, self.limits.into())?)
                }
                Node::Store {
                    previous,
                    key,
                    value,
                } => Item::Slots(previous.store(key, value, self.limits.into())?),
                Node::SlotsIte {
                    condition,
                    then_state,
                    else_state,
                } => Item::Slots(then_state.join_guarded(
                    &else_state,
                    condition,
                    self.limits.into(),
                )?),
                // Views are reconstructed on demand from the three memo tables,
                // as before; they do not acquire a separate retention/work cost.
                Node::View { .. } => continue,
            };
            match (item, renamed) {
                (Item::Word(old), Item::Word(new)) => {
                    super::state::check(new.nodes(), new.depth(), self.limits.into())?;
                    self.expressions.insert(old, new);
                }
                (Item::Bytes(old), Item::Bytes(new)) => {
                    self.bytes.insert(old, new);
                }
                (Item::Slots(old), Item::Slots(new)) => {
                    self.slots.insert(old, new);
                }
                _ => unreachable!("structural mapping preserves result sorts"),
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
    pub fn expression(&mut self, e: &ExprId) -> Result<ExprId, ExprError> {
        self.prepare(Item::Word(e.clone()))?;
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

impl<'a> Mapper<Borrowed<'a>> for Renamer<'_> {
    type Output = Native;
    type Error = ExprError;
    fn word(&mut self, value: &&ExprId) -> Result<ExprId, ExprError> {
        Ok(self.e(value))
    }
    fn bytes(&mut self, value: &&ByteStateId) -> Result<ByteStateId, ExprError> {
        Ok(self.b(value))
    }
    fn slots(&mut self, value: &&SlotStateId) -> Result<SlotStateId, ExprError> {
        Ok(self.s(value))
    }
    fn view(&mut self, value: &&ByteView) -> Result<ByteView, ExprError> {
        Ok(self.v(value))
    }
    fn constant(&mut self, value: &&U256) -> Result<U256, ExprError> {
        Ok(**value)
    }
    fn input(&mut self, value: &&InputAtom) -> Result<InputAtom, ExprError> {
        Ok(**value)
    }
    fn fresh(&mut self, value: u64) -> Result<u64, ExprError> {
        Ok(value)
    }
    fn exact(&mut self, value: &&Arc<[u8]>) -> Result<Arc<[u8]>, ExprError> {
        Ok(Arc::clone(value))
    }
    fn source(&mut self, value: &&SourceId) -> Result<SourceId, ExprError> {
        Ok(value.renamed(self.e(value.token())))
    }
    fn binding(&mut self, value: &&StateBinding) -> Result<StateBinding, ExprError> {
        Ok(StateBinding {
            index: value.index,
            value: self.e(&value.value),
            numeric: value.numeric.clone(),
        })
    }
}

//! Demand-driven source equations, with exact load identity and conservative joins.
//! Only source constants and total word operators establish numeric facts. Open
//! predecessor sets are refused. Cyclic cell equations use either the existing
//! constant-anchor fast path or independently replayed bounded recurrences.
//! A separately admitted write extent proves untouched zero suffixes. Unknown
//! allocation arithmetic never becomes a pointer convention.
use crate::{EntityId, FactScope, Ir, Operation, U256, Value, ValueKind};
use std::collections::{BTreeMap, BTreeSet};

mod cycles;
mod edge;
mod invariant;
mod relations;
#[cfg(test)]
mod tests;
mod word;
const MAX_DEPTH: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Identity {
    Value(EntityId),
    Content(EntityId, U256),
    Affine(usize),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Word {
    pub base: Option<Identity>,
    pub offset: U256,
    pub low: U256,
    pub high: U256,
}
impl Word {
    pub fn constant(v: U256) -> Self {
        Self {
            base: None,
            offset: v,
            low: v,
            high: v,
        }
    }
    fn opaque(id: EntityId) -> Self {
        Self {
            base: Some(Identity::Value(id)),
            offset: U256::ZERO,
            low: U256::ZERO,
            high: U256::MAX,
        }
    }
    pub fn exact(self) -> Option<U256> {
        self.base.is_none().then_some(self.offset)
    }
    pub fn same(self, other: Self) -> bool {
        self.base == other.base && self.offset == other.offset
    }
    pub fn relative(self, other: Self) -> Option<usize> {
        if self.base != other.base {
            return None;
        }
        usize::try_from(self.offset.wrapping_sub(other.offset)).ok()
    }
    pub fn end(self, size: usize) -> Option<U256> {
        self.high.checked_add(U256::from(size))
    }
}

pub(super) struct Memory<'a, P> {
    pub ir: &'a Ir,
    pub operations: BTreeMap<EntityId, &'a Operation>,
    operation_blocks: BTreeMap<EntityId, EntityId>,
    cyclic_blocks: BTreeSet<EntityId>,
    values: BTreeMap<EntityId, &'a Value>,
    blocks: BTreeMap<EntityId, &'a crate::Block>,
    incoming: BTreeMap<EntityId, Vec<&'a crate::Edge>>,
    words: BTreeMap<EntityId, Word>,
    certified: BTreeMap<EntityId, Word>,
    certified_extents: BTreeMap<EntityId, Word>,
    return_data_size_bound: Option<U256>,
    loads: BTreeMap<(EntityId, U256), Option<Word>>,
    active_words: BTreeSet<EntityId>,
    active_loads: BTreeSet<(EntityId, U256)>,
    cyclic_loads: BTreeSet<(EntityId, U256)>,
    pub pay: P,
    pub refused: bool,
    recursion: usize,
}
impl<'a, P: FnMut(usize) -> bool> Memory<'a, P> {
    pub fn new(ir: &'a Ir, mut pay: P) -> Option<Self> {
        if !pay(ir
            .values
            .len()
            .saturating_add(ir.edges.len())
            .saturating_add(ir.blocks.iter().map(|b| b.operations.len()).sum::<usize>())
            .saturating_mul(4)
            .saturating_add(1))
        {
            return None;
        }
        let return_data_size_bound = execution_bound(ir)?;
        let mut incoming = BTreeMap::<_, Vec<_>>::new();
        for edge in &ir.edges {
            incoming.entry(edge.to).or_default().push(edge);
        }
        let cyclic_blocks = cycles::blocks(ir, &mut pay)?;
        let certified = relations::analyze(ir, &mut pay)?;
        Some(Self {
            ir,
            cyclic_blocks,
            certified: certified.words,
            certified_extents: certified.extents,
            return_data_size_bound,
            blocks: ir.blocks.iter().map(|b| (b.id, b)).collect(),
            operation_blocks: ir
                .blocks
                .iter()
                .flat_map(|b| b.operations.iter().map(move |o| (o.id, b.id)))
                .collect(),
            operations: ir
                .blocks
                .iter()
                .flat_map(|b| &b.operations)
                .map(|o| (o.id, o))
                .collect(),
            values: ir.values.iter().map(|v| (v.id, v)).collect(),
            incoming,
            words: BTreeMap::new(),
            loads: BTreeMap::new(),
            active_words: BTreeSet::new(),
            active_loads: BTreeSet::new(),
            cyclic_loads: BTreeSet::new(),
            pay,
            refused: false,
            recursion: 0,
        })
    }
    pub fn charge(&mut self, work: usize) -> bool {
        if !(self.pay)(work) {
            self.refused = true;
        }
        !self.refused
    }
    pub fn word(&mut self, id: EntityId) -> Option<Word> {
        self.eval(id, 0)
    }
    /// Every byte at or above a freshly replayed write extent is still zero.
    /// Return the first proved zero byte of this nonwrapping query range.
    pub fn zero_suffix(&mut self, memory: EntityId, base: Word, size: usize) -> Option<usize> {
        if !self.charge(1) {
            return None;
        }
        base.end(size)?;
        let extent = *self.certified_extents.get(&memory)?;
        if extent.high <= base.low {
            return Some(0);
        }
        if base
            .relative(extent)
            .is_some_and(|delta| extent.high.checked_add(U256::from(delta)).is_some())
        {
            return Some(0);
        }
        let start = extent.relative(base)?;
        (start <= size && base.high.checked_add(U256::from(start)).is_some()).then_some(start)
    }
    fn load(&mut self, memory: EntityId, address: U256, depth: usize) -> Option<Word> {
        if !self.charge(1) || depth > MAX_DEPTH || self.recursion >= MAX_DEPTH {
            return None;
        }
        let key = (memory, address);
        if let Some(result) = self.loads.get(&key) {
            return *result;
        }
        if !self.active_loads.insert(key) {
            self.cyclic_loads.insert(key);
            return None;
        }
        self.recursion += 1;
        let mut result = self.load_equation(memory, address, depth);
        self.active_loads.remove(&key);
        if result.is_none() && self.cyclic_loads.remove(&key) {
            result = self.invariant_load(memory, address, depth);
        }
        self.loads.insert(key, result);
        self.recursion -= 1;
        result
    }
    fn load_equation(&mut self, memory: EntityId, address: U256, depth: usize) -> Option<Word> {
        match self.values.get(&memory)?.kind.clone() {
            ValueKind::Memory { operation: None } => Some(Word::constant(U256::ZERO)),
            ValueKind::Memory {
                operation: Some(id),
            } => {
                let op = *self.operations.get(&id)?;
                if op.memory_output != Some(memory) {
                    return None;
                }
                if op.opcode == 0x52 && op.inputs.len() == 2 {
                    let offset = self.eval(op.inputs[0], depth + 1)?;
                    if offset.exact() == Some(address) {
                        return self.eval(op.inputs[1], depth + 1);
                    }
                }
                if !self.writer_disjoint(op, Word::constant(address), 32) {
                    return None;
                }
                self.load(op.memory_input?, address, depth + 1)
            }
            ValueKind::MemoryParameter { block } => {
                if self.blocks.get(&block)?.memory_parameter != memory {
                    return None;
                }
                if self.ir.fact_scope != FactScope::CompleteLocalCfg {
                    return None;
                }
                let edges = self.incoming.get(&block)?.clone();
                if Some(block) == self.ir.entry || edges.is_empty() || !self.charge(edges.len()) {
                    return None;
                }
                let mut result: Option<Word> = None;
                for edge in edges {
                    let word = self.load(edge.memory, address, depth + 1)?;
                    let word = self.on_edge(word, edge, depth + 1);
                    result = Some(match result {
                        Some(r) => Word {
                            low: r.low.min(word.low),
                            high: r.high.max(word.high),
                            ..if r.same(word) {
                                r
                            } else {
                                Word {
                                    base: Some(Identity::Content(memory, address)),
                                    offset: U256::ZERO,
                                    low: U256::ZERO,
                                    high: U256::MAX,
                                }
                            }
                        },
                        None => word,
                    });
                }
                result
            }
            _ => None,
        }
    }
    pub fn predecessor_memory(&self, memory: EntityId) -> Option<EntityId> {
        match self.values.get(&memory)?.kind {
            ValueKind::Memory {
                operation: Some(op),
            } => {
                let op = self.operations.get(&op)?;
                (op.memory_output == Some(memory))
                    .then_some(op.memory_input)
                    .flatten()
            }
            ValueKind::MemoryParameter { block }
                if self.ir.fact_scope == FactScope::CompleteLocalCfg
                    && Some(block) != self.ir.entry =>
            {
                if self.blocks.get(&block)?.memory_parameter != memory {
                    return None;
                }
                let incoming = self.incoming.get(&block)?;
                let first = incoming.first()?.memory;
                incoming.iter().all(|e| e.memory == first).then_some(first)
            }
            _ => None,
        }
    }
    pub fn writer(&self, memory: EntityId) -> Option<&'a Operation> {
        let ValueKind::Memory {
            operation: Some(id),
        } = self.values.get(&memory)?.kind
        else {
            return None;
        };
        let op = *self.operations.get(&id)?;
        (op.memory_output == Some(memory)).then_some(op)
    }
    /// A copy/output range has an ordinary, nonwrapping address domain even
    /// when its width is symbolic. A proved lower bound beyond the queried
    /// cell therefore suffices; demanding an exact width would lose slot
    /// contents through unrelated ABI/returndata buffer construction.
    pub fn writer_disjoint(&mut self, op: &Operation, target: Word, target_size: usize) -> bool {
        let offset_index = match op.opcode {
            0x52 | 0x53 | 0x37 | 0x39 | 0x3e | 0x5e => 0,
            0x3c => 1,
            0xf1 | 0xf2 => 5,
            0xf4 | 0xfa => 4,
            _ => return false,
        };
        let size_index = match op.opcode {
            0x37 | 0x39 | 0x3e | 0x5e => Some(2),
            0x3c => Some(3),
            0xf1 | 0xf2 => Some(6),
            0xf4 | 0xfa => Some(5),
            _ => None,
        };
        if size_index
            .and_then(|i| op.inputs.get(i))
            .and_then(|id| self.word(*id))
            .and_then(Word::exact)
            == Some(U256::ZERO)
        {
            return true;
        }
        let Some(offset) = op.inputs.get(offset_index).and_then(|id| self.word(*id)) else {
            return false;
        };
        if target.end(target_size).is_some_and(|end| end <= offset.low) {
            return true;
        }
        self.write_range(op)
            .is_some_and(|(offset, size)| self.disjoint(offset, size, target, target_size))
    }
    pub fn write_range(&mut self, op: &Operation) -> Option<(Word, usize)> {
        let (offset, size) = match op.opcode {
            0x52 => (*op.inputs.first()?, 32),
            0x53 => (*op.inputs.first()?, 1),
            0x37 | 0x39 | 0x3e | 0x5e if op.inputs.len() == 3 => (
                op.inputs[0],
                usize::try_from(self.word(op.inputs[2])?.exact()?).ok()?,
            ),
            0x3c if op.inputs.len() == 4 => (
                op.inputs[1],
                usize::try_from(self.word(op.inputs[3])?.exact()?).ok()?,
            ),
            0xf1 | 0xf2 if op.inputs.len() == 7 => (
                op.inputs[5],
                usize::try_from(self.word(op.inputs[6])?.exact()?).ok()?,
            ),
            0xf4 | 0xfa if op.inputs.len() == 6 => (
                op.inputs[4],
                usize::try_from(self.word(op.inputs[5])?.exact()?).ok()?,
            ),
            _ => return None,
        };
        if size == 0 {
            return Some((Word::constant(U256::ZERO), 0));
        }
        Some((self.word(offset)?, size))
    }
    pub fn disjoint(&self, a: Word, an: usize, b: Word, bn: usize) -> bool {
        if an == 0 || bn == 0 {
            return true;
        }
        if a.end(an).is_some_and(|end| end <= b.low) || b.end(bn).is_some_and(|end| end <= a.low) {
            return true;
        }
        // Exact modular identity separates two ordinary, nonwrapping ranges
        // only when neither circular distance lies inside the other range.
        a.base == b.base && a.end(an).is_some() && b.end(bn).is_some() && {
            let delta = b.offset.wrapping_sub(a.offset);
            U256::from(an) <= delta && delta <= U256::ZERO.wrapping_sub(U256::from(bn))
        }
    }
}

/// A semantic bound is usable only with its complete declared execution domain.
/// Generic source has no fork-dependent size axiom. Producer and certificate
/// replay both construct this same memory kernel from the immutable IR witness.
fn execution_bound(ir: &Ir) -> Option<Option<U256>> {
    if ir.execution_domain.is_generic() {
        ir.execution_fork.is_none().then_some(None)
    } else {
        let fork = ir.execution_fork?;
        ir.execution_domain.return_data_size_bound(fork).map(Some)
    }
}

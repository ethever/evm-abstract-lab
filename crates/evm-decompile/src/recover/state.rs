//! Private executable facts, independent of monotone wire summaries.
//!
//! Addresses contain only exact words or trusted expressions rooted in immutable
//! inputs. Unknown aliases invalidate observations; equal numeric approximations
//! and equal source labels never establish address or value equality.

use crate::{EntityId, U256};
use analysis_kernel::cost;
use evm_domain::{EvmValue, NumericValue, relational::RelationState, symbolic::ExprId};
use std::collections::{BTreeMap, BTreeSet};

pub(super) type AbstractValue = EvmValue<NumericValue>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Cell {
    pub value: EntityId,
    pub abstract_value: AbstractValue,
    // Only literal JUMPDEST addresses, independent of the numeric lattice.
    pub control: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Address {
    Exact(U256),
    Expression(ExprId),
}
impl Address {
    /// The caller funds the singleton/expression query and key traversal.
    pub fn of(value: &AbstractValue) -> Option<Self> {
        value
            .singleton()
            .map(Self::Exact)
            .or_else(|| value.expression().cloned().map(Self::Expression))
    }
    fn offset(&self) -> Option<u64> {
        match self {
            Self::Exact(value) => u64::try_from(*value).ok(),
            Self::Expression(_) => None,
        }
    }
    fn work_size(&self) -> usize {
        match self {
            Self::Exact(_) => 1,
            Self::Expression(expression) => expression.work_size(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Memory {
    // Default bytes remain zero outside all possibly written concrete ranges.
    zero: bool,
    written: Vec<(u64, u64)>,
    pub words: BTreeMap<Address, AbstractValue>,
}
impl Memory {
    pub fn new() -> Self {
        Self {
            zero: true,
            written: Vec::new(),
            words: BTreeMap::new(),
        }
    }
    pub fn clear(&mut self) {
        self.zero = false;
        self.written.clear();
        self.words.clear();
    }
    fn untouched(&self, address: &Address) -> bool {
        self.zero
            && address.offset().is_some_and(|offset| {
                offset
                    .checked_add(32)
                    .is_some_and(|end| self.written.iter().all(|(a, b)| end <= *a || *b <= offset))
            })
    }
    pub fn read(&self, address: Option<&Address>) -> AbstractValue {
        // Every successful read of a pristine zero backing is zero, including
        // symbolic or unrepresentable addresses. Zero-length writes preserve it.
        if self.zero && self.written.is_empty() {
            return AbstractValue::constant(U256::ZERO);
        }
        let Some(address) = address else {
            return AbstractValue::top();
        };
        self.words.get(address).cloned().unwrap_or_else(|| {
            if self.untouched(address) {
                AbstractValue::constant(U256::ZERO)
            } else {
                AbstractValue::top()
            }
        })
    }
    fn mark_written(&mut self, start: u64, end: u64) {
        if !self.zero {
            return;
        }
        let mut ranges = Vec::with_capacity(self.written.len().saturating_add(1));
        let (mut start, mut end) = (start, end);
        let mut inserted = false;
        for (old_start, old_end) in self.written.drain(..) {
            if old_end < start {
                ranges.push((old_start, old_end));
            } else if end < old_start {
                if !inserted {
                    ranges.push((start, end));
                    inserted = true;
                }
                ranges.push((old_start, old_end));
            } else {
                start = start.min(old_start);
                end = end.max(old_end);
            }
        }
        if !inserted {
            ranges.push((start, end));
        }
        self.written = ranges;
    }
    pub fn write(&mut self, address: Option<Address>, width: u64, value: Option<AbstractValue>) {
        if width == 0 {
            return;
        }
        let Some(address) = address else {
            self.clear();
            return;
        };
        let range = address
            .offset()
            .and_then(|offset| offset.checked_add(width).map(|end| (offset, end)));
        self.words.retain(|key, _| {
            matches!((key.offset(), range), (Some(offset), Some((start, end)))
                if offset.checked_add(32).is_some_and(|old_end| old_end <= start || end <= offset))
        });
        if let Some((start, end)) = range {
            self.mark_written(start, end);
        } else {
            // A symbolic write may touch any concrete byte. Its exact word can
            // still be forwarded when the same trusted address is loaded.
            self.zero = false;
            self.written.clear();
        }
        if width == 32
            && let Some(value) = value
            && (!matches!(address, Address::Exact(_)) || range.is_some())
        {
            self.words.insert(address, value);
        }
    }
    /// Keep observations true on both paths, joining alternatives at each key.
    /// The caller funds structural work, at most two zero-literal factories
    /// per union key, and each selected value join.
    pub fn merge(
        &mut self,
        other: &Self,
        mut join: impl FnMut(&AbstractValue, &AbstractValue) -> Option<AbstractValue>,
    ) -> Option<()> {
        let keys: BTreeSet<_> = self
            .words
            .keys()
            .chain(other.words.keys())
            .cloned()
            .collect();
        let mut words = BTreeMap::new();
        for key in keys {
            let left = self.words.get(&key).cloned().or_else(|| {
                self.untouched(&key)
                    .then(|| AbstractValue::constant(U256::ZERO))
            });
            let right = other.words.get(&key).cloned().or_else(|| {
                other
                    .untouched(&key)
                    .then(|| AbstractValue::constant(U256::ZERO))
            });
            if let (Some(left), Some(right)) = (left, right) {
                words.insert(key, join(&left, &right)?);
            }
        }
        self.words = words;
        self.zero &= other.zero;
        if self.zero {
            for (start, end) in &other.written {
                self.mark_written(*start, *end);
            }
        } else {
            self.written.clear();
        }
        Some(())
    }
    pub fn merge_work(&self, other: &Self) -> usize {
        let keys = self.words.len().saturating_add(other.words.len());
        let ranges = self.written.len().saturating_add(other.written.len());
        map_work(&self.words)
            .saturating_add(map_work(&other.words))
            .saturating_mul(cost::tree_work(keys).saturating_add(1))
            .saturating_mul(16)
            .saturating_add(keys.saturating_mul(ranges).saturating_mul(8))
            // Repeated ordered interval insertion is bounded quadratically.
            .saturating_add(ranges.saturating_mul(ranges).saturating_mul(8))
    }
    pub fn work_size(&self) -> usize {
        map_work(&self.words).saturating_add(self.written.len().saturating_mul(4))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Storage {
    pub slots: BTreeMap<Address, AbstractValue>,
}
impl Storage {
    pub fn read(&self, address: Option<&Address>) -> AbstractValue {
        address
            .and_then(|address| self.slots.get(address))
            .cloned()
            .unwrap_or_else(AbstractValue::top)
    }
    pub fn write(&mut self, address: Option<Address>, value: AbstractValue) {
        let Some(address) = address else {
            self.slots.clear();
            return;
        };
        self.slots.retain(
            |old, _| matches!((old, &address), (Address::Exact(a), Address::Exact(b)) if a != b),
        );
        self.slots.insert(address, value);
    }
    pub fn merge(
        &mut self,
        other: &Self,
        mut join: impl FnMut(&AbstractValue, &AbstractValue) -> Option<AbstractValue>,
    ) -> Option<()> {
        let mut slots = BTreeMap::new();
        for (key, left) in &self.slots {
            if let Some(right) = other.slots.get(key) {
                slots.insert(key.clone(), join(left, right)?);
            }
        }
        self.slots = slots;
        Some(())
    }
    pub fn merge_work(&self, other: &Self) -> usize {
        map_work(&self.slots)
            .saturating_add(map_work(&other.slots))
            .saturating_mul(
                cost::tree_work(self.slots.len().saturating_add(other.slots.len()))
                    .saturating_add(1),
            )
            .saturating_mul(16)
    }
    pub fn work_size(&self) -> usize {
        map_work(&self.slots)
    }
}
fn map_work(map: &BTreeMap<Address, AbstractValue>) -> usize {
    map.iter().fold(1usize, |work, (address, value)| {
        work.saturating_add(address.work_size())
            .saturating_add(value.clone_work())
            .saturating_add(value.work_size())
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct State {
    pub stack: Vec<Cell>,
    pub memory: Memory,
    pub storage: Storage,
    pub transient: Storage,
    pub relations: RelationState,
}
impl State {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            memory: Memory::new(),
            // An arbitrary runtime invocation can be reentrant, including into
            // an existing transient plane. Neither initial plane is all-zero.
            storage: Storage::default(),
            transient: Storage::default(),
            relations: RelationState::default(),
        }
    }
    pub fn work_size(&self) -> usize {
        self.stack
            .iter()
            .fold(1usize, |work, cell| {
                work.saturating_add(cell.abstract_value.clone_work())
                    .saturating_add(cell.abstract_value.work_size())
            })
            .saturating_add(self.memory.work_size())
            .saturating_add(self.storage.work_size())
            .saturating_add(self.transient.work_size())
            .saturating_add(self.relations.work_size().saturating_mul(4))
    }
    pub fn merge_work(&self, other: &Self) -> usize {
        self.work_size()
            .saturating_add(other.work_size())
            .saturating_mul(8)
            .saturating_add(self.memory.merge_work(&other.memory))
            .saturating_add(self.storage.merge_work(&other.storage))
            .saturating_add(self.transient.merge_work(&other.transient))
            .saturating_add(
                self.relations
                    .work_size()
                    .saturating_add(other.relations.work_size())
                    .saturating_mul(
                        cost::tree_work(self.relations.len().saturating_add(other.relations.len()))
                            .saturating_add(1),
                    )
                    .saturating_mul(8),
            )
    }
    pub fn instruction_work(&self) -> usize {
        let entries = self
            .memory
            .words
            .len()
            .saturating_add(self.storage.slots.len())
            .saturating_add(self.transient.slots.len());
        self.work_size()
            .saturating_mul(cost::tree_work(entries).saturating_add(1))
            .saturating_mul(8)
    }
    /// Runtime identities describe one block execution, never a stored entry.
    pub fn forget_runtime(&mut self) {
        for value in self
            .stack
            .iter_mut()
            .map(|cell| &mut cell.abstract_value)
            .chain(self.memory.words.values_mut())
            .chain(self.storage.slots.values_mut())
            .chain(self.transient.slots.values_mut())
        {
            value.forget_identity();
        }
    }
}

//! Price policy identity independently of the executable payload it partitions.
//!
//! Finite cardinality does not bound a key's heap traversal. All opaque quotes,
//! comparisons and copies are surrounded by both cancellation scopes; indexes
//! cache retained costs so reserving a later full copy needs no key traversal.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, btree_map},
};

use crate::{
    analysis::machine::{FrameKey, MachineError, MachineKey},
    domain::relational::QueryReason,
    resource::WorkBudget,
};

use super::{Arrival, PartitionPolicy};

// Rust 1.99.0 (rust-toolchain.toml): alloc/collections/btree/node.rs uses B=6,
// capacity=11 and non-root minimum=5. Keep these bounds aligned with the pinned
// toolchain; tests exercise actual comparisons and insertion across tree splits.
const BTREE_BRANCH: usize = 6;
const BTREE_CAPACITY: usize = 2 * BTREE_BRANCH - 1;

/// An ordered policy identity with an independent logical work quotation.
///
/// `key_work` must read cached or structural metadata, without cloning,
/// comparing, discovering semantic facts or doing the work being quoted. Its
/// result bounds a complete clone and disposal; comparing two keys costs at
/// most the saturating sum of their quotations. Include inline fields and all
/// retained heap contents. Quotes must remain valid while an index retains a
/// key, and must be preserved by cloning. As with algebra work quotations, this
/// is a trusted implementation contract. `usize::MAX` refuses the operation.
pub trait PolicyKey: Clone + Ord {
    /// Conservative work for cloning and disposing of this complete key.
    fn key_work(&self) -> usize;
}

/// Optional cached quotation for an existing ordered key representation.
///
/// The supplied work must satisfy [`PolicyKey`] for the wrapped value and its
/// cached metadata. The quotation is not part of equality or ordering, so two
/// wrappers with the same value remain the same policy identity.
#[derive(Clone, Debug)]
pub struct QuotedKey<K> {
    value: K,
    work: usize,
}

impl<K> QuotedKey<K> {
    /// Attach a trusted conservative quotation without inspecting the value.
    pub const fn new(value: K, work: usize) -> Self {
        Self { value, work }
    }

    /// Borrow the underlying identity without cloning it.
    pub const fn value(&self) -> &K {
        &self.value
    }
}

impl<K: PartialEq> PartialEq for QuotedKey<K> {
    fn eq(&self, other: &Self) -> bool {
        self.value.eq(&other.value)
    }
}

impl<K: Eq> Eq for QuotedKey<K> {}

impl<K: Ord> PartialOrd for QuotedKey<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord> Ord for QuotedKey<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.value.cmp(&other.value)
    }
}

impl<K: Clone + Ord> PolicyKey for QuotedKey<K> {
    fn key_work(&self) -> usize {
        self.work
    }
}

// Count inline machine words twice for a full copy and disposal. Fixed fields
// in FrameKey (including addresses, code and symbolic owners) are all inline.
fn inline_work<T>() -> usize {
    std::mem::size_of::<T>()
        .div_ceil(std::mem::size_of::<usize>())
        .max(1)
        .saturating_mul(2)
}

macro_rules! scalar_keys {
    ($($key:ty),+ $(,)?) => {
        $(impl PolicyKey for $key {
            fn key_work(&self) -> usize {
                inline_work::<Self>()
            }
        })+
    };
}

scalar_keys!(
    (),
    bool,
    usize,
    u64,
    u32,
    u16,
    u8,
    u128,
    isize,
    i64,
    i32,
    i16,
    i8,
    i128
);

impl<K: PolicyKey> PolicyKey for Option<K> {
    fn key_work(&self) -> usize {
        self.as_ref()
            .map_or(0, PolicyKey::key_work)
            .saturating_add(inline_work::<Self>())
    }
}

impl<A: PolicyKey, B: PolicyKey> PolicyKey for (A, B) {
    fn key_work(&self) -> usize {
        self.0
            .key_work()
            .saturating_add(self.1.key_work())
            .saturating_add(inline_work::<Self>())
    }
}

impl<A: PolicyKey, B: PolicyKey, C: PolicyKey> PolicyKey for (A, B, C) {
    fn key_work(&self) -> usize {
        self.0
            .key_work()
            .saturating_add(self.1.key_work())
            .saturating_add(self.2.key_work())
            .saturating_add(inline_work::<Self>())
    }
}

impl<K: PolicyKey> PolicyKey for Box<K> {
    fn key_work(&self) -> usize {
        self.as_ref()
            .key_work()
            .saturating_add(inline_work::<Self>())
            .saturating_add(2)
    }
}

impl PolicyKey for String {
    fn key_work(&self) -> usize {
        self.len()
            .saturating_mul(2)
            .saturating_add(inline_work::<Self>())
            .saturating_add(2)
    }
}

impl PolicyKey for Vec<u8> {
    fn key_work(&self) -> usize {
        self.len()
            .saturating_mul(2)
            .saturating_add(inline_work::<Self>())
            .saturating_add(2)
    }
}

impl PolicyKey for FrameKey {
    fn key_work(&self) -> usize {
        self.jump_history
            .len()
            .saturating_mul(2)
            .saturating_add(inline_work::<Self>())
            .saturating_add(2)
    }
}

impl PolicyKey for MachineKey {
    fn key_work(&self) -> usize {
        self.frames
            .iter()
            .fold(inline_work::<Self>().saturating_add(2), |work, frame| {
                work.saturating_add(frame.key_work())
            })
    }
}

pub(in crate::analysis) fn classify<P: PartitionPolicy>(
    policy: &P,
    arrival: Arrival,
    budget: &mut WorkBudget,
) -> Result<P::Class, MachineError> {
    checkpoint(budget)?;
    let work = policy.classification_work(arrival);
    checkpoint(budget)?;
    reserve(budget, work)?;
    let class = policy.classify(arrival);
    checkpoint(budget)?;
    Ok(class)
}

pub(in crate::analysis) fn clone_key<K: PolicyKey>(
    key: &K,
    budget: &mut WorkBudget,
) -> Result<K, MachineError> {
    let work = quote(key, budget)?;
    reserve(budget, work)?;
    let cloned = key.clone();
    checkpoint(budget)?;
    Ok(cloned)
}

pub(in crate::analysis) fn checkpoint(budget: &WorkBudget) -> Result<(), MachineError> {
    if analysis_control::current_cancellation().is_cancelled()
        || budget.cancellation().is_cancelled()
    {
        Err(MachineError::Relations(QueryReason::Cancelled))
    } else if budget.exhausted() {
        Err(MachineError::Work)
    } else {
        Ok(())
    }
}

fn quote<K: PolicyKey>(key: &K, budget: &WorkBudget) -> Result<usize, MachineError> {
    checkpoint(budget)?;
    let work = key.key_work();
    checkpoint(budget)?;
    if work == usize::MAX {
        Err(MachineError::Work)
    } else {
        Ok(work)
    }
}

fn reserve(budget: &mut WorkBudget, work: usize) -> Result<(), MachineError> {
    checkpoint(budget)?;
    if work == usize::MAX || !budget.charge(work) {
        checkpoint(budget)?;
        return Err(MachineError::Work);
    }
    checkpoint(budget)
}

/// Private precision index, independent of the public structural projection.
///
/// The caller must reserve `work_size()` before cloning and check both
/// cancellation scopes after the actual clone. Borrowed iteration performs no
/// key comparisons or copies; consumers price any opaque operations themselves.
#[derive(Clone, Debug)]
pub(in crate::analysis) struct KeyIndex<K: PolicyKey> {
    entries: BTreeMap<K, usize>,
    maximum_key_work: usize,
    total_key_work: usize,
}

impl<K: PolicyKey> Default for KeyIndex<K> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: PolicyKey> KeyIndex<K> {
    pub(in crate::analysis) const fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
            maximum_key_work: 0,
            total_key_work: 0,
        }
    }

    pub(in crate::analysis) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    pub(in crate::analysis) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(in crate::analysis) fn keys(&self) -> btree_map::Keys<'_, K, usize> {
        self.entries.keys()
    }

    #[cfg(test)]
    pub(in crate::analysis) fn iter(&self) -> btree_map::Iter<'_, K, usize> {
        self.entries.iter()
    }

    /// Borrow only native IDs; callers prepay traversal before visiting them.
    pub(in crate::analysis) fn values(&self) -> btree_map::Values<'_, K, usize> {
        self.entries.values()
    }

    pub(in crate::analysis) fn work_size(&self) -> usize {
        Self::retention_work(self.len(), self.total_key_work)
    }

    pub(in crate::analysis) fn get(
        &self,
        key: &K,
        budget: &mut WorkBudget,
    ) -> Result<Option<usize>, MachineError> {
        let key_work = quote(key, budget)?;
        reserve(budget, self.search_work(key_work))?;
        let value = self.entries.get(key).copied();
        checkpoint(budget)?;
        Ok(value)
    }

    /// Prepay and locate the vacant entry before the builder touches native
    /// state. `build` must retain its native obligation on error and perform its
    /// own last cancellation check before committing that state. After success,
    /// the prepaid index commit cannot refuse or invoke opaque key operations.
    pub(in crate::analysis) fn insert_with(
        &mut self,
        key: K,
        budget: &mut WorkBudget,
        build: impl FnOnce(&mut WorkBudget) -> Result<usize, MachineError>,
    ) -> Result<usize, MachineError> {
        let key_work = quote(&key, budget)?;
        let next_len = self.len().saturating_add(1);
        let next_total = self.total_key_work.saturating_add(key_work);
        let next_maximum = self.maximum_key_work.max(key_work);
        if Self::retention_work(next_len, next_total) == usize::MAX {
            return Err(MachineError::Work);
        }
        // node::insert_recursing splits at most one node at each old level,
        // then may allocate a new root. Each split/insert touches at most 11
        // key/value slots, 12 edges and their parent links. Four passes over
        // those slots cover allocation/init, moves and link/length metadata;
        // entry_work includes fixed pointer bookkeeping. No opaque key clone
        // or comparison occurs in VacantEntry::insert after this search.
        let insertion_work = Self::levels(self.len())
            .saturating_add(1)
            .saturating_mul(BTREE_CAPACITY.saturating_add(1))
            .saturating_mul(Self::entry_work())
            .saturating_mul(4)
            .saturating_add(self.search_work(key_work));
        reserve(budget, insertion_work)?;
        let entry = self.entries.entry(key);
        checkpoint(budget)?;
        match entry {
            btree_map::Entry::Occupied(_) => Err(MachineError::Shape),
            btree_map::Entry::Vacant(entry) => {
                let value = build(budget)?;
                entry.insert(value);
                self.maximum_key_work = next_maximum;
                self.total_key_work = next_total;
                Ok(value)
            }
        }
    }

    fn search_work(&self, key_work: usize) -> usize {
        // search::find_key_index scans at most 11 keys per node; search_tree
        // descends one child at each level. It never revisits a resident key.
        // Cached maximum work bounds the other operand without another quote.
        self.comparison_limit()
            .saturating_mul(
                key_work
                    .saturating_add(self.maximum_key_work)
                    .saturating_add(1),
            )
            .saturating_add(1)
    }

    fn comparison_limit(&self) -> usize {
        self.len()
            .min(BTREE_CAPACITY.saturating_mul(Self::levels(self.len())))
    }

    fn levels(len: usize) -> usize {
        if len == 0 {
            return 0;
        }
        // map.rs guarantees a nonempty root and >=5 keys (>=6 children)
        // in every non-root node. A tree with L levels therefore retains at
        // least 2*6^(L-1)-1 keys. ceil(n/2) equals floor((n+1)/2) without
        // overflowing n+1, giving L <= 1+floor(log_6(ceil(n/2))).
        usize::try_from(len.div_ceil(2).ilog(BTREE_BRANCH))
            .unwrap_or(usize::MAX)
            .saturating_add(1)
    }

    fn entry_work() -> usize {
        inline_work::<K>()
            .saturating_add(inline_work::<usize>())
            .saturating_add(8)
    }

    fn retention_work(len: usize, key_work: usize) -> usize {
        inline_work::<Self>()
            .saturating_add(key_work)
            .saturating_add(len.saturating_mul(Self::entry_work()))
    }
}

#[cfg(test)]
mod tests;

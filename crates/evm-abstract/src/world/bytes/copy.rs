//! COPY keeps a complete checked root and a separate, bounded presentation.

use super::{COPY_FACT_LIMIT, EvmByteArray, RangeError, limits, zero_with};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    provenance::MAX_ORIGINS,
    symbolic::{ByteStateKind, ExprError},
};
use alloy_primitives::U256;
use std::collections::{BTreeMap, BTreeSet};

fn bounds<N: WordElement>(value: &EvmValue<N>) -> (U256, U256) {
    crate::world::unsigned_bounds(value)
}

// Only literal leaves are display certificates. No retained expression, stale
// cell or inactive implication is used to prove a number. The caller requires
// an empty resulting operand/conditional context, including both snapshots.
fn literal<N: WordElement>(array: &EvmByteArray<N>, index: usize) -> Option<EvmValue<N>> {
    let byte = match array.root.kind() {
        ByteStateKind::Zero => U256::ZERO,
        ByteStateKind::Exact(bytes) => U256::from(bytes.get(index).copied().unwrap_or(0)),
        ByteStateKind::Input {
            length, bindings, ..
        } => {
            // Use the captured root length, never the expandable wrapper.
            if U256::from(index) >= length.as_constant()? {
                return Some(zero_with());
            }
            let at = bindings
                .binary_search_by_key(&U256::from(index), |binding| binding.index)
                .ok()?;
            let binding = &bindings[at];
            let byte = binding.value.as_constant()?;
            if binding.numeric != EvmValue::<N>::constant(byte).numeric().observation() {
                return None;
            }
            byte
        }
        _ => return None,
    };
    (byte <= U256::from(255)).then(|| EvmValue::<N>::constant(byte))
}

pub(super) fn project<N: WordElement>(
    previous: &EvmByteArray<N>,
    source: &EvmByteArray<N>,
    destination: &EvmValue<N>,
    offset: &EvmValue<N>,
    size: &EvmValue<N>,
    complete: &EvmByteArray<N>,
) -> Result<BTreeMap<usize, EvmValue<N>>, RangeError> {
    let (size_lo, size_hi) = bounds(size);
    let (dst_lo, dst_hi) = bounds(destination);
    let starts = destination
        .singleton()
        .and_then(|at| usize::try_from(at).ok())
        .zip(offset.singleton().and_then(|at| usize::try_from(at).ok()));
    let mut old = previous.bytes.keys().take(COPY_FACT_LIMIT);
    let mut incoming = starts.map(|(_, start)| source.bytes.range(start..).take(COPY_FACT_LIMIT));
    let mut keys = BTreeSet::new();
    // Alternate the two finite streams so a large destination cache cannot
    // crowd out every translated source coordinate. At most 512 keys visited.
    for _ in 0..COPY_FACT_LIMIT {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        let old = old.next();
        let next = incoming.as_mut().and_then(Iterator::next);
        if old.is_none() && next.is_none() {
            break;
        }
        if let Some(index) = old {
            keys.insert(*index);
        }
        if keys.len() == COPY_FACT_LIMIT {
            break;
        }
        if let (Some((dst, src)), Some((index, _))) = (starts, next) {
            let delta = *index - src;
            if U256::from(delta) < size_hi
                && let Some(index) = dst.checked_add(delta)
            {
                keys.insert(index);
            }
        }
        if keys.len() == COPY_FACT_LIMIT {
            break;
        }
    }
    let literal_context = complete.operands.is_empty()
        && complete.conditional.len() == 0
        && [destination, offset, size].into_iter().all(|value| {
            value
                .expression()
                .and_then(|term| term.as_constant())
                .is_some_and(|literal| value.singleton() == Some(literal))
        });
    let mut bytes = BTreeMap::new();
    for index in keys {
        if analysis_control::current_cancellation().is_cancelled() {
            return Err(ExprError::Cancelled.into());
        }
        let at = U256::from(index);
        let untouched = at < dst_lo || (at >= dst_hi && at - dst_hi >= size_hi);
        let value = if !literal_context {
            None
        } else if untouched {
            literal(previous, index)
        } else {
            destination.singleton().and_then(|dst| {
                let delta = at.checked_sub(dst)?;
                if delta >= size_lo {
                    return None;
                }
                let index = offset.singleton()?.checked_add(delta)?;
                literal(source, usize::try_from(index).ok()?)
            })
        };
        bytes.insert(index, value.unwrap_or_else(EvmValue::<N>::unknown_byte));
    }
    Ok(bytes)
}

fn height(entries: usize) -> usize {
    (usize::BITS - entries.leading_zeros()) as usize + 1
}

fn probes(entries: usize) -> usize {
    // A B-tree node has at most eleven keys. Use a binary-tree height upper
    // bound and never more comparisons than a complete scan of its entries.
    entries.min(height(entries).saturating_mul(12))
}

fn scalar_work<N: WordElement>(value: &EvmValue<N>) -> usize {
    value
        .work_size()
        .saturating_add(value.expression().map_or_else(
            || value.numeric().work_size().saturating_add(8),
            |e| e.nodes(),
        ))
}

// Cached counts only: quoting the fee must not census metadata or walk roots.
struct SourceWork {
    bytes: usize,
    operands: usize,
    allocations: usize,
    conditional: usize,
    conditional_work: usize,
    nodes: usize,
}

impl<N: WordElement> From<&EvmByteArray<N>> for SourceWork {
    fn from(array: &EvmByteArray<N>) -> Self {
        Self {
            bytes: array.bytes.len(),
            operands: array.operands.len(),
            allocations: array.allocations.len(),
            conditional: array.conditional.len(),
            conditional_work: array.conditional.work_size(),
            nodes: array.root.nodes(),
        }
    }
}

fn work<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    target: &EvmByteArray<N>,
    source: SourceWork,
    ranges: usize,
    branches: usize,
    domain: AlgebraRef<'_, D>,
) -> usize {
    // Finite sets/expressions share handles; only bounded origin sets are deep
    // cloned. Numeric/provenance unions are componentwise, not capacity² tuples.
    let value_copy = 12 + MAX_ORIGINS + 4;
    let union = domain
        .capacity()
        .saturating_add(1)
        .saturating_mul(height(domain.capacity().saturating_add(1)))
        .saturating_add(
            MAX_ORIGINS
                .saturating_mul(2)
                .saturating_mul(height(MAX_ORIGINS * 2)),
        )
        .saturating_add(16);
    let facts = target
        .operands
        .len()
        .saturating_add(source.operands)
        .saturating_add(3);
    let clones = source
        .bytes
        .saturating_add(target.bytes.len())
        .saturating_add(source.operands)
        .saturating_add(target.operands.len())
        .saturating_add(4)
        .saturating_mul(value_copy)
        .saturating_add(
            source
                .allocations
                .saturating_add(target.allocations.len())
                .saturating_mul(3),
        )
        .saturating_add(source.conditional.saturating_add(target.conditional.len()));
    let source_merges = if target.operands.is_empty() {
        // expand only changes length/allocations; source import still sees an
        // empty operand map. Clone its ordered tree, validate cached key/value
        // bounds and normalize owned values, without per-key search/reinsert.
        // Retain may move two nodes' entries per level when deleting a key.
        // Pay key/value moves plus the additional clone, keeping the original
        // numeric/provenance union and value-copy envelopes for cleanup.
        let entry_copy = value_copy.saturating_add(1);
        source.operands.saturating_mul(
            height(source.operands)
                .saturating_mul(12)
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_mul(entry_copy)
                .saturating_add(8)
                .saturating_add(union.saturating_mul(4))
                .saturating_add(value_copy.saturating_mul(4)),
        )
    } else {
        // Arbitrary source keys are checked before map lookup. Pay their
        // policy-sized comparison shape without an unpaid census of keys.
        // State handles compare immutable IDs, never root histories.
        source.operands.saturating_mul(
            limits(domain)
                .max_nodes
                .saturating_mul(2)
                .saturating_add(1)
                .saturating_mul(probes(facts).saturating_mul(2).saturating_add(4))
                .saturating_add(union.saturating_mul(4))
                .saturating_add(value_copy.saturating_mul(4)),
        )
    };
    let ranges_work = ranges
        .saturating_mul(probes(facts).saturating_mul(12).saturating_add(32))
        .saturating_add(
            ranges
                .saturating_mul(probes(target.allocations.len().saturating_add(2)))
                .saturating_mul(4),
        )
        .saturating_add(branches.saturating_mul(union))
        .saturating_add(union.saturating_mul(12));
    // extend uses pointer deduplication between top-level conditional branches;
    // rebuilding cached scope metrics visits their existing fact payloads.
    let conditional = source
        .conditional
        .saturating_mul(target.conditional.len().saturating_add(source.conditional))
        .saturating_add(source.conditional_work)
        .saturating_add(target.conditional.work_size())
        .saturating_mul(4);
    let copies = target.root.copy_leaves();
    // COPY canonicalizes its immutable source once. Keep the old source/range
    // envelope for that walk, including its failed stopping guard. There are at
    // most 2*copies-1 target calls; eight logical actions per leaf cover their
    // new-view scalar guards, shallow builds and new common-Copy frames. Those
    // scalar fields are bounded additively by source nodes plus range terms,
    // including numeric-only synthesis and CALL's retained minimum expression.
    // Old target fields and recursive join_update factoring keep their separate
    // history fee below. Zero nodes is the empty-return-source sentinel: no COPY
    // runs in that lane, so preserve its original quote exactly.
    let source_work = if source.nodes == 0 {
        copies.saturating_mul(ranges).saturating_mul(8)
    } else {
        source
            .nodes
            .saturating_add(1)
            .saturating_mul(ranges)
            .saturating_mul(8)
            .saturating_add(
                copies
                    .saturating_mul(source.nodes.saturating_add(ranges).saturating_add(1))
                    .saturating_mul(8),
            )
    };
    let roots = source_work
        .saturating_add(
            target
                .root
                .nodes()
                .saturating_mul(height(target.root.nodes()))
                .saturating_mul(4),
        )
        .saturating_add(target.metadata_len().saturating_mul(4));
    let candidates = target
        .bytes
        .len()
        .min(COPY_FACT_LIMIT)
        .saturating_add(source.bytes.min(COPY_FACT_LIMIT));
    let projection = candidates
        .saturating_mul(
            value_copy
                .saturating_add(height(COPY_FACT_LIMIT).saturating_mul(4))
                .saturating_add(height(source.nodes))
                .saturating_add(height(target.root.nodes()))
                .saturating_add(32),
        )
        .saturating_add(height(source.bytes));
    clones
        .saturating_add(source_merges)
        .saturating_add(ranges_work)
        .saturating_add(conditional)
        .saturating_add(roots)
        .saturating_add(projection)
}

fn alternatives<N: WordElement>(value: &EvmValue<N>) -> usize {
    value.constants().map_or(1, BTreeSet::len)
}

impl<N: WordElement> EvmByteArray<N> {
    /// Complete COPY fee, including the caller's captured source clone and the
    /// bounded presentation. No logical range or metadata discovery is scanned.
    pub(crate) fn copy_work_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        source: &Self,
        destination: &EvmValue<N>,
        offset: &EvmValue<N>,
        size: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        let ranges = scalar_work(&self.length)
            .saturating_add(scalar_work(destination))
            .saturating_add(scalar_work(offset))
            .saturating_add(scalar_work(size))
            .saturating_add(8);
        let branches = alternatives(&self.length)
            .saturating_mul(alternatives(destination))
            .saturating_mul(alternatives(size));
        work(self, source.into(), ranges, branches, domain)
    }

    /// Code sources are constructed after admission; lengths bound their exact
    /// maps, and the number of accounts bounds a possible selected source root.
    pub(crate) fn copy_code_work_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        bytes: usize,
        accounts: usize,
        destination: &EvmValue<N>,
        offset: &EvmValue<N>,
        size: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        let source = SourceWork {
            bytes,
            // Exact/unknown code starts with no operands. Selected joins only
            // intersect those maps; refreshing byte cells does not add facts.
            operands: 0,
            allocations: 0,
            conditional: 0,
            conditional_work: 0,
            // Unknown account code contributes an Input (three nodes); every
            // selected join adds its selector and Ite, even with no code bytes.
            nodes: accounts.max(1).saturating_mul(6).saturating_add(3),
        };
        let ranges = scalar_work(&self.length)
            .saturating_add(scalar_work(destination))
            .saturating_add(scalar_work(offset))
            .saturating_add(scalar_work(size))
            .saturating_add(8);
        let branches = alternatives(&self.length)
            .saturating_mul(alternatives(destination))
            .saturating_mul(alternatives(size));
        let joined_nodes = source.nodes;
        work(self, source, ranges, branches, domain)
            .saturating_add(
                bytes
                    .saturating_mul(12 + MAX_ORIGINS + 4 + height(bytes))
                    .saturating_add(bytes.saturating_mul(2)),
            )
            .saturating_add(
                // Source construction still uses existing checked joins/refresh.
                // Their metadata checks also run for unknown, byte-empty accounts.
                accounts
                    .saturating_mul(joined_nodes)
                    .saturating_mul(height(joined_nodes))
                    .saturating_mul(12 + MAX_ORIGINS + 4),
            )
    }

    /// CALL's requested allocation precedes COPY unless either size is proved
    /// zero. Quote only normalization for a zero request; an empty source still
    /// needs the complete target expansion/retention envelope.
    pub(crate) fn copy_return_data_work_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        source: &Self,
        destination: &EvmValue<N>,
        requested: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        // Complete cached membership can independently certify zero. Opaque
        // carriers use the general envelope without discovery during quotation.
        let zero = |value: &EvmValue<N>| {
            value
                .constants()
                .is_some_and(|set| set.len() == 1 && set.contains(&U256::ZERO))
        };
        if zero(requested) {
            // Both normalizations still run, including numeric-only sources.
            return scalar_work(destination)
                .saturating_add(scalar_work(requested))
                .saturating_add(8)
                .saturating_mul(4);
        }
        if zero(&source.length) {
            let ranges = scalar_work(requested)
                .saturating_add(scalar_work(&self.length))
                .saturating_add(scalar_work(destination))
                .saturating_add(8)
                .saturating_mul(4);
            let branches = alternatives(&self.length)
                .saturating_mul(alternatives(destination))
                .saturating_mul(alternatives(requested));
            // minimum(right=0) and copy_from(size=0) skip all source work and
            // the second wrapper clone. Keep work's target checks unchanged:
            // an existing allocation calls check_existing on the whole target.
            let source = SourceWork {
                bytes: 0,
                operands: 0,
                allocations: 0,
                conditional: 0,
                conditional_work: 0,
                nodes: 0,
            };
            return work(self, source, ranges, branches, domain);
        }
        let ranges = scalar_work(requested)
            .saturating_add(scalar_work(&source.length))
            .saturating_add(scalar_work(&self.length))
            .saturating_add(scalar_work(destination))
            .saturating_add(8)
            .saturating_mul(4);
        let branches = alternatives(&self.length)
            .saturating_mul(alternatives(destination))
            .saturating_mul(alternatives(requested))
            .saturating_mul(alternatives(&source.length));
        work(self, source.into(), ranges, branches, domain)
            .saturating_add(self.metadata_len().saturating_mul(12 + MAX_ORIGINS + 4))
    }
}

#[cfg(test)]
mod canonical_tests;
#[cfg(test)]
mod tests;

#[cfg(test)]
impl EvmByteArray<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn copy_work(
        &self,
        source: &Self,
        destination: &EvmValue<crate::domain::NumericValue>,
        offset: &EvmValue<crate::domain::NumericValue>,
        size: &EvmValue<crate::domain::NumericValue>,
        domain: crate::domain::Domain,
    ) -> usize {
        self.copy_work_with(
            source,
            destination,
            offset,
            size,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
    }
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn copy_code_work(
        &self,
        bytes: usize,
        accounts: usize,
        destination: &EvmValue<crate::domain::NumericValue>,
        offset: &EvmValue<crate::domain::NumericValue>,
        size: &EvmValue<crate::domain::NumericValue>,
        domain: crate::domain::Domain,
    ) -> usize {
        self.copy_code_work_with(
            bytes,
            accounts,
            destination,
            offset,
            size,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
    }
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn copy_return_data_work(
        &self,
        source: &Self,
        destination: &EvmValue<crate::domain::NumericValue>,
        requested: &EvmValue<crate::domain::NumericValue>,
        domain: crate::domain::Domain,
    ) -> usize {
        self.copy_return_data_work_with(
            source,
            destination,
            requested,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
    }
}

//! Bounded presentation projection; the captured root and facts remain authoritative.

use super::{COPY_FACT_LIMIT, EvmByteArray, zero_with};
use crate::domain::AlgebraRef;
#[cfg(test)]
use crate::domain::{AbstractValue, Domain};
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    provenance::MAX_ORIGINS,
    symbolic::{ByteStateKind, ExprId, ExprKind},
};
#[cfg(test)]
use crate::world::ByteArray;
#[cfg(test)]
use crate::world::bytes::zero;
use alloy_primitives::U256;
use std::collections::BTreeMap;

type Facts<N = crate::domain::NumericValue> = BTreeMap<ExprId, EvmValue<N>>;

fn bounds<N: WordElement>(value: &EvmValue<N>) -> (U256, U256) {
    crate::world::unsigned_bounds(value)
}

/// A retained expression alone is not a certificate for its cached numeric
/// value: update_values/refine_operands can change the surrounding facts. Use
/// an explicit current-root guarantee, or check a leaf's constant binding.
fn certified<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &EvmByteArray<N>,
    index: usize,
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
) -> bool {
    if value
        .constants()
        .is_some_and(|set| set.len() > domain.capacity())
    {
        return false;
    }
    if let Some(expression) = value.expression()
        && let ExprKind::ByteRead { state, index: at } = expression.kind()
        && state == &source.root
        && at.as_constant() == Some(U256::from(index))
        && source.operands.get(expression) == Some(value)
    {
        return true;
    }
    if source.conditional.len() != 0 {
        return false;
    }
    let byte = match source.root.kind() {
        ByteStateKind::Zero => U256::ZERO,
        ByteStateKind::Exact(bytes) => U256::from(bytes.get(index).copied().unwrap_or(0)),
        ByteStateKind::Input { bindings, .. } => {
            let Ok(at) = bindings.binary_search_by_key(&U256::from(index), |b| b.index) else {
                return false;
            };
            let binding = &bindings[at];
            let Some(byte) = binding.value.as_constant() else {
                return false;
            };
            if binding.numeric != EvmValue::<N>::constant(byte).numeric().observation() {
                return false;
            }
            byte
        }
        _ => return false,
    };
    let constant_with = EvmValue::<N>::constant(byte);
    let expression = ExprId::constant(byte);
    // Constant-key guarantees can carry observed sources after normalization.
    // Keep their complete metadata rather than copying just a numeric byte.
    let expected = source.operands.get(&expression).map_or_else(
        || constant_with.clone(),
        |fact| {
            domain
                .join(fact, &constant_with)
                .with_numeric(constant_with.numeric().clone())
        },
    );
    value == &expected
}

fn source_bounds<N: WordElement>(source: &EvmByteArray<N>) -> Option<(U256, U256)> {
    match source.root.kind() {
        ByteStateKind::Exact(bytes) => {
            let length = U256::from(bytes.len());
            Some((length, length))
        }
        ByteStateKind::Input { length, .. } => Some(length.as_constant().map_or_else(
            || {
                source
                    .operands
                    .get(length)
                    .map_or((U256::ZERO, U256::MAX), bounds)
            },
            |length| (length, length),
        )),
        // Other certificates describe a whole current-root read, including
        // its internal padding. MSIZE/wrapper length is not a root boundary:
        // expand can change it without changing the captured input length.
        _ => None,
    }
}

pub(super) fn project<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    source: &EvmByteArray<N>,
    offset: &EvmValue<N>,
    size: &EvmValue<N>,
    offset_expr: &ExprId,
    size_expr: &ExprId,
    facts: &Facts<N>,
    domain: AlgebraRef<'_, D>,
) -> BTreeMap<usize, EvmValue<N>> {
    let mut bytes = BTreeMap::new();
    let Some(start) = offset.singleton() else {
        return bytes;
    };
    // Remembering the two range operands is the only possible context change.
    // Compare those keys only; never scan the full fact map or its implications.
    let same_context = [offset_expr, size_expr]
        .into_iter()
        .all(|key| source.operands.get(key) == facts.get(key));
    let Ok(host_start) = usize::try_from(start) else {
        return bytes;
    };
    let (output_lo, output_hi) = bounds(size);
    let source_bounds = if same_context {
        source_bounds(source)
    } else {
        None
    };
    for (source_index, value) in source.bytes.range(host_start..).take(COPY_FACT_LIMIT) {
        let at = U256::from(*source_index);
        let Some(delta) = at.checked_sub(start) else {
            continue;
        };
        if delta >= output_hi {
            break;
        }
        let Ok(index) = usize::try_from(delta) else {
            continue;
        };
        let value = if source_bounds.is_some_and(|(_, hi)| at >= hi) {
            zero_with()
        } else if !same_context || !certified(source, *source_index, value, domain) {
            // Keep the candidate coordinate for summary projection, which
            // rereads the complete root. The old display value proves nothing.
            EvmValue::<N>::unknown_byte()
        } else if delta >= output_lo || source_bounds.is_some_and(|(lo, _)| at >= lo) {
            // Union removes the source equality/alias when padding is
            // possible. These display values never enter operand facts.
            domain.join(value, &zero_with())
        } else {
            value.clone()
        };
        bytes.insert(index, value);
    }
    bytes
}

fn tree_height(entries: usize) -> usize {
    (usize::BITS - entries.leading_zeros()) as usize + 1
}

fn tree_work(entries: usize) -> usize {
    entries
        .saturating_add(1)
        .saturating_mul(tree_height(entries))
}

fn range_key_nodes<N: WordElement>(value: &EvmValue<N>) -> usize {
    if let Some(expression) = value.expression() {
        expression.nodes()
    } else if value.singleton().is_some() || value.numeric().is_top() {
        1
    } else {
        // expr(): one observed slot with a fresh binding and numeric payload,
        // followed by a read at zero. No operand-map census is needed.
        value.numeric().work_size().saturating_add(8)
    }
}

fn comparison_work(query_nodes: usize) -> usize {
    // ExprId ordering first compares cached fingerprints, then matching node
    // kinds lexicographically. A collision visits at most the query's expanded
    // shape (plus the paired nodes); an unmatched kind/arity stops that walk.
    // Byte/slot state handles compare IDs, without walking their histories.
    query_nodes.saturating_mul(2).saturating_add(1)
}

impl<N: WordElement> EvmByteArray<N> {
    /// Prepay the slice algorithm from cached counts, before any map/graph
    /// census. Exact payloads and logical lengths are retained by reference.
    pub(crate) fn slice_work_with<D: EvmNumericAlgebra<Element = N>>(
        &self,
        offset: &EvmValue<N>,
        size: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        // Twelve inline numeric fields/limbs, bounded origin copies, and the
        // finite-set, expression, identity and status handles of a value clone.
        let value_copy = 12 + MAX_ORIGINS + 4;
        // Componentwise finite/provenance unions, not arithmetic tuples. Set
        // construction includes ordered insertion; remaining components are
        // fixed-width word joins.
        let union = tree_work(domain.capacity().saturating_add(1))
            .saturating_add(tree_work(MAX_ORIGINS.saturating_mul(2)))
            .saturating_add(12);
        let range = offset.work_size().saturating_add(size.work_size());
        let facts = self.operands.len().saturating_add(2);
        let cached = if offset.singleton().is_some() {
            self.bytes.len().min(COPY_FACT_LIMIT)
        } else {
            0
        };
        // One canonicalization chain and checked root construction. Six
        // retained handles/checks per node cover view fields and the chain;
        // range normalization/observed-scalar construction has four passes.
        let roots = self.root.nodes().saturating_mul(6);
        let ranges = range.saturating_mul(4).saturating_add(32);
        let copied = self
            .operands
            .len()
            .saturating_mul(value_copy + 2)
            .saturating_add(self.conditional.len().saturating_add(1));
        // Each search compares at most every key, even on fingerprint
        // collisions. Four searches per range term cover contains/entry and
        // the two maps in the context check. The bound depends on the query,
        // not unrelated historical operand-key sizes or a policy assumption.
        let probes = comparison_work(range_key_nodes(offset))
            .saturating_add(comparison_work(range_key_nodes(size)))
            .saturating_mul(facts)
            .saturating_mul(4);
        let input_length = if matches!(self.root.kind(), ByteStateKind::Input { .. }) {
            comparison_work(self.root.nodes()).saturating_mul(facts)
        } else {
            0
        };
        let remembered = probes
            .saturating_add(input_length)
            .saturating_add(value_copy.saturating_add(union).saturating_mul(4));
        // Certificate queries are a constant or ByteRead(state ID, constant):
        // four scalar/handle nodes bound their comparison even against an
        // unrelated deep key. No per-cell arbitrary-expression query occurs.
        // Also pay for leaf binary search, coordinate scan/insertion, a value
        // copy, constant source metadata and a possible zero-padding union.
        let per_cell = comparison_work(4)
            .saturating_mul(facts)
            .saturating_mul(2)
            .saturating_add(tree_height(self.root.nodes()))
            .saturating_add(tree_height(COPY_FACT_LIMIT))
            .saturating_add(value_copy)
            .saturating_add(union.saturating_mul(2))
            .saturating_add(12);
        roots
            .saturating_add(ranges)
            .saturating_add(copied)
            .saturating_add(remembered)
            .saturating_add(tree_height(self.bytes.len()))
            .saturating_add(cached.saturating_mul(per_cell))
            .saturating_add(value_copy.saturating_mul(2))
    }
}

#[cfg(test)]
mod tests;

impl EvmByteArray<crate::domain::NumericValue> {
    /// Default numeric-policy compatibility wrapper.
    pub(crate) fn slice_work(
        &self,
        offset: &EvmValue<crate::domain::NumericValue>,
        size: &EvmValue<crate::domain::NumericValue>,
        domain: crate::domain::Domain,
    ) -> usize {
        self.slice_work_with(
            offset,
            size,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
    }
}

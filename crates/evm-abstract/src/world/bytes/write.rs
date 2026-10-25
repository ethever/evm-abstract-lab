//! Share a fresh word's byte projection only with its algebra's certificate.

use super::{EvmByteArray, constant};
use crate::domain::AlgebraRef;
use crate::domain::{EvmNumericAlgebra, WordElement};
use crate::domain::{
    EvmValue,
    provenance::Origin,
    symbolic::{ByteStateKind, ExprKind},
};
use alloy_primitives::U256;

#[cfg(test)]
mod tests;

#[cfg(test)]
// A test-only count of actual Reader evaluations, separate from byte reads.
pub(super) static NUMERIC_READS: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);

fn literal<N: WordElement>(value: &EvmValue<N>, n: U256) -> bool {
    // Canonical inline components avoid a whole-product singleton query's
    // general congruence division or finite-set membership before prepayment.
    value.known_bits().singleton() == Some(n)
        && value.interval().singleton() == Some(n)
        && value.congruence().singleton() == Some(n)
        && value.finite_constants().singleton() == Some(n)
        && value
            .expression()
            .is_none_or(|e| e.as_constant() == Some(n))
        && value.identity().input().is_none()
        && !value.symbolic_limit_reached()
        && !value.provenance().is_code_address()
        && value
            .provenance()
            .origins()
            .sources()
            .is_some_and(|s| s.len() == 1 && s.contains(&Origin::Constant))
}

// Structural admission is independent of numeric representation. The injected
// algebra certifies equal BYTE results and quotes only its numeric work.
pub(super) fn eligible<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &EvmByteArray<N>,
    offset: &EvmValue<N>,
    value: &EvmValue<N>,
    width: usize,
    domain: AlgebraRef<'_, D>,
) -> bool {
    // Admission never depends on MSIZE/allocation history: transfer may touch
    // them between quotation and execution of the already-funded writer.
    width == 32
        && array.memory
        && matches!(array.root.kind(), ByteStateKind::Zero)
        && array.bytes.is_empty()
        && array.operands.is_empty()
        && array.conditional.len() == 0
        && literal(offset, U256::ZERO)
        && value
            .expression()
            .is_some_and(|e| !matches!(e.kind(), ExprKind::Constant(_) | ExprKind::Ite { .. }))
        && domain.numeric_algebra().uniform_word_bytes(value.numeric())
}

pub(super) fn refresh<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &mut EvmByteArray<N>,
    domain: AlgebraRef<'_, D>,
) {
    let indices = array.bytes.keys().copied().collect::<Vec<_>>();
    let mut numeric = None;
    for index in indices {
        // Each call retains the original snapshot, literal-index remember,
        // Context, authoritative expression, checks and failure marker. Only
        // the first Reader executes BYTE; its complete AV is cloned 32 times.
        let byte = array
            .read_byte_with_numeric(&constant(index), domain, Some(&mut numeric))
            .unwrap_or_else(|_| EvmValue::<N>::unknown_byte().with_symbolic_limit());
        array.bytes.insert(index, byte);
    }
}

impl<N: WordElement> EvmByteArray<N> {
    /// Cached-only admission and complete quotation before effects or literals.
    pub(crate) fn uniform_word_write_work<D: EvmNumericAlgebra<Element = N>>(
        &self,
        offset: &EvmValue<N>,
        value: &EvmValue<N>,
        domain: AlgebraRef<'_, D>,
    ) -> Option<usize> {
        if !eligible(self, offset, value, 32, domain) {
            return None;
        }
        Some(work(self, value, domain))
    }
    /// The writer owns arithmetic pass admission: source provenance and host-index
    /// feasibility are structural properties, independent of numeric transfer.
    /// Opaque/replacement policies keep both passes unless their existing native
    /// scalar certificate proves candidate completeness and stable finite copies.
    pub(crate) fn memory_write_passes<D: EvmNumericAlgebra<Element = N>>(
        offset: &EvmValue<N>,
        value: &EvmValue<N>,
        width: usize,
        domain: AlgebraRef<'_, D>,
    ) -> usize {
        if !domain.numeric_algebra().supports_product_scalar_shortcuts() {
            return 2;
        }
        let numeric = value.numeric();
        let candidate = numeric.finite_constants().cardinality() == Some(1)
            || numeric.known_bits().singleton().is_some()
            || numeric.interval().singleton().is_some()
            || numeric.congruence().singleton().is_some();
        let constant_origin = value
            .provenance()
            .origins()
            .sources()
            .is_some_and(|origins| origins.len() == 1 && origins.contains(&Origin::Constant))
            && !value.provenance().is_code_address();
        let source_pass = usize::from(candidate && !constant_origin);
        // Normalization clones the numeric element without changing its finite
        // cover. One last-key lookup bounds every conversion and width addition.
        let cached = offset.constants().is_some_and(|values| {
            values.last().is_none_or(|last| {
                usize::try_from(*last)
                    .ok()
                    .and_then(|start| start.checked_add(width.saturating_sub(1)))
                    .is_some()
            })
        });
        source_pass + usize::from(!cached)
    }
}

fn work<N: WordElement, D: EvmNumericAlgebra<Element = N>>(
    array: &EvmByteArray<N>,
    value: &EvmValue<N>,
    domain: AlgebraRef<'_, D>,
) -> usize {
    use crate::domain::{provenance::MAX_ORIGINS, symbolic::ExprId};

    let numeric = domain
        .numeric_algebra()
        .uniform_word_numeric_work(value.numeric(), array.length.numeric());
    if numeric == usize::MAX {
        return usize::MAX;
    }
    // Origins are bounded owned copies; expressions/identities share handles.
    // Input numeric copies use their actual carrier quotes. The policy quote
    // owns copies of newly projected byte results, unavailable before payment.
    const METADATA_COPY: usize = MAX_ORIGINS + 4;
    let copy = [value, &array.length, &array.default]
        .into_iter()
        .map(|value| value.numeric().clone_work())
        .max()
        .unwrap_or(0)
        .saturating_add(METADATA_COPY);
    let nodes = value.expression().map_or(0, ExprId::nodes);
    // MSIZE/allocation history may change after quotation during touch_memory.
    // Reserve the original expansion and one additional allocation, without
    // changing eligibility when touch collapses a nonliteral length to 32.
    let length = &array.length;
    let exact_length = length.known_bits().singleton().is_some_and(|n| {
        length.interval().singleton() == Some(n)
            && length.congruence().singleton() == Some(n)
            && length.finite_constants().singleton() == Some(n)
    });
    let length_nodes = length.expression().map_or_else(
        || {
            if exact_length {
                1
            } else {
                // Observed root, fresh source, binding, key and SlotRead.
                length.numeric().work_size().saturating_add(5)
            }
        },
        ExprId::nodes,
    );
    let candidates = if exact_length {
        0
    } else {
        length
            .constants()
            .map_or(0, std::collections::BTreeSet::len)
    };
    let length_metadata = candidates.saturating_mul(METADATA_COPY.saturating_mul(6));
    let allocations = array.allocations.len().saturating_add(1);
    let height = (usize::BITS - allocations.leading_zeros()) as usize + 1;
    // Eleven keys per B-tree node. New literal pairs compare a kind tag or
    // fixed-width constants even on a fingerprint collision with an old key.
    let probes = allocations.min(height.saturating_mul(12));
    let staged = copy
        .saturating_mul(8)
        .saturating_add(allocations.saturating_mul(copy.saturating_mul(2).saturating_add(3)))
        .saturating_add(probes.saturating_mul(16).saturating_mul(2));
    let expansion = nodes
        .saturating_add(length_nodes.saturating_mul(4))
        .saturating_add(28)
        .saturating_mul(256)
        .saturating_add(length_metadata)
        .saturating_add(length.work_size())
        .saturating_add(array.default.work_size());
    // Per cell: source facts, literal metadata, Context and checked ByteRead.
    // Numeric result copies belong to the policy's complete numeric quote.
    let cells = METADATA_COPY
        .saturating_mul(6)
        .saturating_add(nodes.saturating_add(5).saturating_mul(256))
        .saturating_add(32)
        .saturating_mul(32);
    staged
        .saturating_add(expansion)
        .saturating_add(numeric)
        .saturating_add(cells)
}

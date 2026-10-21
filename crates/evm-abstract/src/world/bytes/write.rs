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
// algebra alone certifies equal BYTE results and funds the complete fast path.
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
        Some(domain.numeric_algebra().uniform_word_write_work(
            value,
            &self.length,
            &self.default,
            self.allocations.len(),
        ))
    }
}

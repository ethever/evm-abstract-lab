//! A1 shares only equal complete numeric projections within one fresh word write.

use super::{ByteArray, constant, numeric};
use crate::domain::{
    AbstractValue, Domain, Profile,
    known_bits::KnownBits,
    provenance::{MAX_ORIGINS, Origin},
    symbolic::{ByteStateKind, ExprId, ExprKind},
};
use alloy_primitives::U256;

#[cfg(test)]
mod tests;

#[cfg(test)]
// A test-only count of actual Reader evaluations, separate from byte reads.
pub(super) static NUMERIC_READS: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);

fn literal(value: &AbstractValue, n: U256) -> bool {
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

// All queries use inline fields or cached counts. In particular, allocation
// emptiness is not a guard: transfer's touch_memory precedes the staged write.
pub(super) fn eligible(
    array: &ByteArray,
    offset: &AbstractValue,
    value: &AbstractValue,
    width: usize,
    domain: Domain,
) -> bool {
    width == 32
        && array.memory
        && matches!(array.root.kind(), ByteStateKind::Zero)
        && array.bytes.is_empty()
        && array.operands.is_empty()
        && array.conditional.len() == 0
        && literal(offset, U256::ZERO)
        && domain.spec().profile() == Profile::Product
        && value.expression().is_some_and(|e| {
            !matches!(e.kind(), ExprKind::Constant(_) | ExprKind::Ite { .. })
        })
        && value.finite_constants().is_top()
        && *value.known_bits() == KnownBits::top()
        // Together these four components imply whole-product nonsingleton;
        // also exclude the interval/congruence all-exact transfer shortcuts.
        && value.interval().singleton().is_none()
        && value.congruence().singleton().is_none()
}

pub(super) fn refresh(array: &mut ByteArray, domain: Domain) {
    let indices = array.bytes.keys().copied().collect::<Vec<_>>();
    let mut numeric = None;
    for index in indices {
        // Each call retains the original snapshot, literal-index remember,
        // Context, authoritative expression, checks and failure marker. Only
        // the first Reader executes BYTE; its complete AV is cloned 32 times.
        let byte = array
            .read_byte_with_numeric(&constant(index), domain, Some(&mut numeric))
            .unwrap_or_else(|_| AbstractValue::unknown_byte().with_symbolic_limit());
        array.bytes.insert(index, byte);
    }
}

impl ByteArray {
    /// Quote A1 before any effect or traversal; all other writes keep the old fee.
    pub(crate) fn uniform_word_write_work(
        &self,
        offset: &AbstractValue,
        value: &AbstractValue,
        domain: Domain,
    ) -> Option<usize> {
        if !eligible(self, offset, value, 32, domain) {
            return None;
        }
        // Finite sets and expressions share handles; origins are bounded deep
        // copies. Literal scalar work is 13 numeric units plus one origin.
        const VALUE_COPY: usize = 12 + MAX_ORIGINS + 4;
        const LITERAL_WORK: usize = 14;
        let nodes = value.expression().map_or(0, ExprId::nodes);
        // MSIZE may be nonliteral even with a Zero content root. A length
        // guard would change eligibility when touch collapses {0,16} to 32.
        // Quote its original expansion instead, using cached shape only.
        let exact_length = self.length.known_bits().singleton().is_some_and(|n| {
            self.length.interval().singleton() == Some(n)
                && self.length.congruence().singleton() == Some(n)
                && self.length.finite_constants().singleton() == Some(n)
        });
        // A synthesized numeric-only source adds five nodes: observed root,
        // fresh source token, fresh binding value, constant key and SlotRead.
        let length_nodes = self.length.expression().map_or_else(
            || {
                if exact_length {
                    1
                } else {
                    self.length.numeric().work_size().saturating_add(5)
                }
            },
            ExprId::nodes,
        );
        let candidates = if exact_length {
            0
        } else {
            self.length
                .constants()
                .map_or(0, std::collections::BTreeSet::len)
        };
        let joined = candidates.min(domain.capacity()).saturating_add(1);
        let join_height = (usize::BITS - joined.leading_zeros()) as usize + 1;
        // At most k joins of a <=capacity set and one constant. Pay bounded
        // set insertion/comparison shape, copies and the 256-bit congruence
        // gcd/difference steps (4096, as in the numeric fixed-width base).
        let length_joins = candidates.saturating_mul(
            4096usize
                .saturating_add(joined.saturating_mul(join_height).saturating_mul(256))
                .saturating_add(VALUE_COPY.saturating_mul(6)),
        );
        // Reserve the allocation touch_memory may install after this quote.
        let allocations = self.allocations.len().saturating_add(1);
        let height = (usize::BITS - allocations.leading_zeros()) as usize + 1;
        // Each B-tree node has at most eleven keys. Only the new literal pair
        // is compared, so even a fingerprint collision with an arbitrary old
        // key compares a kind tag or fixed-width constants, never its children.
        let allocation_probes = allocations.min(height.saturating_mul(12));
        let staged = VALUE_COPY
            .saturating_mul(8)
            .saturating_add(allocations.saturating_mul(VALUE_COPY.saturating_mul(2) + 3))
            .saturating_add(allocation_probes.saturating_mul(16).saturating_mul(2));
        // Expanded uses only constant candidates/joins and checked scalar
        // terms here, never Domain::apply. Four old-expression copies bound
        // its maximum/guard terms.
        let expand_root = nodes
            .saturating_add(length_nodes.saturating_mul(4))
            .saturating_add(28)
            .saturating_mul(256)
            .saturating_add(length_joins)
            .saturating_add(self.length.work_size())
            .saturating_add(self.default.work_size());
        // Nonfinite source forces the ordinary Product fee: the empty-argument
        // base has the same one tuple, then add actual cached operand sizes.
        // Reader disables relations; authoritative expressions are paid below.
        let numeric = numeric::numeric_domain(domain)
            .operation_work(&[])
            .saturating_add(LITERAL_WORK)
            .saturating_add(value.work_size());
        // Per cell: complete numeric clone, one source-fact map clone, literal
        // remember/metadata copies, Context and checked ByteRead/attach. Roots
        // have nodes+3; ByteRead adds two. Index collection and B-tree insertion
        // visit at most 32 coordinates. No word-read packing is charged here.
        let cells = VALUE_COPY
            .saturating_mul(6)
            .saturating_add(nodes.saturating_add(5).saturating_mul(256))
            .saturating_add(32)
            .saturating_mul(32);
        Some(
            staged
                .saturating_add(expand_root)
                .saturating_add(numeric)
                .saturating_add(cells),
        )
    }
}

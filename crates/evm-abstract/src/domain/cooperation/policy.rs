//! Numeric assembly selects independently typed rules and a frozen work proof.
use super::{
    BoundsToMask, BuiltinWork, ComposedWork, DomainSpec, MaskToBounds, NumericReduction,
    NumericValue, ProductContext, ProductWork, Reduction, RuleError, Scope,
};
use crate::domain::{EvmNumericAlgebra, SummaryConstruction, WordElement};

/// Native Word256 product with replaceable mask/range reductions.
///
/// Rules run through typed capabilities and preserve the fixed input's joint
/// concretization. `from_spec` selects the exact built-in pair and its verified
/// work/packing laws. `new` uses each selected rule's `RuleWork` contract through
/// `ComposedWork`; `with_work` selects an explicit replacement allowance.
#[derive(Clone, Copy, Debug)]
pub struct ProductAlgebra<M = BoundsToMask, R = MaskToBounds, W = BuiltinWork> {
    spec: DomainSpec,
    masks: M,
    bounds: R,
    work: W,
    // Only from_spec constructs this certificate for the exact built-in bundle.
    // Replacing any policy drops it; custom rules retain conservative opt-outs.
    builtin: bool,
}

impl ProductAlgebra {
    /// Assemble the built-in product, including its shape-sensitive work proof.
    pub fn from_spec(spec: DomainSpec) -> Self {
        Self {
            spec,
            masks: BoundsToMask,
            bounds: MaskToBounds,
            work: BuiltinWork,
            builtin: true,
        }
    }
}
impl Default for ProductAlgebra {
    fn default() -> Self {
        Self::from_spec(DomainSpec::default())
    }
}
impl<M, R> ProductAlgebra<M, R, ComposedWork> {
    /// Assemble replacement rules with their explicit component work bounds.
    pub fn new(spec: DomainSpec, masks: M, bounds: R) -> Self {
        Self {
            spec,
            masks,
            bounds,
            work: ComposedWork,
            builtin: false,
        }
    }
}
impl<M, R, W> ProductAlgebra<M, R, W> {
    /// Replace the paid closure policy while preserving the numeric limits.
    pub fn with_work<P>(self, work: P) -> ProductAlgebra<M, R, P> {
        ProductAlgebra {
            spec: self.spec,
            masks: self.masks,
            bounds: self.bounds,
            work,
            builtin: false,
        }
    }
}
impl<M, R, W: ProductWork<M, R>> ProductAlgebra<M, R, W> {
    fn closure_work(&self, finite: usize) -> usize {
        self.work.closure_work(
            self.spec,
            &self.masks,
            &self.bounds,
            finite.max(self.spec.capacity()),
        )
    }
}
impl<M, R, W> EvmNumericAlgebra for ProductAlgebra<M, R, W>
where
    M: Copy + std::fmt::Debug,
    R: Copy + std::fmt::Debug,
    W: ProductWork<M, R>,
    for<'r, 'k> M: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
    for<'r, 'k> R: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
{
    type Element = NumericValue;
    fn spec(&self) -> DomainSpec {
        self.spec
    }
    fn project_numeric(&self, value: &NumericValue) -> NumericValue {
        self.spec.numeric_project(value)
    }
    fn join_numeric(&self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        self.spec.numeric_join(left, right)
    }
    fn widen_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.spec.numeric_widen(old, next)
    }
    fn widen_procedure_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.spec.numeric_widen_procedure(old, next)
    }
    fn reduce_numeric(&self, value: &NumericValue) -> NumericReduction<NumericValue> {
        if self.builtin && analysis_control::current_cancellation().is_cancelled() {
            return super::limited(value.clone(), crate::domain::ReductionStatus::Cancelled);
        }
        let literal = self
            .builtin
            .then(|| self.spec.native_scalar_literal(value))
            .flatten();
        if self.builtin && (literal.is_some() || self.spec.native_scalar_is_closed(value)) {
            let out = literal.map_or_else(|| value.clone(), NumericValue::constant);
            if analysis_control::current_cancellation().is_cancelled() {
                return super::limited(out, crate::domain::ReductionStatus::Cancelled);
            }
            return NumericReduction::stable(out);
        }
        if self.builtin {
            return super::close(self.spec, value.clone());
        }
        let work = self.closure_work(value.finite.work_size());
        super::close_with_work(self.spec, value.clone(), self.masks, self.bounds, work)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericReduction<NumericValue> {
        self.spec
            .numeric_apply_with(op, args, |value| self.reduce_numeric(&value))
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        if self.builtin {
            self.spec.numeric_operation_work(op, args)
        } else {
            self.spec
                .numeric_transfer_work(op, args, Some(self.closure_work(self.spec.capacity())))
        }
    }
    fn literal_quote_work(&self, _literal: crate::U256) -> usize {
        // Shared scalar literal factories and common literal metadata fit here.
        64
    }
    fn summary_join_work(
        &self,
        left: &NumericValue,
        right: &NumericValue,
        widen_procedure: bool,
    ) -> usize {
        if widen_procedure {
            // Native procedure widening first compares all product components:
            // its finite equality can visit the complete two input trees. The
            // remaining path clones next, normalizes at most two interval
            // segments, builds two fixed-width prefix masks and may discard the
            // finite component. No cooperation/reduction rule is invoked.
            return 1024usize
                .saturating_add(
                    left.finite_constants()
                        .work_size()
                        .saturating_add(right.finite_constants().work_size())
                        .saturating_mul(16),
                )
                .saturating_add(right.clone_work())
                .saturating_add(right.work_size());
        }
        let finite = crate::domain::finite_algebra::summary_union_work(
            left.finite_constants(),
            right.finite_constants(),
            self.spec.capacity(),
        );
        let congruence = left.congruence().join_work(right.congruence());
        // Inline mask/hull/nonzero assembly and final native result release.
        finite.saturating_add(congruence).saturating_add(256)
    }
    fn summary_canonical_work(&self, value: &NumericValue) -> usize {
        // NumericValue's canonicalizer discovers once, then clones the actual
        // value or creates a literal. Price both paths and final result release.
        value
            .singleton_work()
            .saturating_add(value.clone_work())
            .saturating_add(self.literal_quote_work(crate::U256::ZERO))
            .saturating_add(value.work_size().max(13))
    }
    fn summary_construction_work(&self, construction: SummaryConstruction) -> usize {
        // These exact NumericValue factories only initialize inline components;
        // ranges normalize at most two segments. No product closure executes,
        // even for replacement rules or an arbitrarily large finite capacity.
        match construction {
            SummaryConstruction::Top => 32,
            SummaryConstruction::UnknownByte
            | SummaryConstruction::UnknownAddress
            | SummaryConstruction::UnsignedRange { .. } => 512,
        }
    }
    fn summary_equal_work(&self, left: &NumericValue, right: &NumericValue) -> usize {
        // Only finite equality has an input-sized scan. All other product
        // fields are inline fixed-width words/tags, including congruence.
        128usize.saturating_add(
            left.finite_constants()
                .work_size()
                .saturating_add(right.finite_constants().work_size())
                .saturating_mul(16),
        )
    }
    fn summary_is_top_work(&self, _value: &NumericValue) -> usize {
        // Finite Top is a tag test; masks, both interval orders, congruence and
        // nonzero are inline. No finite candidate scan or modulo query occurs.
        128
    }
    fn summary_compact_work(&self, value: &NumericValue) -> usize {
        // Dense-range proof reads two B-tree endpoints and inline interval
        // bounds, then copies inline components/shared handles. No closure.
        let len = value.finite_constants().work_size();
        let height = usize::BITS as usize - len.max(1).leading_zeros() as usize;
        256usize
            .saturating_add(height.saturating_mul(128))
            .saturating_add(value.clone_work())
            .saturating_add(value.work_size())
    }
    fn summary_restore_work(&self, value: &NumericValue, previous: &NumericValue) -> usize {
        // The previous finite allocation alone can be restored; the current
        // interval proves redundancy. Price endpoints, both possible retained
        // payloads and shared-handle copies without running that proof.
        self.summary_compact_work(previous)
            .saturating_add(value.clone_work())
            .saturating_add(value.work_size())
            .saturating_add(32)
    }
    fn summary_observation_work(&self, observation: &crate::domain::WordObservation) -> usize {
        // WordObservation::numeric copies inline fields and one shared finite
        // handle; it neither filters candidates nor reduces components.
        64usize.saturating_add(observation.work_size())
    }
    fn projection_work(&self, _value: &NumericValue) -> usize {
        0
    }
    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
        if self.builtin
            && facts == 0
            && (self.spec.native_scalar_is_closed(value)
                || self.spec.native_scalar_literal(value).is_some())
        {
            // Complete inline/at-most-two-endpoint certificate, bounded interval
            // normalization, three possible import/reduction/project copies and
            // their result lifetimes. No cooperation kernel is built on this path.
            return self
                .spec
                .native_closed_scalar_work(value.finite.work_size())
                .unwrap();
        }
        if self.builtin {
            self.spec.numeric_refinement_work(value, facts)
        } else {
            self.spec.numeric_refinement_work_with(
                value,
                facts,
                Some(self.closure_work(value.finite.work_size().max(facts))),
            )
        }
    }
    fn refinement_import_work(
        &self,
        value: &NumericValue,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> Option<usize> {
        // Import first, then quote closure on the actual imported payload. An
        // atom count cannot bound a replacement rule's MemberOf payload work.
        Some(
            self.spec
                .numeric_import_work(value, facts, finite_values, expression_work),
        )
    }
    fn observe_byte_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericValue {
        self.spec
            .numeric_observe_byte_with(op, args, |value| self.reduce_numeric(&value))
    }
    fn byte_quotes_uniform(&self) -> bool {
        self.builtin
    }
    fn uniform_word_bytes(&self, word: &NumericValue) -> bool {
        self.builtin
            && word.finite_constants().is_top()
            && *word.known_bits() == crate::domain::known_bits::KnownBits::top()
            && word.interval().singleton().is_none()
            && word.congruence().singleton().is_none()
    }
    fn uniform_word_numeric_work(&self, word: &NumericValue, length: &NumericValue) -> usize {
        if !self.builtin {
            return usize::MAX;
        }
        // These exact carriers copy inline words and a shared finite handle.
        // No common value metadata or memory/graph representation is priced here.
        const NUMERIC_COPY: usize = 12;
        let exact_length = length.known_bits().singleton().is_some_and(|n| {
            length.interval().singleton() == Some(n)
                && length.congruence().singleton() == Some(n)
                && length.finite_constants().singleton() == Some(n)
        });
        let candidates = if exact_length {
            0
        } else {
            length
                .constants()
                .map_or(0, std::collections::BTreeSet::len)
        };
        let joined = candidates.min(self.spec.capacity()).saturating_add(1);
        let height = (usize::BITS - joined.leading_zeros()) as usize + 1;
        // At most k joins of a <=capacity finite set and one scalar: bounded
        // insertion/comparison, copies and fixed-width congruence gcd/difference.
        let joins = candidates.saturating_mul(
            4096usize
                .saturating_add(joined.saturating_mul(height).saturating_mul(256))
                .saturating_add(NUMERIC_COPY.saturating_mul(6)),
        );
        // The certified nonfinite word needs one BYTE evaluation. Quote its
        // actual payload; a literal index adds only a fixed inline numeric
        // operand, whose construction/release is reserved separately. Numeric
        // copies cover all retained cell results and the staging/query lifetimes.
        self.operation_work(revm_bytecode::opcode::BYTE, std::iter::once(word))
            .saturating_add(self.literal_quote_work(crate::U256::ZERO))
            .saturating_add(word.work_size())
            .saturating_add(NUMERIC_COPY.saturating_mul(6).saturating_mul(32))
            .saturating_add(joins)
    }
    fn byte_image_candidate(&self, value: &NumericValue) -> bool {
        value.finite_constants().is_top() && value.congruence().is_top()
    }
    fn byte_image_complete(&self, value: &NumericValue) -> bool {
        super::super::byte_projection::full_byte_image(value)
    }
    fn pack_word_work(&self, bytes: &[&NumericValue; 32]) -> Option<usize> {
        self.builtin
            .then(|| super::super::byte_projection::pack_work(self.spec, bytes))
    }
    fn pack_word_numeric(&self, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
        self.builtin
            .then(|| super::super::byte_projection::pack_numeric(self.spec, bytes))
            .flatten()
    }
    fn uniform_byte_top(&self, value: &NumericValue) -> bool {
        self.builtin && value == &NumericValue::unknown_byte()
    }
    fn supports_product_scalar_shortcuts(&self) -> bool {
        self.builtin
    }
}

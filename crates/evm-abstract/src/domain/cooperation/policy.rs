//! Numeric assembly selects independently typed rules and a frozen work proof.
use super::{
    BoundsToMask, BuiltinWork, ComposedWork, DomainSpec, MaskToBounds, NumericReduction,
    NumericValue, ProductContext, ProductWork, Reduction, RuleError, Scope,
};
use crate::domain::{EvmNumericAlgebra, EvmValue, ValueAlgebra};

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
                .numeric_transfer_work(args, Some(self.closure_work(self.spec.capacity())))
        }
    }
    fn literal_quote_work(&self, _literal: crate::U256) -> usize {
        // Shared scalar literal factories and common literal metadata fit here.
        64
    }
    fn projection_work(&self, _value: &NumericValue) -> usize {
        0
    }
    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
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
    fn uniform_word_write_work(
        &self,
        value: &EvmValue<NumericValue>,
        length: &EvmValue<NumericValue>,
        default: &EvmValue<NumericValue>,
        allocations: usize,
    ) -> usize {
        use crate::domain::{provenance::MAX_ORIGINS, symbolic::ExprId};

        if !self.builtin {
            return usize::MAX;
        }

        // Finite sets and expressions share handles; origins are bounded deep
        // copies. Literal scalar work is 13 numeric units plus one origin.
        const VALUE_COPY: usize = 12 + MAX_ORIGINS + 4;
        const LITERAL_WORK: usize = 14;
        let nodes = value.expression().map_or(0, ExprId::nodes);
        // MSIZE may be nonliteral even with a Zero content root. A length
        // guard would change eligibility when touch collapses {0,16} to 32.
        // Quote its original expansion instead, using cached shape only.
        let exact_length = length.known_bits().singleton().is_some_and(|n| {
            length.interval().singleton() == Some(n)
                && length.congruence().singleton() == Some(n)
                && length.finite_constants().singleton() == Some(n)
        });
        // A synthesized numeric-only source adds five nodes: observed root,
        // fresh source token, fresh binding value, constant key and SlotRead.
        let length_nodes = length.expression().map_or_else(
            || {
                if exact_length {
                    1
                } else {
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
        let joined = candidates.min(self.spec.capacity()).saturating_add(1);
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
        let allocations = allocations.saturating_add(1);
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
        // terms here, without numeric transfer. Four old-expression copies bound
        // its maximum/guard terms.
        let expand_root = nodes
            .saturating_add(length_nodes.saturating_mul(4))
            .saturating_add(28)
            .saturating_mul(256)
            .saturating_add(length_joins)
            .saturating_add(length.work_size())
            .saturating_add(default.work_size());
        // This default-only proof uses one nonfinite tuple: the numeric base
        // plus cached operand sizes bounds the default BYTE transfer. Reader
        // disables relations; authoritative expressions are paid below.
        let numeric = ValueAlgebra::new(self)
            .without_relations()
            .operation_work(revm_bytecode::opcode::BYTE, &[])
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
        staged
            .saturating_add(expand_root)
            .saturating_add(numeric)
            .saturating_add(cells)
    }
    fn memory_write_passes(
        &self,
        offset: &EvmValue<NumericValue>,
        value: &EvmValue<NumericValue>,
        width: usize,
    ) -> usize {
        self.spec.numeric_memory_write_passes(offset, value, width)
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

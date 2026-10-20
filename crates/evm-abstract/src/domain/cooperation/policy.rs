//! Numeric policy assembly selects independent registered components statically.
use super::{
    BoundsToMask, Domain, DomainSpec, MaskToBounds, NumericReduction, NumericValue, ProductContext,
    Reduction, RuleError, Scope, close,
};
use crate::domain::EvmNumericAlgebra;

/// Default EVM product with independently replaceable mask/range reductions.
///
/// Component rules are small Copy policies, evaluated through `Reduction` and
/// their typed capabilities. They must charge computation through `Charges` and
/// preserve the fixed input's joint concretization. The immutable numeric policy
/// itself is borrowed once by the outer `ValueAlgebra` API. Per-call epochs and
/// channels are owned by `close`; equal `DomainSpec`s never share stored facts.
#[derive(Clone, Copy, Debug)]
pub struct ProductAlgebra<M = BoundsToMask, R = MaskToBounds> {
    domain: Domain,
    masks: M,
    bounds: R,
}
impl<M, R> ProductAlgebra<M, R> {
    /// Assemble a frozen product policy with explicit independently typed rules.
    pub fn new(spec: DomainSpec, masks: M, bounds: R) -> Self {
        Self {
            domain: Domain::from_spec(spec),
            masks,
            bounds,
        }
    }
}
impl Default for ProductAlgebra {
    fn default() -> Self {
        Self::new(Domain::default().spec(), BoundsToMask, MaskToBounds)
    }
}
impl<M, R> EvmNumericAlgebra for ProductAlgebra<M, R>
where
    M: Copy + std::fmt::Debug,
    R: Copy + std::fmt::Debug,
    for<'r, 'k> M: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
    for<'r, 'k> R: Reduction<ProductContext<'r, 'k>, Task = Scope, Error = RuleError>,
{
    type Element = NumericValue;
    fn spec(&self) -> DomainSpec {
        self.domain.spec()
    }
    fn project_numeric(&self, v: &NumericValue) -> NumericValue {
        self.domain.numeric_project(v)
    }
    fn join_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.numeric_join(a, b)
    }
    fn widen_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.numeric_widen(a, b)
    }
    fn widen_procedure_numeric(&self, a: &NumericValue, b: &NumericValue) -> NumericValue {
        self.domain.numeric_widen_procedure(a, b)
    }
    fn reduce_numeric(&self, v: &NumericValue) -> NumericReduction<NumericValue> {
        close(self.domain, v.clone(), self.masks, self.bounds)
    }
    fn apply_numeric(&self, op: u8, args: &[NumericValue]) -> NumericReduction<NumericValue> {
        self.domain
            .numeric_apply_with(op, args, |v| self.reduce_numeric(&v))
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        self.domain.numeric_operation_work(args)
    }
    fn projection_work(&self, v: &NumericValue) -> usize {
        self.domain.numeric_projection_work(v)
    }
    fn refinement_work(&self, v: &NumericValue, facts: usize) -> usize {
        self.domain.numeric_refinement_work(v, facts)
    }
    fn observe_byte_numeric(&self, op: u8, args: &[NumericValue]) -> NumericValue {
        crate::domain::byte_projection::observe_with(self.domain, op, args, || {
            self.apply_numeric(op, args).value
        })
    }
    fn byte_image_candidate(&self, v: &NumericValue) -> bool {
        EvmNumericAlgebra::byte_image_candidate(&self.domain, v)
    }
    fn byte_image_complete(&self, v: &NumericValue) -> bool {
        EvmNumericAlgebra::byte_image_complete(&self.domain, v)
    }
    fn pack_word_numeric(&self, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
        EvmNumericAlgebra::pack_word_numeric(&self.domain, bytes)
    }
    fn uniform_byte_top(&self, v: &NumericValue) -> bool {
        EvmNumericAlgebra::uniform_byte_top(&self.domain, v)
    }
    fn supports_product_scalar_shortcuts(&self) -> bool {
        true
    }
}

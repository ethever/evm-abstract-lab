//! Numeric policy assembly selects independent registered components statically.
use super::{
    BoundsToMask, ComposedWork, Domain, DomainSpec, MaskToBounds, NumericReduction, NumericValue,
    ProductContext, ProductWork, Reduction, RuleError, Scope,
};
use crate::domain::EvmNumericAlgebra;

/// Default EVM product with independently replaceable mask/range reductions.
///
/// Component rules are small Copy policies, evaluated through `Reduction` and
/// their typed capabilities. They must charge computation through `Charges` and
/// preserve the fixed input's joint concretization. The immutable numeric policy
/// itself is borrowed once by the outer `ValueAlgebra` API. Per-call epochs and
/// channels are owned by `close`; equal `DomainSpec`s never share stored facts.
/// The default [`ComposedWork`] requires each selected rule's [`super::RuleWork`]
/// contract. `with_work` can inject a policy for this exact pair, including an
/// explicit [`super::BoundedWork`] fallback. Built-in convergence proofs are not
/// applied to replacement rules.
#[derive(Clone, Copy, Debug)]
pub struct ProductAlgebra<M = BoundsToMask, R = MaskToBounds, W = ComposedWork> {
    domain: Domain,
    masks: M,
    bounds: R,
    work: W,
}

impl<M, R> ProductAlgebra<M, R> {
    /// Assemble independently typed rules with their explicit component costs.
    pub fn new(spec: DomainSpec, masks: M, bounds: R) -> Self {
        Self {
            domain: Domain::from_spec(spec),
            masks,
            bounds,
            work: ComposedWork,
        }
    }
}
impl<M, R, W> ProductAlgebra<M, R, W> {
    /// Select the closure's paid work policy without changing its precision,
    /// fact capacity, or maximum number of rule evaluations.
    pub fn with_work<P>(self, work: P) -> ProductAlgebra<M, R, P> {
        ProductAlgebra {
            domain: self.domain,
            masks: self.masks,
            bounds: self.bounds,
            work,
        }
    }
}
impl Default for ProductAlgebra {
    fn default() -> Self {
        Self::new(Domain::default().spec(), BoundsToMask, MaskToBounds)
    }
}
impl<M, R, W> ProductAlgebra<M, R, W>
where
    W: ProductWork<M, R>,
{
    fn closure_work(&self, finite: usize) -> usize {
        self.work.closure_work(
            self.domain.spec(),
            &self.masks,
            &self.bounds,
            finite.max(self.domain.capacity()),
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
        let work = self.closure_work(v.finite.work_size());
        super::close_with_work(self.domain, v.clone(), self.masks, self.bounds, work)
    }
    fn apply_numeric(&self, op: u8, args: &[NumericValue]) -> NumericReduction<NumericValue> {
        self.domain
            .numeric_apply_with(op, args, |v| self.reduce_numeric(&v))
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        self.domain
            .numeric_operation_work_with(args, Some(self.closure_work(self.domain.capacity())))
    }
    fn literal_quote_work(&self, literal: crate::U256) -> usize {
        // Rules do not replace this assembly's NumericValue literal factory.
        self.domain.literal_quote_work(literal)
    }
    fn projection_work(&self, v: &NumericValue) -> usize {
        self.domain.numeric_projection_work(v)
    }
    fn refinement_work(&self, v: &NumericValue, facts: usize) -> usize {
        self.domain.numeric_refinement_work_with(
            v,
            facts,
            Some(self.closure_work(v.finite.work_size().max(facts))),
        )
    }
    fn refinement_import_work(
        &self,
        v: &NumericValue,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> Option<usize> {
        // This assembly retains Domain's import implementation, but its rules
        // may cost more on the imported payload than on the original input.
        // Pay import first, then quote the actual result through this policy;
        // an atom count alone cannot bound a new MemberOf payload's rule work.
        Some(
            self.domain
                .numeric_import_work(v, facts, finite_values, expression_work),
        )
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
    // Replacing M/R can change even a cube's scalar-fold presentation. Retain
    // the default pack opt-out rather than forwarding Domain's stronger law.
    fn uniform_byte_top(&self, v: &NumericValue) -> bool {
        EvmNumericAlgebra::uniform_byte_top(&self.domain, v)
    }
    fn supports_product_scalar_shortcuts(&self) -> bool {
        true
    }
}

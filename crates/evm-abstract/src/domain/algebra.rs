//! Injected numeric semantics with the common EVM metadata algebra.
//!
//! Policies are immutable handles; `ValueAlgebra<&D>` borrows a non-Copy policy.
//! Executable values, saved stores and summaries retain `D::Element`. Only
//! immutable-source observations cross the explicitly coarsening vocabulary.

use super::{
    Domain, DomainSpec, EvmReduction, EvmValue, NumericValue, ReductionStatus, WordElement,
    WorkExhausted, facts, provenance::Provenance, symbolic,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::fmt::Debug;

/// Numeric transfer/refinement result, independent of provenance and expressions.
#[derive(Clone, Debug)]
pub struct NumericReduction<N: WordElement> {
    /// Sound upper approximation; retained even on a proved contradiction.
    pub value: N,
    /// Local closure or explicit precision/contradiction boundary.
    pub status: ReductionStatus,
    /// Complete bounded propagation rounds.
    pub rounds: usize,
    /// Strictly strengthened semantic facts.
    pub strengthened: usize,
}
impl<N: WordElement> NumericReduction<N> {
    /// An already stable numeric result without any reduction work.
    pub fn stable(value: N) -> Self {
        Self {
            value,
            status: ReductionStatus::Stable,
            rounds: 0,
            strengthened: 0,
        }
    }
    fn attach(self, value: &EvmValue<N>) -> EvmReduction<N> {
        EvmReduction {
            value: value.copy_metadata(self.value),
            status: self.status,
            rounds: self.rounds,
            strengthened: self.strengthened,
            symbolic_error: None,
        }
    }
}

/// Statically selected EVM numeric semantics and frozen per-run policy.
///
/// `join_numeric` covers both inputs without local reduction. Widening is a
/// separately named upper approximation; it must not be undone by reductions.
/// Reduction consequences hold only in the current immutable input generation.
/// Operations use EVM pop order and Word256 wrapping semantics; unsupported pure
/// operations return a sound upper approximation rather than an invented value.
/// All methods are caller-precharged by the public work quotation methods.
pub trait EvmNumericAlgebra: Debug {
    /// The actual executable numeric element, not a report-only adapter.
    type Element: WordElement;
    /// Frozen common execution/reduction/retention limits.
    fn spec(&self) -> DomainSpec;
    /// Sound policy projection, independent of metadata/source identity.
    fn project_numeric(&self, value: &Self::Element) -> Self::Element;
    /// Stored upper-bound join; must cover both arguments.
    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element;
    /// Cover a globally joined value while accelerating moving bounds.
    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element;
    /// Procedure acceleration including cross-component anti-feedback policy.
    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element;
    /// Registered local reduction, retaining sound information on a limit.
    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element>;
    /// Pure numeric instruction, with stack top first in `args`.
    fn apply_numeric(&self, op: u8, args: &[Self::Element]) -> NumericReduction<Self::Element>;
    /// Upper bound for numeric transfer and its dependency/reduction work.
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a;
    /// Upper bound for policy projection beyond ordinary input cloning.
    fn projection_work(&self, value: &Self::Element) -> usize;
    /// Upper bound for guarantee import and reduction.
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize;
    /// Optional byte observation shortcut; the default runs ordinary transfer.
    fn observe_byte_numeric(&self, op: u8, args: &[Self::Element]) -> Self::Element {
        self.apply_numeric(op, args).value
    }
    /// Cheap admission for an explicitly paid full-byte-image certificate.
    fn byte_image_candidate(&self, _value: &Self::Element) -> bool {
        false
    }
    /// Full-byte-image certificate, called only after its fixed-width payment.
    fn byte_image_complete(&self, _value: &Self::Element) -> bool {
        false
    }
    /// Optional numeric replacement for the ordered 32-byte SHL/OR fold.
    fn pack_word_numeric(&self, _bytes: &[&Self::Element; 32]) -> Option<Self::Element> {
        None
    }
    /// Recognize the default scalar fold's unconstrained-byte normal form.
    fn uniform_byte_top(&self, _value: &Self::Element) -> bool {
        false
    }
    /// Opt into the default product's fixed-width scalar import/singleton cost proofs.
    /// Custom representations decline unless those representation/work contracts hold.
    fn supports_product_scalar_shortcuts(&self) -> bool {
        false
    }
}

impl<D: EvmNumericAlgebra + ?Sized> EvmNumericAlgebra for &D {
    type Element = D::Element;
    fn spec(&self) -> DomainSpec {
        D::spec(self)
    }
    fn project_numeric(&self, value: &Self::Element) -> Self::Element {
        D::project_numeric(self, value)
    }
    fn join_numeric(&self, left: &Self::Element, right: &Self::Element) -> Self::Element {
        D::join_numeric(self, left, right)
    }
    fn widen_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        D::widen_numeric(self, old, next)
    }
    fn widen_procedure_numeric(&self, old: &Self::Element, next: &Self::Element) -> Self::Element {
        D::widen_procedure_numeric(self, old, next)
    }
    fn reduce_numeric(&self, value: &Self::Element) -> NumericReduction<Self::Element> {
        D::reduce_numeric(self, value)
    }
    fn apply_numeric(&self, op: u8, args: &[Self::Element]) -> NumericReduction<Self::Element> {
        D::apply_numeric(self, op, args)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        D::operation_work(self, args)
    }
    fn projection_work(&self, value: &Self::Element) -> usize {
        D::projection_work(self, value)
    }
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize {
        D::refinement_work(self, value, facts)
    }
    fn observe_byte_numeric(&self, op: u8, args: &[Self::Element]) -> Self::Element {
        D::observe_byte_numeric(self, op, args)
    }
    fn byte_image_candidate(&self, value: &Self::Element) -> bool {
        D::byte_image_candidate(self, value)
    }
    fn byte_image_complete(&self, value: &Self::Element) -> bool {
        D::byte_image_complete(self, value)
    }
    fn pack_word_numeric(&self, bytes: &[&Self::Element; 32]) -> Option<Self::Element> {
        D::pack_word_numeric(self, bytes)
    }
    fn uniform_byte_top(&self, value: &Self::Element) -> bool {
        D::uniform_byte_top(self, value)
    }
    fn supports_product_scalar_shortcuts(&self) -> bool {
        D::supports_product_scalar_shortcuts(self)
    }
}

/// Cheap recursive view over the original immutable numeric policy.
pub type AlgebraRef<'a, D> = ValueAlgebra<&'a D>;

/// Common source/identity/expression algebra around an injected numeric policy.
#[derive(Clone, Copy, Debug)]
pub struct ValueAlgebra<D: EvmNumericAlgebra> {
    numeric: D,
    relations: Option<super::relational::RelationLimits>,
}

impl<D: EvmNumericAlgebra> ValueAlgebra<D> {
    /// Freeze an owned or borrowed numeric policy handle.
    pub fn new(numeric: D) -> Self {
        Self {
            numeric,
            relations: None,
        }
    }
    /// The exact injected numeric policy; no default domain is substituted.
    pub fn numeric_algebra(&self) -> &D {
        &self.numeric
    }
    /// A cheap handle even when the policy owns a large immutable configuration.
    pub fn borrowed(&self) -> ValueAlgebra<&D> {
        ValueAlgebra {
            numeric: &self.numeric,
            relations: self.relations,
        }
    }
    /// Common frozen limits used by metadata/retention and public reports.
    pub fn spec(&self) -> DomainSpec {
        let spec = self.numeric.spec();
        self.relations
            .map_or(spec, |relations| spec.with_relations(relations))
    }
    /// Refine existing custom state; never rebuild it through an exported snapshot.
    pub fn constrain_numeric(
        &self,
        value: &D::Element,
        facts: &super::WordObservation,
    ) -> Result<NumericReduction<D::Element>, facts::FactError> {
        let constrained = value.constrain(facts)?;
        Ok(self.numeric.reduce_numeric(&constrained))
    }
    /// Cheap admission for the selected domain's full-byte-image certificate.
    pub fn byte_image_candidate(&self, value: &D::Element) -> bool {
        self.numeric.byte_image_candidate(value)
    }
    /// Explicitly paid full-byte-image certificate of the selected domain.
    pub fn byte_image_complete(&self, value: &D::Element) -> bool {
        self.numeric.byte_image_complete(value)
    }
    /// Optional domain-owned replacement for the 32-byte numeric scalar fold.
    pub fn pack_word_numeric(&self, bytes: &[&D::Element; 32]) -> Option<D::Element> {
        self.numeric.pack_word_numeric(bytes)
    }
    /// Domain-owned canonical unknown-byte recognition.
    pub fn uniform_byte_top(&self, value: &D::Element) -> bool {
        self.numeric.uniform_byte_top(value)
    }
    /// Full-candidate storage capacity from the frozen common policy.
    pub fn capacity(&self) -> usize {
        self.spec().capacity()
    }
    /// Numeric policy projection, preserving immutable-input identity.
    pub fn project(&self, value: &EvmValue<D::Element>) -> EvmValue<D::Element> {
        let mut out = value.copy_metadata(self.numeric.project_numeric(value.numeric()));
        out.forget_identity();
        if !self.spec().relations().enabled {
            out.forget_expression();
            out.symbolic_limit = false;
        }
        out
    }
    /// Join without exchanging facts. Source categories never create equality.
    pub fn join(
        &self,
        left: &EvmValue<D::Element>,
        right: &EvmValue<D::Element>,
    ) -> EvmValue<D::Element> {
        let mut out = EvmValue {
            numeric: self.numeric.join_numeric(left.numeric(), right.numeric()),
            provenance: left.provenance.join(&right.provenance),
            identity: left.identity.join(&right.identity),
            expression: left
                .expression
                .as_ref()
                .filter(|e| Some(*e) == right.expression.as_ref())
                .cloned(),
            symbolic_limit: left.symbolic_limit || right.symbolic_limit,
        };
        if out.singleton().is_some() {
            out.symbolic_limit = false;
        }
        out
    }
    /// Sound global acceleration; local descending reducers never call this.
    pub fn widen(
        &self,
        old: &EvmValue<D::Element>,
        next: &EvmValue<D::Element>,
    ) -> EvmValue<D::Element> {
        self.widen_metadata(
            old,
            next,
            self.numeric.widen_numeric(old.numeric(), next.numeric()),
        )
    }
    fn widen_metadata(
        &self,
        old: &EvmValue<D::Element>,
        next: &EvmValue<D::Element>,
        numeric: D::Element,
    ) -> EvmValue<D::Element> {
        let mut out = next.copy_metadata(numeric);
        out.identity = old.identity.join(&next.identity);
        out.provenance = old.provenance.join(&next.provenance);
        out.symbolic_limit =
            (old.symbolic_limit || next.symbolic_limit) && out.singleton().is_none();
        if old.expression != next.expression {
            out.forget_expression();
        }
        out
    }
    /// Procedure acceleration preserves the selected algebra's convergence policy.
    pub fn widen_procedure(
        &self,
        old: &EvmValue<D::Element>,
        next: &EvmValue<D::Element>,
    ) -> EvmValue<D::Element> {
        let numeric = self
            .numeric
            .widen_procedure_numeric(old.numeric(), next.numeric());
        let no_acceleration = old.numeric() == next.numeric()
            || next.constants().is_some_and(|values| {
                values.len() <= self.spec().widening_after_updates().saturating_add(1)
            })
            || (self.spec().profile() == super::Profile::Product
                && old.interval().unsigned_bounds() == next.interval().unsigned_bounds());
        if &numeric == next.numeric() && no_acceleration {
            return next.copy_metadata(numeric);
        }
        if self.spec().profile() == super::Profile::ConstantsOnly {
            return EvmValue::from_numeric(numeric);
        }
        self.widen_metadata(old, next, numeric)
    }
    /// Sound local closure; input and all metadata remain independently owned.
    pub fn reduce(&self, value: &EvmValue<D::Element>) -> EvmReduction<D::Element> {
        self.numeric.reduce_numeric(value.numeric()).attach(value)
    }
    /// Construct from validated guarantees; unsupported custom constraints may coarsen.
    pub fn from_facts(
        &self,
        predicates: &[facts::UnaryPredicate],
    ) -> Result<EvmValue<D::Element>, facts::FactError> {
        let mut lattice = facts::FactLattice::new(self.spec().fact_limit());
        for predicate in predicates {
            lattice.insert(facts::Fact::Unary(facts::UnaryFact::new(
                facts::Symbol::THIS,
                predicate.clone(),
            )))?;
        }
        let initial = super::reduce::import(&EvmValue::top(), &lattice)?;
        let reduced = self.reduce(&initial);
        match reduced.status {
            ReductionStatus::Empty => Err(facts::FactError::Contradiction {
                subject: facts::Symbol::THIS,
            }),
            ReductionStatus::OriginConflict => Err(facts::FactError::OriginContradiction {
                subject: facts::Symbol::THIS,
            }),
            _ => Ok(self.project(&reduced.value)),
        }
    }
    /// Pure numeric instruction with shared source and expression semantics.
    pub fn apply(&self, op: u8, args: &[EvmValue<D::Element>]) -> EvmValue<D::Element> {
        self.apply_detailed(op, args).value
    }
    /// Numeric and optional expression status are independently observable.
    pub fn apply_detailed(
        &self,
        op: u8,
        args: &[EvmValue<D::Element>],
    ) -> EvmReduction<D::Element> {
        let expected = match op {
            opcode::ISZERO | opcode::NOT | opcode::CLZ => 1,
            opcode::ADDMOD | opcode::MULMOD => 3,
            _ => 2,
        };
        let supported = matches!(op,0x01..=0x0b|0x10..=0x1e) && args.len() == expected;
        let exact = (supported && args.len() > 1 && args[0].same_identity(&args[1]))
            .then(|| match op {
                opcode::EQ => Some(U256::from(1)),
                opcode::XOR | opcode::SUB | opcode::LT | opcode::GT | opcode::SLT | opcode::SGT => {
                    Some(U256::ZERO)
                }
                _ => None,
            })
            .flatten();
        let numeric = if let Some(exact) = exact {
            NumericReduction::stable(D::Element::constant(exact))
        } else {
            self.numeric.apply_numeric(
                op,
                &args.iter().map(|v| v.numeric.clone()).collect::<Vec<_>>(),
            )
        };
        let mut reduced = numeric.attach(&EvmValue::top());
        reduced.value.symbolic_limit = args.iter().any(EvmValue::symbolic_limit_reached)
            && reduced.value.singleton().is_none();
        if supported {
            reduced.value.provenance = Provenance::transfer(
                &args
                    .iter()
                    .map(|v| v.provenance.clone())
                    .collect::<Vec<_>>(),
            );
        }
        let limits = self.spec().relations();
        if limits.enabled
            && reduced.value.singleton().is_none()
            && let Some(expressions) = args
                .iter()
                .map(EvmValue::symbolic_expression)
                .collect::<Option<Vec<_>>>()
        {
            match symbolic::ExprId::operation(op, &expressions, limits.expressions()) {
                Ok(expression) => {
                    if let Some(exact) = expression.as_constant() {
                        reduced.value = reduced.value.with_numeric(D::Element::constant(exact));
                        reduced.value.forget_expression();
                    } else {
                        reduced.value = reduced.value.with_expression(expression);
                    }
                }
                Err(source) => {
                    if matches!(source, symbolic::ExprError::Limit { .. }) {
                        reduced.value.symbolic_limit = true;
                    }
                    if reduced.status == ReductionStatus::Stable {
                        reduced.status = ReductionStatus::SymbolicLimit;
                    }
                    reduced.symbolic_error = Some(source);
                }
            }
        }
        reduced
    }
    /// Use exactly the caller's root ledger; refusal returns no partial result.
    pub fn apply_budgeted(
        &self,
        op: u8,
        args: &[EvmValue<D::Element>],
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<EvmReduction<D::Element>, WorkExhausted> {
        if !budget.charge(self.operation_work(args)) {
            return Err(WorkExhausted);
        }
        Ok(self.apply_detailed(op, args))
    }
    /// Numeric address truncation does not assert account presence.
    pub fn address_projection(&self, value: &EvmValue<D::Element>) -> EvmValue<D::Element> {
        let mut out = self.apply(
            opcode::AND,
            &[value.clone(), EvmValue::constant(U256::MAX >> 96usize)],
        );
        out.provenance = out.provenance.with_code_address_role();
        out
    }
    /// Precharged byte observation. Only binary BYTE/AND enter the optional
    /// numeric hook; other operations/arities retain ordinary apply metadata.
    pub fn observe_byte(&self, op: u8, args: &[EvmValue<D::Element>]) -> EvmValue<D::Element> {
        if self.spec().relations().enabled
            || !matches!(op, opcode::BYTE | opcode::AND)
            || args.len() != 2
        {
            return self.apply(op, args);
        }
        let numeric = self.numeric.observe_byte_numeric(
            op,
            &args.iter().map(|v| v.numeric.clone()).collect::<Vec<_>>(),
        );
        let mut out = EvmValue::from_numeric(numeric);
        out.provenance = Provenance::transfer(
            &args
                .iter()
                .map(|v| v.provenance.clone())
                .collect::<Vec<_>>(),
        );
        out.symbolic_limit =
            args.iter().any(EvmValue::symbolic_limit_reached) && out.singleton().is_none();
        out
    }
    /// Numeric plus expression/metadata work quotation; no work is run here.
    pub fn operation_work(&self, args: &[EvmValue<D::Element>]) -> usize {
        let symbolic = if self.spec().relations().enabled {
            args.iter()
                .fold(1usize, |cost, value| {
                    cost.saturating_add(value.expression().map_or(1, symbolic::ExprId::nodes))
                })
                .saturating_mul(256)
        } else {
            0
        };
        self.numeric
            .operation_work(args.iter().map(EvmValue::numeric))
            .saturating_add(symbolic)
            // Numeric execution materializes the operand slice only after payment.
            .saturating_add(
                args.iter()
                    .fold(0usize, |n, v| n.saturating_add(v.numeric.clone_work())),
            )
            .saturating_add(
                args.iter()
                    .map(|v| v.work_size().saturating_sub(v.numeric.work_size()))
                    .fold(0usize, usize::saturating_add),
            )
    }
    /// Numeric policy-projection work beyond existing copying.
    pub fn projection_work(&self, value: &EvmValue<D::Element>) -> usize {
        self.numeric.projection_work(value.numeric())
    }
    /// Numeric import/reduction and the caller's actual metadata traversal.
    pub fn refinement_work(&self, value: &EvmValue<D::Element>, facts: usize) -> usize {
        self.numeric
            .refinement_work(value.numeric(), facts)
            .saturating_add(value.work_size().saturating_sub(value.numeric.work_size()))
    }
}

// The default implementation alone owns NumericValue. Custom implementations
// are invoked directly above; no conversion to the default carrier occurs.
impl EvmNumericAlgebra for Domain {
    type Element = NumericValue;
    fn spec(&self) -> DomainSpec {
        Domain::spec(*self)
    }
    fn project_numeric(&self, value: &NumericValue) -> NumericValue {
        self.numeric_project(value)
    }
    fn join_numeric(&self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        self.numeric_join(left, right)
    }
    fn widen_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.numeric_widen(old, next)
    }
    fn widen_procedure_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.numeric_widen_procedure(old, next)
    }
    fn reduce_numeric(&self, value: &NumericValue) -> NumericReduction<NumericValue> {
        super::reduce::numeric(*self, value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[NumericValue]) -> NumericReduction<NumericValue> {
        self.numeric_apply(op, args)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        self.numeric_operation_work(args)
    }
    fn projection_work(&self, value: &NumericValue) -> usize {
        self.numeric_projection_work(value)
    }
    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
        self.numeric_refinement_work(value, facts)
    }
    fn observe_byte_numeric(&self, op: u8, args: &[NumericValue]) -> NumericValue {
        super::byte_projection::observe(*self, op, args)
    }
    fn byte_image_candidate(&self, value: &NumericValue) -> bool {
        self.spec().profile() == super::Profile::Product
            && value.finite_constants().is_top()
            && value.congruence().is_top()
    }
    fn byte_image_complete(&self, value: &NumericValue) -> bool {
        super::byte_projection::full_byte_image(value)
    }
    fn pack_word_numeric(&self, bytes: &[&NumericValue; 32]) -> Option<NumericValue> {
        super::byte_projection::pack_numeric(*self, bytes)
    }
    fn uniform_byte_top(&self, value: &NumericValue) -> bool {
        value == &NumericValue::unknown_byte()
    }
    fn supports_product_scalar_shortcuts(&self) -> bool {
        true
    }
}

impl<D: EvmNumericAlgebra> AlgebraRef<'_, D> {
    /// Same borrowed policy and view type, with expression construction disabled.
    pub fn without_relations(mut self) -> Self {
        let mut relations = self.spec().relations();
        relations.enabled = false;
        self.relations = Some(relations);
        self
    }
}

#[cfg(test)]
mod tests;

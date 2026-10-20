//! Injected numeric semantics with the common EVM metadata algebra.
//!
//! Policies are immutable handles; `ValueAlgebra<&D>` borrows a non-Copy policy.
//! Executable values, saved stores and summaries retain `D::Element`. Only
//! immutable-source observations cross the explicitly coarsening vocabulary.

use super::{
    Domain, DomainSpec, EvmReduction, EvmValue, NumericValue, ReductionStatus, WordElement,
    WorkExhausted, facts, symbolic,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::fmt::Debug;

#[cfg(test)]
pub(super) static OPERAND_COPIES: crate::test_instrumentation::Counter<usize> =
    crate::test_instrumentation::Counter::new(0);

pub(super) fn clone_numeric_operands<'a, N: WordElement + 'a>(
    args: impl Iterator<Item = &'a N>,
) -> Vec<N> {
    args.inspect(|_| {
        #[cfg(test)]
        OPERAND_COPIES.update(|copies| *copies += 1);
    })
    .cloned()
    .collect()
}

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
    fn into_value(self) -> EvmReduction<N> {
        EvmReduction {
            value: EvmValue::from_numeric(self.value),
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
    /// Borrow facade operands. The compatible default materializes the owned
    /// slice required by an existing custom implementation after prepayment.
    fn apply_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element>
    where
        Self::Element: 'a,
    {
        self.apply_numeric(op, &clone_numeric_operands(args))
    }
    /// Additional facade operand work, without cloning or enumeration. Override
    /// with borrowed execution when materialization is avoided.
    fn operand_work<'a>(&self, args: impl Iterator<Item = &'a EvmValue<Self::Element>>) -> usize
    where
        Self::Element: 'a,
    {
        args.fold(0usize, |work, value| {
            work.saturating_add(value.numeric().clone_work())
                .saturating_add(
                    value
                        .work_size()
                        .saturating_sub(value.numeric().work_size()),
                )
        })
    }
    /// Upper bound for numeric transfer and its dependency/reduction work.
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a;
    /// Opcode-aware quotation for already-precharged callers. The compatible
    /// default retains an existing custom policy's whole-operation envelope.
    fn operation_work_for<'a>(
        &self,
        _op: u8,
        args: impl Iterator<Item = &'a Self::Element>,
    ) -> usize
    where
        Self::Element: 'a,
    {
        self.operation_work(args)
    }
    /// Construct and release a full literal operand. Opaque constructors need
    /// an explicit bound; unknown work refuses before constructing the value.
    fn literal_quote_work(&self, _literal: U256) -> usize {
        usize::MAX
    }
    /// Complete BYTE quotes (numeric and facade operand work) are identical
    /// for all literal indices0..31 and the same retained word.
    fn byte_quotes_uniform(&self) -> bool {
        false
    }
    /// Certify that `observe_byte_numeric(BYTE, [i, word])` gives equivalent
    /// numeric results for every literal i in 0..32 under this exact policy,
    /// including hidden carrier constraints. Component shapes or equal prices
    /// alone cannot establish this law. Inspect only cached metadata here.
    /// Admission depends only on the retained word, so a preceding memory touch
    /// cannot invalidate a paid fast path. Custom policies decline by default.
    fn uniform_word_bytes(&self, _word: &Self::Element) -> bool {
        false
    }
    /// Complete fresh-word writer fee, called only after `uniform_word_bytes`.
    /// The caller guarantees literal offset zero, a Zero content root, no stored
    /// bytes/bindings or conditional context, and a nonconstant, non-ITE source
    /// expression. Cover staging/expansion and all 32 checked byte projections,
    /// including carrier copies, literals, queries and common metadata. Refresh
    /// observes BYTE once; a singleton source may also require staging's 32
    /// provenance-preserving BYTE calls. Reserve the extra allocation transfer
    /// may install before the write, beyond the current `allocations` count.
    ///
    /// Quotation inspects only cached metadata: no literal construction or work
    /// needing separate payment. MAX honestly refuses metered execution before
    /// effects; pure library calls remain caller-funded. Ordinary policies that
    /// decline the semantic certificate retain their per-index quote and work.
    fn uniform_word_write_work(
        &self,
        _value: &EvmValue<Self::Element>,
        _length: &EvmValue<Self::Element>,
        _default: &EvmValue<Self::Element>,
        _allocations: usize,
    ) -> usize {
        usize::MAX
    }
    /// Upper bound on width-sized arithmetic passes in the shared memory writer.
    /// Opaque elements retain both possible passes without inspecting their state.
    fn memory_write_passes(
        &self,
        _offset: &EvmValue<Self::Element>,
        _value: &EvmValue<Self::Element>,
        _width: usize,
    ) -> usize {
        2
    }
    /// Upper bound for policy projection beyond ordinary input cloning.
    fn projection_work(&self, value: &Self::Element) -> usize;
    /// Upper bound for guarantee import and reduction.
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize;
    /// Optional import-only reservation for staged refinement. `finite_values`
    /// bounds actual input/member payload visits; `expression_work` is the
    /// caller's structural action bound. None preserves whole-operation payment.
    fn refinement_import_work(
        &self,
        _value: &Self::Element,
        _facts: usize,
        _finite_values: usize,
        _expression_work: usize,
    ) -> Option<usize> {
        None
    }
    /// Optional byte observation shortcut; the default runs ordinary transfer.
    fn observe_byte_numeric(&self, op: u8, args: &[Self::Element]) -> Self::Element {
        self.apply_numeric(op, args).value
    }
    /// Borrowed dispatch preserves an existing custom byte override by default.
    fn observe_byte_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> Self::Element
    where
        Self::Element: 'a,
    {
        self.observe_byte_numeric(op, &clone_numeric_operands(args))
    }
    /// Cheap admission for an explicitly paid full-byte-image certificate.
    fn byte_image_candidate(&self, _value: &Self::Element) -> bool {
        false
    }
    /// Full-byte-image certificate, called only after its fixed-width payment.
    fn byte_image_complete(&self, _value: &Self::Element) -> bool {
        false
    }
    /// Optional complete fee for `pack_word_numeric`, including failed admission,
    /// predicates, allocation and construction. Inspect only cached metadata.
    /// Metered callers decline None without calling the hook and refuse MAX
    /// before doing work. Pure library calls remain caller-funded/unmetered:
    /// they may invoke the hook but cannot infer its price from an absent quote.
    /// This separate opt-in is needed because ordinary operation quotes cannot
    /// price an opaque carrier's whole-word certificate or custom cooperation.
    fn pack_word_work(&self, _bytes: &[&Self::Element; 32]) -> Option<usize> {
        None
    }
    /// Numeric replacement for the ordered 32-byte SHL/OR fold under this
    /// exact policy. Some promises equivalent numeric results, including hidden
    /// carrier constraints and required custom reductions. Matching standard
    /// observations alone cannot establish this law. None keeps ordinary apply.
    /// Metered callers first pay `pack_word_work`; pure library callers fund
    /// the work themselves. Queries on the returned element owe their own fees.
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
    fn apply_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element>
    where
        Self::Element: 'a,
    {
        D::apply_numeric_borrowed(self, op, args)
    }
    fn operand_work<'a>(&self, args: impl Iterator<Item = &'a EvmValue<Self::Element>>) -> usize
    where
        Self::Element: 'a,
    {
        D::operand_work(self, args)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        D::operation_work(self, args)
    }
    fn operation_work_for<'a>(&self, op: u8, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        D::operation_work_for(self, op, args)
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        D::literal_quote_work(self, literal)
    }
    fn byte_quotes_uniform(&self) -> bool {
        D::byte_quotes_uniform(self)
    }
    fn uniform_word_bytes(&self, word: &Self::Element) -> bool {
        D::uniform_word_bytes(self, word)
    }
    fn uniform_word_write_work(
        &self,
        value: &EvmValue<Self::Element>,
        length: &EvmValue<Self::Element>,
        default: &EvmValue<Self::Element>,
        allocations: usize,
    ) -> usize {
        D::uniform_word_write_work(self, value, length, default, allocations)
    }
    fn memory_write_passes(
        &self,
        offset: &EvmValue<Self::Element>,
        value: &EvmValue<Self::Element>,
        width: usize,
    ) -> usize {
        D::memory_write_passes(self, offset, value, width)
    }
    fn projection_work(&self, value: &Self::Element) -> usize {
        D::projection_work(self, value)
    }
    fn refinement_work(&self, value: &Self::Element, facts: usize) -> usize {
        D::refinement_work(self, value, facts)
    }
    fn refinement_import_work(
        &self,
        value: &Self::Element,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> Option<usize> {
        D::refinement_import_work(self, value, facts, finite_values, expression_work)
    }
    fn observe_byte_numeric(&self, op: u8, args: &[Self::Element]) -> Self::Element {
        D::observe_byte_numeric(self, op, args)
    }
    fn observe_byte_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> Self::Element
    where
        Self::Element: 'a,
    {
        D::observe_byte_numeric_borrowed(self, op, args)
    }
    fn byte_image_candidate(&self, value: &Self::Element) -> bool {
        D::byte_image_candidate(self, value)
    }
    fn byte_image_complete(&self, value: &Self::Element) -> bool {
        D::byte_image_complete(self, value)
    }
    fn pack_word_work(&self, bytes: &[&Self::Element; 32]) -> Option<usize> {
        D::pack_word_work(self, bytes)
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
    /// Cached-metadata fee for the optional domain-owned numeric fold replacement.
    /// Metered callers decline None and refuse MAX before invoking the hook;
    /// an absent quote supplies no price for a pure caller-funded library call.
    pub fn pack_word_work(&self, bytes: &[&D::Element; 32]) -> Option<usize> {
        self.numeric.pack_word_work(bytes)
    }
    /// Metered callers must first pay the complete domain-owned pack fee.
    /// Pure library callers fund the hook themselves, even without a quote.
    /// Queries on the returned element require their separate query fees.
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
            self.numeric
                .apply_numeric_borrowed(op, args.iter().map(EvmValue::numeric))
        };
        let mut reduced = numeric.into_value();
        reduced.value.symbolic_limit = args.iter().any(EvmValue::symbolic_limit_reached)
            && reduced.value.singleton().is_none();
        if supported {
            reduced.value.provenance = EvmValue::transfer_provenance(args);
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
        let work = self.operation_work(args);
        if work == usize::MAX || !budget.charge(work) {
            return Err(WorkExhausted);
        }
        let cancellation = budget.cancellation();
        let result =
            analysis_control::with_cancellation(&cancellation, || self.apply_detailed(op, args));
        if !budget.charge(0) {
            return Err(WorkExhausted);
        }
        Ok(result)
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
        let numeric = self
            .numeric
            .observe_byte_numeric_borrowed(op, args.iter().map(EvmValue::numeric));
        let mut out = EvmValue::from_numeric(numeric);
        out.provenance = EvmValue::transfer_provenance(args);
        out.symbolic_limit =
            args.iter().any(EvmValue::symbolic_limit_reached) && out.singleton().is_none();
        out
    }
    /// Numeric plus expression/metadata work quotation; no work is run here.
    pub fn operation_work(&self, args: &[EvmValue<D::Element>]) -> usize {
        self.operation_envelope(
            args.iter(),
            self.numeric
                .operation_work(args.iter().map(EvmValue::numeric)),
        )
    }
    /// Contextual quotation without operand copies or candidate enumeration.
    pub fn operation_work_for(&self, op: u8, args: &[EvmValue<D::Element>]) -> usize {
        self.operation_envelope(
            args.iter(),
            self.numeric
                .operation_work_for(op, args.iter().map(EvmValue::numeric)),
        )
    }
    /// Internal quote view for a fixed mix of existing and synthetic operands;
    /// callers retain the values, so quoting never clones their numeric payloads.
    pub(crate) fn operation_work_for_refs(&self, op: u8, args: &[&EvmValue<D::Element>]) -> usize {
        self.operation_envelope(
            args.iter().copied(),
            self.numeric
                .operation_work_for(op, args.iter().map(|value| value.numeric())),
        )
    }
    /// Reservation for constructing and releasing an actual literal operand.
    pub fn literal_quote_work(&self, literal: U256) -> usize {
        self.numeric.literal_quote_work(literal)
    }
    /// Domain-owned guarantee for the fixed-width BYTE batch.
    pub(crate) fn byte_quotes_uniform(&self) -> bool {
        self.numeric.byte_quotes_uniform()
    }
    /// Backend-owned bound on the memory writer's existing arithmetic passes.
    pub(crate) fn memory_write_passes(
        &self,
        offset: &EvmValue<D::Element>,
        value: &EvmValue<D::Element>,
        width: usize,
    ) -> usize {
        self.numeric.memory_write_passes(offset, value, width)
    }
    fn operation_envelope<'a>(
        &self,
        args: impl Iterator<Item = &'a EvmValue<D::Element>> + Clone,
        numeric: usize,
    ) -> usize
    where
        D::Element: 'a,
    {
        let symbolic = if self.spec().relations().enabled {
            args.clone()
                .fold(1usize, |cost, value| {
                    cost.saturating_add(value.expression().map_or(1, symbolic::ExprId::nodes))
                })
                .saturating_mul(256)
        } else {
            0
        };
        numeric
            .saturating_add(symbolic)
            .saturating_add(self.numeric.operand_work(args))
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
    /// Additional closure reservation after the actual numeric import is known.
    pub fn numeric_reduction_work(&self, value: &D::Element) -> usize {
        self.numeric.refinement_work(value, 0)
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
    fn apply_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericReduction<NumericValue> {
        self.numeric_apply_borrowed(op, args)
    }
    fn operand_work<'a>(&self, args: impl Iterator<Item = &'a EvmValue<NumericValue>>) -> usize {
        let mut metadata = 0usize;
        let copies = self.numeric_borrowed_copy_work(args.map(|value| {
            metadata = metadata.saturating_add(
                value
                    .work_size()
                    .saturating_sub(value.numeric().work_size()),
            );
            value.numeric()
        }));
        copies.saturating_add(metadata)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        self.numeric_operation_work(args)
    }
    fn operation_work_for<'a>(
        &self,
        op: u8,
        args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        self.numeric_operation_work_for(op, args)
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        // Singleton containers/Arc and disposal, four-limb mask/range/congruence
        // construction and common literal metadata fit this fixed envelope.
        64
    }
    fn byte_quotes_uniform(&self) -> bool {
        true
    }
    fn uniform_word_bytes(&self, word: &NumericValue) -> bool {
        // Only the default transfer's coarse BYTE rule has this shape law.
        // More precise policies using NumericValue opt in independently.
        self.spec().profile() == super::Profile::Product
            && word.finite_constants().is_top()
            && *word.known_bits() == super::known_bits::KnownBits::top()
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
        use super::{provenance::MAX_ORIGINS, symbolic::ExprId};

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
        let joined = candidates.min(self.capacity()).saturating_add(1);
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
        // terms here, never Domain::apply. Four old-expression copies bound
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
        self.numeric_memory_write_passes(offset, value, width)
    }
    fn projection_work(&self, value: &NumericValue) -> usize {
        self.numeric_projection_work(value)
    }
    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
        self.numeric_refinement_work(value, facts)
    }
    fn refinement_import_work(
        &self,
        value: &NumericValue,
        facts: usize,
        finite_values: usize,
        expression_work: usize,
    ) -> Option<usize> {
        Some(self.numeric_import_work(value, facts, finite_values, expression_work))
    }
    fn observe_byte_numeric(&self, op: u8, args: &[NumericValue]) -> NumericValue {
        super::byte_projection::observe(*self, op, args)
    }
    fn observe_byte_numeric_borrowed<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a NumericValue>,
    ) -> NumericValue {
        self.numeric_observe_byte_borrowed(op, args)
    }
    fn byte_image_candidate(&self, value: &NumericValue) -> bool {
        self.spec().profile() == super::Profile::Product
            && value.finite_constants().is_top()
            && value.congruence().is_top()
    }
    fn byte_image_complete(&self, value: &NumericValue) -> bool {
        super::byte_projection::full_byte_image(value)
    }
    fn pack_word_work(&self, bytes: &[&NumericValue; 32]) -> Option<usize> {
        Some(super::byte_projection::pack_work(*self, bytes))
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
    /// Pay import and then actual-input closure, or the unchanged custom whole
    /// quote. The callback preserves the caller's local/root refusal semantics.
    pub(crate) fn intersect_numeric_with_payment<E>(
        self,
        left: &D::Element,
        right: &D::Element,
        metadata_work: usize,
        mut pay: impl FnMut(usize) -> Result<(), E>,
    ) -> Result<Option<D::Element>, E> {
        let finite = left
            .finite_constants()
            .cardinality()
            .unwrap_or(0)
            .saturating_add(right.finite_constants().cardinality().unwrap_or(0));
        let Some(import) = self.numeric.refinement_import_work(left, 1, finite, 0) else {
            pay(self
                .numeric
                .refinement_work(left, 1)
                .saturating_add(metadata_work))?;
            return Ok(super::relational::intersect_numeric(left, right, self));
        };
        pay(import.saturating_add(metadata_work))?;
        let numeric = match left.constrain(&right.observation()) {
            Ok(numeric) => numeric,
            Err(
                facts::FactError::Contradiction { .. }
                | facts::FactError::ConflictingBits
                | facts::FactError::InvalidBounds
                | facts::FactError::EmptyFiniteSet,
            ) => return Ok(None),
            Err(_) => return Ok(Some(left.clone())),
        };
        if numeric
            .constants()
            .is_some_and(|values| !values.iter().any(|word| numeric.contains(*word)))
        {
            return Ok(None);
        }
        pay(self.numeric_reduction_work(&numeric))?;
        let reduced = self.numeric.reduce_numeric(&numeric);
        Ok((reduced.status != ReductionStatus::Empty)
            .then(|| self.numeric.project_numeric(&reduced.value)))
    }
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

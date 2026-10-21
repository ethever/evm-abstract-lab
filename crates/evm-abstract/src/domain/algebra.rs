//! Injected numeric semantics with the common EVM metadata algebra.
//!
//! Policies are immutable handles; `ValueAlgebra<&D>` borrows a non-Copy policy.
//! Executable values, saved stores and summaries retain `D::Element`. Only
//! immutable-source observations cross the explicitly coarsening vocabulary.

use super::{
    DomainSpec, EvmReduction, EvmValue, ReductionStatus, WordElement, WorkExhausted, facts,
    symbolic,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::fmt::Debug;

/// A result proof travels with the unchanged numeric element. An absent query
/// is distinct from a completed discovery that found no singleton.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum SingletonProof {
    #[default]
    NotQueried,
    Answer(Option<U256>),
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
    fn with_metadata(self, wrap: impl FnOnce(N) -> EvmValue<N>) -> EvmReduction<N> {
        EvmReduction {
            value: wrap(self.value),
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
/// Pure methods are caller-funded. Metered callers prepay the corresponding
/// complete operation quote. That envelope owns every query inside the quoted
/// operation, including facade metadata decisions; it does not fund independent
/// queries on operands before the operation or on its returned element later.
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
    /// Pure numeric instruction over borrowed operands, with stack top first.
    /// An implementation that copies operands includes that work in its quote.
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element>
    where
        Self::Element: 'a;
    /// Facade metadata traversal/copy work, without numeric operand cloning.
    /// Override only when the implementation needs a different metadata bound.
    fn operand_work<'a>(&self, args: impl Iterator<Item = &'a EvmValue<Self::Element>>) -> usize
    where
        Self::Element: 'a,
    {
        operand_metadata_work(args)
    }
    /// Complete opcode-aware envelope for transfer, dependency/reduction work
    /// and singleton/expression queries inside apply_detailed or observe_byte.
    /// This includes the numeric implementation's own operand copies and result
    /// queries; independent caller queries require their separate payment.
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a;
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
    /// Complete guarantee-import/reduction envelope, including queries/copies
    /// inside `constrain`, import and reduction. Queries by the caller on the
    /// returned element require their own payment.
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
    /// Optional borrowed byte observation; ordinary transfer is a sound default.
    fn observe_byte_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> Self::Element
    where
        Self::Element: 'a,
    {
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
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> NumericReduction<Self::Element>
    where
        Self::Element: 'a,
    {
        D::apply_numeric(self, op, args)
    }
    fn operand_work<'a>(&self, args: impl Iterator<Item = &'a EvmValue<Self::Element>>) -> usize
    where
        Self::Element: 'a,
    {
        D::operand_work(self, args)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Self::Element>) -> usize
    where
        Self::Element: 'a,
    {
        D::operation_work(self, op, args)
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
    fn observe_byte_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Self::Element>,
    ) -> Self::Element
    where
        Self::Element: 'a,
    {
        D::observe_byte_numeric(self, op, args)
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
    /// Pure/caller-funded, including `constrain`'s internal queries and reduction.
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
        self.join_metadata(
            left,
            right,
            self.numeric.join_numeric(left.numeric(), right.numeric()),
        )
    }
    // Join and acceleration share metadata laws, independently of the selected
    // numeric operation. In particular, identity.join drops runtime identities.
    fn join_metadata(
        &self,
        left: &EvmValue<D::Element>,
        right: &EvmValue<D::Element>,
        numeric: D::Element,
    ) -> EvmValue<D::Element> {
        EvmValue {
            numeric,
            provenance: left.provenance.join(&right.provenance),
            identity: left.identity.join(&right.identity),
            expression: left
                .expression
                .as_ref()
                .filter(|e| Some(*e) == right.expression.as_ref())
                .cloned(),
            symbolic_limit: left.symbolic_limit || right.symbolic_limit,
        }
    }
    /// Sound global acceleration; local descending reducers never call this.
    pub fn widen(
        &self,
        old: &EvmValue<D::Element>,
        next: &EvmValue<D::Element>,
    ) -> EvmValue<D::Element> {
        self.join_metadata(
            old,
            next,
            self.numeric.widen_numeric(old.numeric(), next.numeric()),
        )
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
        self.join_metadata(old, next, numeric)
    }
    /// Sound local closure; input and all metadata remain independently owned.
    pub fn reduce(&self, value: &EvmValue<D::Element>) -> EvmReduction<D::Element> {
        self.numeric
            .reduce_numeric(value.numeric())
            .with_metadata(|numeric| value.copy_metadata(numeric))
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
        self.apply_detailed_with_singleton(op, args).0
    }
    /// Reuse the actual result discovery owned by the complete operation fee.
    /// The proof is updated if expression folding installs a different literal.
    pub(crate) fn apply_detailed_with_singleton(
        &self,
        op: u8,
        args: &[EvmValue<D::Element>],
    ) -> (EvmReduction<D::Element>, SingletonProof) {
        let supported = super::concrete::arity(op) == Some(args.len());
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
                .apply_numeric(op, args.iter().map(EvmValue::numeric))
        };
        let mut reduced = numeric.with_metadata(EvmValue::from_numeric);
        let limits = self.spec().relations();
        let inherited_limit = args.iter().any(EvmValue::symbolic_limit_reached);
        // One discovery belongs to the enclosing operation envelope. Reuse it
        // for both metadata decisions instead of executing the query twice.
        let mut proof = if let Some(exact) = exact {
            SingletonProof::Answer(Some(exact))
        } else if inherited_limit || limits.enabled {
            SingletonProof::Answer(reduced.value.singleton())
        } else {
            SingletonProof::NotQueried
        };
        let singleton = match proof {
            SingletonProof::Answer(answer) => answer,
            SingletonProof::NotQueried => None,
        };
        reduced.value.symbolic_limit = inherited_limit && singleton.is_none();
        if supported {
            reduced.value.provenance = EvmValue::transfer_provenance(args);
        }
        if limits.enabled
            && singleton.is_none()
            && let Some(expressions) = args
                .iter()
                .map(EvmValue::symbolic_expression)
                .collect::<Option<Vec<_>>>()
        {
            match symbolic::ExprId::operation(op, &expressions, limits.expressions()) {
                Ok(expression) => {
                    if let Some(exact) = expression.as_constant() {
                        proof = SingletonProof::Answer(Some(exact));
                        reduced.value = reduced
                            .value
                            .with_numeric_after_singleton(D::Element::constant(exact), Some(exact));
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
        (reduced, proof)
    }
    /// Use exactly the caller's root ledger; refusal returns no partial result.
    pub fn apply_budgeted(
        &self,
        op: u8,
        args: &[EvmValue<D::Element>],
        budget: &mut crate::resource::WorkBudget,
    ) -> Result<EvmReduction<D::Element>, WorkExhausted> {
        let current = analysis_control::current_cancellation();
        if budget.exhausted() || current.is_cancelled() {
            return Err(WorkExhausted);
        }
        let work = self.operation_work(op, args);
        if work == usize::MAX || current.is_cancelled() || !budget.charge(work) {
            return Err(WorkExhausted);
        }
        let cancellation = budget.cancellation();
        let result =
            analysis_control::with_cancellation(&cancellation, || self.apply_detailed(op, args));
        if budget.exhausted() || current.is_cancelled() || !budget.charge(0) {
            return Err(WorkExhausted);
        }
        Ok(result)
    }
    /// Numeric address truncation does not assert account presence.
    pub fn address_projection(&self, value: &EvmValue<D::Element>) -> EvmValue<D::Element> {
        self.address_projection_with_singleton(&[
            value.clone(),
            EvmValue::constant(U256::MAX >> 96usize),
        ])
        .0
    }

    /// Apply already prepared `[address, low-160-bit mask]` operands. Executable
    /// callers own their copies/literal factory before reserving the complete
    /// AND operation; no second mask or operand copy is constructed here.
    pub(crate) fn address_projection_with_singleton(
        &self,
        operands: &[EvmValue<D::Element>; 2],
    ) -> (EvmValue<D::Element>, SingletonProof) {
        let (reduced, proof) = self.apply_detailed_with_singleton(opcode::AND, operands);
        let mut out = reduced.value;
        out.provenance = out.provenance.with_code_address_role();
        (out, proof)
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
            .observe_byte_numeric(op, args.iter().map(EvmValue::numeric));
        let mut out = EvmValue::from_numeric(numeric);
        out.provenance = EvmValue::transfer_provenance(args);
        out.symbolic_limit =
            args.iter().any(EvmValue::symbolic_limit_reached) && out.singleton().is_none();
        out
    }
    /// Complete opcode-aware quotation without operand copies or enumeration.
    pub fn operation_work(&self, op: u8, args: &[EvmValue<D::Element>]) -> usize {
        self.operation_envelope(
            args.iter(),
            self.numeric
                .operation_work(op, args.iter().map(EvmValue::numeric)),
        )
    }
    /// Internal quote view for a fixed mix of existing and synthetic operands;
    /// callers retain the values, so quoting never clones their numeric payloads.
    pub(crate) fn operation_work_refs(&self, op: u8, args: &[&EvmValue<D::Element>]) -> usize {
        self.operation_envelope(
            args.iter().copied(),
            self.numeric
                .operation_work(op, args.iter().map(|value| value.numeric())),
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

pub(super) fn operand_metadata_work<'a, N: WordElement + 'a>(
    args: impl Iterator<Item = &'a EvmValue<N>>,
) -> usize {
    args.fold(0usize, |work, value| {
        work.saturating_add(
            value
                .work_size()
                .saturating_sub(value.numeric().work_size()),
        )
    })
}

impl<D: EvmNumericAlgebra> AlgebraRef<'_, D> {
    /// Pay import and then actual-input closure, or the unchanged custom whole
    /// quote. The callback preserves the caller's local/root refusal semantics.
    /// The caller checks cancellation after return before installing the result.
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
            return super::relational::intersect_numeric_with(left, right, self, |_| Ok(()));
        };
        pay(import.saturating_add(metadata_work))?;
        super::relational::intersect_numeric_with(left, right, self, |numeric| {
            pay(self.numeric_reduction_work(numeric))
        })
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

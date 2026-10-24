//! Native copies are charged independently of the optional relation query.

use super::*;
use crate::{
    domain::{DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction},
    resource::clone_payment::tests::{Policy as Inner, RECEIPT, Word, private_value, reset},
};

#[derive(Debug)]
struct Algebra(Inner);
impl EvmNumericAlgebra for Algebra {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        self.0.project_numeric(value)
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        self.0.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.0.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.0.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        self.0.reduce_numeric(value)
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        self.0.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        self.0.operation_work(op, args)
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        self.0.literal_quote_work(literal)
    }
    fn projection_work(&self, value: &Word) -> usize {
        self.0.projection_work(value)
    }
    fn summary_equal_work(&self, left: &Word, right: &Word) -> usize {
        // This concrete test carrier compares its numeric fields and owned
        // exclusion buffer; its Eq fee is independent of the clone envelope.
        left.work_size()
            .saturating_add(right.work_size())
            .saturating_mul(32)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        // This backend retains its owned predicate through each internal query
        // phase; its whole-operation envelope pays those native copies.
        self.0
            .refinement_work(value, facts)
            .saturating_add(value.clone_work().saturating_mul(128))
    }
}

fn native_fixture() -> (
    ValueAlgebra<Algebra>,
    RelationLimits,
    RelationState,
    EvmValue<Word>,
) {
    let algebra = ValueAlgebra::new(Algebra(Inner(ProductAlgebra::default())));
    let limits = algebra.borrowed().spec().relations();
    let expression = ExprId::fresh().unwrap();
    let relations = bounded_relations(std::slice::from_ref(&expression), &limits);
    let value = private_value(100_000)
        .with_expression(expression)
        .with_symbolic_limit();
    (algebra, limits, relations, value)
}

#[test]
fn branch_moves_the_owned_native_answer_and_summary_pays_cache_copies() {
    let _scope = RECEIPT.scope();
    let (algebra, limits, relations, value) = native_fixture();
    let expression = value.expression().unwrap();
    let mut independent = WorkBudget::new(usize::MAX);
    reset();
    relations
        .refine_numeric_budgeted(
            expression,
            &value,
            algebra.borrowed(),
            &limits,
            &mut independent,
        )
        .unwrap();
    let query_clones = RECEIPT.get().clones;
    assert!(query_clones > 0);
    let query_fee = expression.work_size() + relations.work_size() + independent.used();
    let copy_fee = value.numeric().clone_work();
    let equality_fee = algebra
        .numeric_algebra()
        .summary_equal_work(value.numeric(), value.numeric());
    for summary in [false, true] {
        for hit_short in [false, true] {
            let mut actual = value.clone();
            reset();
            let maximum = query_fee
                + if summary {
                    3 * copy_fee + equality_fee - usize::from(hit_short)
                } else {
                    0
                };
            let mut budget = WorkBudget::new(maximum);
            let mut diagnostics = Vec::new();
            let policy = if summary {
                Policy::Summary
            } else {
                Policy::Branch {
                    diagnostics: &mut diagnostics,
                    pc: 9,
                }
            };
            let mut projection =
                Projection::new(&relations, algebra.borrowed(), &limits, &mut budget, policy);
            projection.value(&mut actual);
            assert_eq!(actual, value);
            assert_eq!(
                RECEIPT.get().clones,
                query_clones + usize::from(summary) * 2
            );
            if summary {
                // Both the original and already-refined bound reuse the same
                // native cache; every unavoidable hit copy is priced.
                projection.value(&mut actual);
                assert_eq!(
                    RECEIPT.get().clones,
                    query_clones + 2 + usize::from(!hit_short)
                );
            }
            assert_eq!(
                projection.finish(),
                if summary && hit_short {
                    Err(MachineError::Work)
                } else {
                    Ok(true)
                }
            );
            assert_eq!(actual, value);
            assert!(!actual.contains(U256::from(7)));
            assert!(actual.contains(U256::from(8)));
            assert!(diagnostics.is_empty());
        }
    }
}

#[test]
fn cache_copy_refusal_preserves_the_input_and_cancellation_identity() {
    let _scope = RECEIPT.scope();
    let (algebra, limits, relations, value) = native_fixture();
    let mut independent = WorkBudget::new(usize::MAX);
    relations
        .refine_numeric_budgeted(
            value.expression().unwrap(),
            &value,
            algebra.borrowed(),
            &limits,
            &mut independent,
        )
        .unwrap();
    let query_fee =
        value.expression().unwrap().work_size() + relations.work_size() + independent.used();
    let mut actual = value.clone();
    let mut budget = WorkBudget::new(query_fee);
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    projection.value(&mut actual);
    assert!(projection.cache.is_empty());
    assert_eq!(projection.finish(), Err(MachineError::Work));
    assert_eq!(actual, value);

    let payer = analysis_control::Cancellation::new();
    let mut actual = value;
    let mut numeric = actual.numeric().clone();
    numeric.cancel_price = Some(payer.clone());
    actual.replace_numeric_after_singleton(numeric, None);
    let mut budget = analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        &mut budget,
        Policy::Summary,
    );
    reset();
    projection.value(&mut actual);
    assert_eq!(projection.outcome, Ok(true));
    reset();
    RECEIPT.update(|receipt| receipt.armed = true);
    projection.value(&mut actual);
    assert_eq!(
        projection.finish(),
        Err(MachineError::Relations(
            crate::domain::relational::QueryReason::Cancelled
        ))
    );
    assert!(payer.is_cancelled());
    assert_eq!(RECEIPT.get().clones, 0);
}

#[cfg(feature = "imbl")]
#[test]
fn shared_store_cow_is_paid_before_any_native_clone() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, algebra) = crate::resource::clone_payment::tests::fixture("00");
    let domain = algebra.borrowed();
    let mut store = crate::world::EvmStore::new(&world, domain);
    let value = private_value(100_000).with_expression(ExprId::fresh().unwrap());
    store
        .write_checked(
            entry.address,
            &EvmValue::constant(U256::ZERO),
            &value,
            domain,
        )
        .unwrap();
    let saved = store.snapshot();
    let limits = domain.spec().relations();
    let relations = bounded_relations(&[ExprId::fresh().unwrap()], &limits);
    let mut quoted = WorkBudget::new(usize::MAX);
    store.pay_clone_with(&mut quoted).unwrap();
    let fee = quoted.used();
    for maximum in [fee - 1, usize::MAX] {
        let mut actual = store.clone();
        reset();
        let mut budget = WorkBudget::new(maximum);
        let mut diagnostics = Vec::new();
        let mut projection = Projection::new(
            &relations,
            domain,
            &limits,
            &mut budget,
            Policy::Branch {
                diagnostics: &mut diagnostics,
                pc: 0,
            },
        );
        projection.store(&mut actual);
        assert_eq!(
            projection.finish(),
            if maximum < fee {
                Err(MachineError::Work)
            } else {
                Ok(true)
            }
        );
        assert_eq!(RECEIPT.get().clones > 0, maximum >= fee);
        assert_eq!(&actual, saved.state());
        assert!(diagnostics.is_empty());
    }
}

#[test]
fn empty_summary_preserves_native_values_and_checkpoints_without_materializing_copies() {
    let _scope = RECEIPT.scope();
    let (world, entry, _, algebra) = crate::resource::clone_payment::tests::fixture("00");
    let domain = algebra.borrowed();
    let mut input = crate::analysis::transfer::initial(&world, &entry, domain).unwrap();
    input
        .store
        .write_balance(entry.address, private_value(usize::MAX));
    input.active_mut().saved_store = input.store.snapshot();
    let saved = input.active().saved_store.clone();
    let original = input.clone();
    let relations = RelationState::default();
    let limits = domain.spec().relations();
    let mut budget = WorkBudget::new(0);
    reset();
    let mut projection = Projection::new(&relations, domain, &limits, &mut budget, Policy::Summary);
    projection.payload(&mut input).unwrap();
    assert_eq!(projection.finish(), Ok(true));
    assert_eq!(input, original);
    assert!(std::ptr::eq(
        input.active().saved_store.state(),
        saved.state()
    ));
    assert_eq!(RECEIPT.get().clones, 0);
    assert_eq!(RECEIPT.get().quotes, 0);
    assert_eq!(budget.used(), 0);
}

#[derive(Debug, Default)]
struct EqProbe {
    calls: std::cell::Cell<usize>,
    quotes: std::cell::Cell<usize>,
    cancel_quote: Option<analysis_control::Cancellation>,
    cancel_eq: Option<analysis_control::Cancellation>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct EqWord {
    inner: Word,
    equality_fee: usize,
    #[serde(skip)]
    probe: std::rc::Rc<EqProbe>,
}

impl PartialEq for EqWord {
    fn eq(&self, other: &Self) -> bool {
        self.probe.calls.set(self.probe.calls.get() + 1);
        if let Some(cancel) = &self.probe.cancel_eq {
            cancel.cancel();
        }
        self.inner == other.inner && self.equality_fee == other.equality_fee
    }
}
impl Eq for EqWord {}
impl std::fmt::Display for EqWord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.inner, f)
    }
}
impl EqWord {
    fn new(inner: Word, equality_fee: usize) -> Self {
        Self {
            inner,
            equality_fee,
            probe: Default::default(),
        }
    }
}
impl WordElement for EqWord {
    fn top() -> Self {
        Self::new(Word::top(), 1_000_000)
    }
    fn constant(value: U256) -> Self {
        Self::new(Word::constant(value), 1_000_000)
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self::new(Word::from_observation(value), 1_000_000)
    }
    fn observation(&self) -> WordObservation {
        self.inner.observation()
    }
    fn contains(&self, value: U256) -> bool {
        self.inner.contains(value)
    }
    fn singleton(&self) -> Option<U256> {
        self.inner.singleton()
    }
    fn is_top(&self) -> bool {
        self.inner.is_top()
    }
    fn work_size(&self) -> usize {
        self.inner.work_size()
    }
    fn clone_work(&self) -> usize {
        self.inner.clone_work().saturating_add(16)
    }
}

#[derive(Debug)]
struct EqAlgebra;
impl EvmNumericAlgebra for EqAlgebra {
    type Element = EqWord;
    fn spec(&self) -> DomainSpec {
        ProductAlgebra::default().spec()
    }
    fn project_numeric(&self, value: &EqWord) -> EqWord {
        value.clone()
    }
    fn join_numeric(&self, _: &EqWord, _: &EqWord) -> EqWord {
        EqWord::top()
    }
    fn widen_numeric(&self, old: &EqWord, next: &EqWord) -> EqWord {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &EqWord, next: &EqWord) -> EqWord {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &EqWord) -> NumericReduction<EqWord> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _: u8,
        _: impl ExactSizeIterator<Item = &'a EqWord>,
    ) -> NumericReduction<EqWord> {
        NumericReduction::stable(EqWord::top())
    }
    fn operation_work<'a>(&self, _: u8, _: impl Iterator<Item = &'a EqWord>) -> usize {
        1_000_000
    }
    fn projection_work(&self, _: &EqWord) -> usize {
        1_000_000
    }
    fn refinement_work(&self, _: &EqWord, _: usize) -> usize {
        1_000_000
    }
    fn summary_equal_work(&self, left: &EqWord, _: &EqWord) -> usize {
        left.probe.quotes.set(left.probe.quotes.get() + 1);
        if let Some(cancel) = &left.probe.cancel_quote {
            cancel.cancel();
        }
        // A cached explicit native envelope, independent of retained geometry.
        left.equality_fee
    }
}

fn cached_eq_case(
    old_fee: usize,
    refined_fee: usize,
    already_refined: bool,
    budget: &mut WorkBudget,
    probe: std::rc::Rc<EqProbe>,
) -> (Result<bool, MachineError>, EvmValue<EqWord>) {
    let algebra = ValueAlgebra::new(EqAlgebra);
    let limits = algebra.spec().relations();
    let expression = ExprId::fresh().unwrap();
    let mut relations = RelationState::default();
    relations
        .observe(
            &expression,
            &NumericValue::constant(U256::from(11)),
            &limits,
        )
        .unwrap();
    let mut old = EqWord::new(Word::expensive(100_000), old_fee);
    old.probe = std::rc::Rc::clone(&probe);
    let mut refined = EqWord::new(
        Word::constant(U256::from(11)).with_predicate(100_000),
        refined_fee,
    );
    refined.probe = probe;
    let mut value = EvmValue::from_numeric(if already_refined {
        refined.clone()
    } else {
        old.clone()
    })
    .with_expression(expression.clone())
    .with_symbolic_limit();
    let mut projection = Projection::new(
        &relations,
        algebra.borrowed(),
        &limits,
        budget,
        Policy::Summary,
    );
    // Isolate lookup with a sound native cache pair: this source is known11,
    // old covers0..32 except7, and refined retains that private predicate.
    projection
        .cache
        .insert(expression.clone(), vec![(old, Some(refined))]);
    projection.value(&mut value);
    let result = projection.finish();
    assert_eq!(value.expression(), Some(&expression));
    assert_eq!(value.numeric().inner.private.len(), 4096);
    assert!(!value.contains(U256::from(7)));
    assert!(value.contains(U256::from(11)));
    (result, value)
}

#[test]
fn cache_native_eq_refuses_max_and_one_short_before_each_actual_predicate() {
    let _scope = RECEIPT.scope();
    const OLD: usize = 1_000_000;
    const REFINED: usize = 2_000_000;
    // old fee, refined fee, refined shortcut, budget, executed Eq, spent work.
    for (old_fee, refined_fee, shortcut, maximum, calls, spent) in [
        (usize::MAX, REFINED, false, usize::MAX, 0, 0),
        (usize::MAX, REFINED, true, usize::MAX, 0, 0),
        (OLD, REFINED, false, OLD - 1, 0, 0),
        (OLD, REFINED, true, OLD - 1, 0, 0),
        (OLD, usize::MAX, true, usize::MAX, 1, OLD),
        (OLD, REFINED, true, OLD + REFINED - 1, 1, OLD),
    ] {
        let probe = std::rc::Rc::new(EqProbe::default());
        let mut budget = WorkBudget::new(maximum);
        let (result, value) = cached_eq_case(
            old_fee,
            refined_fee,
            shortcut,
            &mut budget,
            std::rc::Rc::clone(&probe),
        );
        assert_eq!(result, Err(MachineError::Work));
        assert_eq!(probe.calls.get(), calls);
        assert_eq!(
            probe.quotes.get(),
            if shortcut && calls == 1 { 2 } else { 1 }
        );
        assert_eq!(budget.used(), spent);
        assert_eq!(value.contains(U256::from(8)), !shortcut);
    }
}

#[test]
fn finite_cache_eq_fees_keep_both_shortcuts_and_the_private_native_bound() {
    let _scope = RECEIPT.scope();
    const COPY: usize = 100_016;
    const OLD: usize = 1_000_000;
    const REFINED: usize = 2_000_000;
    for (shortcut, refined_fee, equality, calls) in [
        (false, REFINED, OLD, 1),
        (false, usize::MAX, OLD, 1), // A successful old Eq must short-circuit.
        (true, REFINED, OLD + REFINED, 2),
    ] {
        for short_copy in [false, true] {
            let probe = std::rc::Rc::new(EqProbe::default());
            let maximum = equality + COPY - usize::from(short_copy);
            let mut budget = WorkBudget::new(maximum);
            let (result, value) = cached_eq_case(
                OLD,
                refined_fee,
                shortcut,
                &mut budget,
                std::rc::Rc::clone(&probe),
            );
            assert_eq!(probe.calls.get(), calls);
            assert_eq!(probe.quotes.get(), calls);
            assert_eq!(
                result,
                if short_copy {
                    Err(MachineError::Work)
                } else {
                    Ok(true)
                }
            );
            assert_eq!(budget.used(), if short_copy { equality } else { maximum });
            assert_eq!(value.contains(U256::from(8)), short_copy && !shortcut);
        }
    }
}

#[test]
fn cancelled_cache_eq_quote_or_hook_cannot_install_the_cached_bound() {
    let _scope = RECEIPT.scope();
    for during_quote in [false, true] {
        let payer = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&payer, || WorkBudget::new(usize::MAX));
        let probe = std::rc::Rc::new(EqProbe {
            cancel_quote: during_quote.then(|| payer.clone()),
            cancel_eq: (!during_quote).then(|| payer.clone()),
            ..EqProbe::default()
        });
        let (result, value) = cached_eq_case(
            1_000_000,
            2_000_000,
            false,
            &mut budget,
            std::rc::Rc::clone(&probe),
        );
        assert_eq!(
            result,
            Err(MachineError::Relations(
                crate::domain::relational::QueryReason::Cancelled
            ))
        );
        assert_eq!(probe.calls.get(), usize::from(!during_quote));
        assert_eq!(probe.quotes.get(), 1);
        assert!(value.contains(U256::from(8)));
        assert!(payer.is_cancelled());
    }
}

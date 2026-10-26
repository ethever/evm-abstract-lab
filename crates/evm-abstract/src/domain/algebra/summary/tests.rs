use super::{
    WRAPPER_WORK, canonical, clone_value, compact, constant, equal, from_observation, is_top, join,
    join_metadata_work, join_plain, restore, scalar, singleton, top, unknown_byte, unsigned_bounds,
    unsigned_range,
};
use crate::{
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, FiniteSetAlgebra,
        NumericReduction, ProductAlgebra, SummaryConstruction, ValueAlgebra, WordElement,
        WordObservation, identity::Symbol, symbolic::ExprId,
    },
    resource::{CloneError, WorkBudget},
};
use alloy_primitives::U256;
use analysis_control::Cancellation;
use serde::Serialize;
use std::fmt;

mod coverage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Receipt {
    clones: usize,
    queries: usize,
    exports: usize,
    constructions: usize,
    projections: usize,
    joins: usize,
    widenings: usize,
    canonicals: usize,
    copy_quotes: usize,
    literal_quotes: usize,
    construction_quotes: usize,
    widened_tariff: usize,
    canonical_tariff: usize,
    equalities: usize,
    top_queries: usize,
}
static RECEIPT: crate::test_instrumentation::Counter<Receipt> =
    crate::test_instrumentation::Counter::new(Receipt {
        clones: 0,
        queries: 0,
        exports: 0,
        constructions: 0,
        projections: 0,
        joins: 0,
        widenings: 0,
        canonicals: 0,
        copy_quotes: 0,
        literal_quotes: 0,
        construction_quotes: 0,
        widened_tariff: 0,
        canonical_tariff: 0,
        equalities: 0,
        top_queries: 0,
    });

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum CancelAt {
    #[default]
    Never,
    Quote,
    Hook,
}

// A real executable exclusion predicate absent from the exported observation.
// Its small representation can lawfully have an independently expensive query
// or clone tariff after a cheap join/project constructs a new carrier.
#[derive(Debug, Serialize)]
struct Word {
    exact: Option<U256>,
    exclude_seven: bool,
    tariff: usize,
    #[serde(skip)]
    cancellation: Option<Cancellation>,
    #[serde(skip)]
    cancel_at: CancelAt,
}
impl Word {
    fn cancel(&self, at: CancelAt) {
        if self.cancel_at == at
            && let Some(token) = &self.cancellation
        {
            token.cancel();
        }
    }
    fn private(tariff: usize) -> Self {
        Self {
            exact: None,
            exclude_seven: true,
            tariff,
            cancellation: None,
            cancel_at: CancelAt::Never,
        }
    }
}
impl Clone for Word {
    fn clone(&self) -> Self {
        RECEIPT.update(|r| r.clones += 1);
        self.cancel(CancelAt::Hook);
        Self {
            exact: self.exact,
            exclude_seven: self.exclude_seven,
            tariff: self.tariff,
            cancellation: self.cancellation.clone(),
            cancel_at: self.cancel_at,
        }
    }
}
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        RECEIPT.update(|r| r.equalities += 1);
        self.cancel(CancelAt::Hook);
        (self.exact, self.exclude_seven, self.tariff)
            == (other.exact, other.exclude_seven, other.tariff)
    }
}
impl Eq for Word {}
impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:?}, excluding seven={}",
            self.exact, self.exclude_seven
        )
    }
}
impl WordElement for Word {
    fn top() -> Self {
        RECEIPT.update(|r| r.constructions += 1);
        Self {
            exclude_seven: false,
            ..Self::private(64)
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            exact: Some(word),
            ..Self::top()
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        RECEIPT.update(|r| r.exports += 1);
        Self {
            exact: observation.singleton(),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        RECEIPT.update(|r| r.exports += 1);
        self.exact
            .map_or_else(WordObservation::top, WordObservation::constant)
    }
    fn contains(&self, word: U256) -> bool {
        self.exact.is_none_or(|exact| exact == word)
            && (!self.exclude_seven || word != U256::from(7))
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|r| r.queries += 1);
        self.cancel(CancelAt::Hook);
        self.exact
    }
    fn is_top(&self) -> bool {
        RECEIPT.update(|r| r.top_queries += 1);
        self.cancel(CancelAt::Hook);
        self.exact.is_none() && !self.exclude_seven
    }
    fn work_size(&self) -> usize {
        16
    }
    fn clone_work(&self) -> usize {
        RECEIPT.update(|r| r.copy_quotes += 1);
        self.cancel(CancelAt::Quote);
        self.tariff
    }
    fn singleton_work(&self) -> usize {
        self.tariff
    }
    fn canonical_for_summary(&self) -> Self {
        RECEIPT.update(|r| r.canonicals += 1);
        self.clone()
    }
}

#[derive(Debug)]
struct Policy {
    joined_tariff: usize,
    projected_tariff: usize,
    literal_fee: usize,
    construction_fee: usize,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            joined_tariff: 64,
            projected_tariff: 64,
            literal_fee: 128,
            construction_fee: 256,
        }
    }
}
impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        DomainSpec::default()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        RECEIPT.update(|r| r.projections += 1);
        Word {
            tariff: self.projected_tariff,
            ..value.clone()
        }
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        RECEIPT.update(|r| r.joins += 1);
        Word {
            exact: left.exact.filter(|exact| Some(*exact) == right.exact),
            exclude_seven: left.exclude_seven && right.exclude_seven,
            tariff: self.joined_tariff,
            cancellation: None,
            cancel_at: CancelAt::Never,
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, _old: &Word, next: &Word) -> Word {
        RECEIPT.update(|r| r.widenings += 1);
        next.clone()
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        usize::MAX
    }
    fn projection_work(&self, _value: &Word) -> usize {
        64
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        usize::MAX
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        RECEIPT.update(|r| r.literal_quotes += 1);
        self.literal_fee
    }
    fn summary_join_work(&self, _left: &Word, right: &Word, widen: bool) -> usize {
        if widen {
            RECEIPT.update(|r| r.widened_tariff = right.tariff);
            right.tariff.saturating_add(32)
        } else {
            64
        }
    }
    fn summary_canonical_work(&self, value: &Word) -> usize {
        RECEIPT.update(|r| r.canonical_tariff = value.tariff);
        value.tariff.saturating_add(32)
    }
    fn summary_construction_work(&self, _construction: SummaryConstruction) -> usize {
        RECEIPT.update(|r| r.construction_quotes += 1);
        self.construction_fee
    }
    fn summary_equal_work(&self, left: &Word, right: &Word) -> usize {
        left.tariff.saturating_add(right.tariff).saturating_add(32)
    }
    fn summary_is_top_work(&self, value: &Word) -> usize {
        value.tariff
    }
    fn summary_compact_work(&self, value: &Word) -> usize {
        value.tariff.saturating_add(32)
    }
    fn summary_restore_work(&self, value: &Word, _previous: &Word) -> usize {
        value.tariff.saturating_add(32)
    }
    fn summary_observation_work(&self, observation: &WordObservation) -> usize {
        self.construction_fee
            .saturating_add(observation.work_size().saturating_mul(4096))
    }
}

#[test]
fn native_compact_restore_and_observation_factories_are_individually_paid() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let value = EvmValue::from_numeric(Word::private(64)).with_symbol(Symbol::Caller, Some(37));
    let mut budget = WorkBudget::new(100_000);
    let compacted = compact(&value, domain.borrowed(), &mut budget).unwrap();
    let restored = restore(&compacted, &value, domain.borrowed(), &mut budget).unwrap();
    assert!(!restored.contains(U256::from(7)));
    assert_eq!(restored.expression(), value.expression());
    assert_eq!(restored.identity(), value.identity());
    let observed = from_observation(
        &WordObservation::constant(U256::from(9)),
        domain.borrowed(),
        &mut budget,
    )
    .unwrap();
    assert_eq!(observed.singleton(), Some(U256::from(9)));
    let expensive = EvmValue::from_numeric(Word::private(usize::MAX));
    RECEIPT.set(Receipt::default());
    assert_eq!(
        compact(&expensive, domain.borrowed(), &mut WorkBudget::new(100_000)),
        Err(CloneError::Work)
    );
    assert_eq!(
        restore(
            &expensive,
            &value,
            domain.borrowed(),
            &mut WorkBudget::new(100_000)
        ),
        Err(CloneError::Work)
    );
    let refusing = ValueAlgebra::new(Policy {
        construction_fee: usize::MAX,
        ..Policy::default()
    });
    assert_eq!(
        from_observation(
            &WordObservation::top(),
            refusing.borrowed(),
            &mut WorkBudget::new(100_000)
        ),
        Err(CloneError::Work)
    );
    assert_eq!((RECEIPT.get().clones, RECEIPT.get().exports), (0, 0));
}

#[test]
fn paid_native_pipeline_preserves_private_predicates_and_metadata_laws() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy {
        joined_tariff: 512,
        projected_tariff: 1024,
        ..Policy::default()
    });
    let input = EvmValue::from_numeric(Word::private(64)).with_symbol(Symbol::Caller, Some(19));
    let mut budget = WorkBudget::new(20_000);
    let joined = join(&input, &input, domain.borrowed(), true, &mut budget).unwrap();
    assert!(!joined.contains(U256::from(7)));
    assert_eq!(joined.identity(), input.identity());
    assert_eq!(joined.expression(), input.expression());
    assert_eq!(RECEIPT.get().widened_tariff, 512);
    let scalar = scalar(&joined, domain.borrowed(), &mut budget).unwrap();
    assert!(!scalar.contains(U256::from(7)));
    assert!(scalar.identity().input().is_none());
    assert!(scalar.expression().is_none());
    assert_eq!(RECEIPT.get().canonical_tariff, 1024);
    let copies = RECEIPT.get().clones;
    let plain = join_plain(&input, &input, domain.borrowed(), false, &mut budget).unwrap();
    assert!(!plain.contains(U256::from(7)));
    assert!(plain.expression().is_none());
    assert_eq!(
        RECEIPT.get().clones,
        copies,
        "metadata erasure moves the native result"
    );
    assert_eq!(RECEIPT.get().exports, 0);
    assert_eq!(RECEIPT.get().queries, 0);
}

#[test]
fn widening_and_canonicalization_quote_actual_expensive_results_before_hooks() {
    let _scope = RECEIPT.scope();
    for expensive in [4096, usize::MAX] {
        let domain = ValueAlgebra::new(Policy {
            joined_tariff: expensive,
            projected_tariff: expensive,
            ..Policy::default()
        });
        let input = EvmValue::from_numeric(Word::private(64));
        RECEIPT.set(Receipt::default());
        let mut budget = WorkBudget::new(2000);
        assert_eq!(
            join(&input, &input, domain.borrowed(), true, &mut budget),
            Err(CloneError::Work)
        );
        let r = RECEIPT.get();
        assert_eq!((r.joins, r.widenings, r.widened_tariff), (1, 0, expensive));
        RECEIPT.set(Receipt::default());
        let mut budget = WorkBudget::new(2000);
        assert_eq!(
            scalar(&input, domain.borrowed(), &mut budget),
            Err(CloneError::Work)
        );
        let r = RECEIPT.get();
        assert_eq!(
            (r.projections, r.canonicals, r.canonical_tariff),
            (1, 0, expensive)
        );
        assert_eq!(r.exports, 0);
    }
}

#[test]
fn factories_use_selected_policy_prices_and_refuse_before_native_construction() {
    let _scope = RECEIPT.scope();
    for fee in [4096, usize::MAX] {
        let domain = ValueAlgebra::new(Policy {
            literal_fee: fee,
            construction_fee: fee,
            ..Policy::default()
        });
        RECEIPT.set(Receipt::default());
        // Use separate ledgers: a finite failed reservation exhausts its ledger.
        assert_eq!(
            constant(U256::ONE, domain.borrowed(), &mut WorkBudget::new(2000)),
            Err(CloneError::Work)
        );
        assert_eq!(
            top(domain.borrowed(), &mut WorkBudget::new(2000)),
            Err(CloneError::Work)
        );
        assert_eq!(
            unknown_byte(domain.borrowed(), &mut WorkBudget::new(2000)),
            Err(CloneError::Work)
        );
        assert_eq!(
            unsigned_range(
                U256::ZERO,
                U256::ONE,
                domain.borrowed(),
                &mut WorkBudget::new(2000)
            ),
            Err(CloneError::Work)
        );
        let r = RECEIPT.get();
        assert_eq!(
            (r.literal_quotes, r.construction_quotes, r.constructions),
            (1, 3, 0)
        );
    }
    let domain = ValueAlgebra::new(Policy::default());
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(1000);
    let literal = constant(U256::ONE, domain.borrowed(), &mut budget).unwrap();
    assert_eq!(budget.used(), 128 + WRAPPER_WORK);
    assert_eq!(singleton(&literal, &mut budget).unwrap(), Some(U256::ONE));
    let _ = top(domain.borrowed(), &mut budget).unwrap();
    assert_eq!(RECEIPT.get().constructions, 2);
}

#[test]
fn independent_copy_and_singleton_refusal_never_execute_unpaid_hooks() {
    let _scope = RECEIPT.scope();
    for (tariff, budget) in [(64, 0), (usize::MAX, 10_000)] {
        let input = EvmValue::from_numeric(Word::private(tariff));
        RECEIPT.set(Receipt::default());
        assert_eq!(
            clone_value(&input, &mut WorkBudget::new(budget)),
            Err(CloneError::Work)
        );
        assert_eq!(
            singleton(&input, &mut WorkBudget::new(budget)),
            Err(CloneError::Work)
        );
        assert_eq!((RECEIPT.get().clones, RECEIPT.get().queries), (0, 0));
    }
    let domain = ValueAlgebra::new(Policy::default());
    let input = EvmValue::from_numeric(Word::private(64));
    RECEIPT.set(Receipt::default());
    assert_eq!(
        join(
            &input,
            &input,
            domain.borrowed(),
            false,
            &mut WorkBudget::new(1)
        ),
        Err(CloneError::Work)
    );
    assert_eq!(RECEIPT.get().joins, 0);
}

#[test]
fn native_predicates_pay_before_hooks_and_keep_full_wrapper_equality() {
    let _scope = RECEIPT.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for (tariff, budget) in [(64, 0), (usize::MAX, 10_000)] {
        let input = EvmValue::from_numeric(Word::private(tariff));
        RECEIPT.set(Receipt::default());
        assert_eq!(
            equal(
                &input,
                &input,
                domain.borrowed(),
                &mut WorkBudget::new(budget)
            ),
            Err(CloneError::Work)
        );
        assert_eq!(
            is_top(&input, domain.borrowed(), &mut WorkBudget::new(budget)),
            Err(CloneError::Work)
        );
        assert_eq!(
            (RECEIPT.get().equalities, RECEIPT.get().top_queries),
            (0, 0)
        );
    }
    let input = EvmValue::from_numeric(Word::private(64));
    let identified = input.clone().with_symbol(Symbol::Caller, Some(23));
    let numeric_top = EvmValue::from_numeric(Word::top());
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(2000);
    assert!(equal(&input, &input, domain.borrowed(), &mut budget).unwrap());
    assert!(!equal(&input, &identified, domain.borrowed(), &mut budget).unwrap());
    assert!(!is_top(&input, domain.borrowed(), &mut budget).unwrap());
    assert!(is_top(&numeric_top, domain.borrowed(), &mut budget).unwrap());
    assert_eq!(
        (RECEIPT.get().equalities, RECEIPT.get().top_queries),
        (2, 2)
    );
    assert_eq!((RECEIPT.get().clones, RECEIPT.get().exports), (0, 0));
}

#[test]
fn bounds_reuse_one_paid_singleton_answer() {
    let _scope = RECEIPT.scope();
    let literal = EvmValue::from_numeric(Word {
        exact: Some(U256::from(9)),
        ..Word::private(64)
    });
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(64);
    assert_eq!(
        unsigned_bounds(&literal, &mut budget).unwrap(),
        (U256::from(9), U256::from(9))
    );
    assert_eq!((RECEIPT.get().queries, budget.used()), (1, 64));
    RECEIPT.set(Receipt::default());
    let input = EvmValue::from_numeric(Word::private(64));
    assert_eq!(
        unsigned_bounds(&input, &mut WorkBudget::new(1000)).unwrap(),
        (U256::ZERO, U256::MAX)
    );
    assert_eq!(RECEIPT.get().queries, 1);
    assert_eq!(RECEIPT.get().exports, 0);
}

#[test]
fn current_and_captured_cancellation_refuse_quotes_and_discard_hook_results() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        for at in [CancelAt::Never, CancelAt::Quote, CancelAt::Hook] {
            let captured = Cancellation::new();
            let current = Cancellation::new();
            let token = if cancel_captured { &captured } else { &current };
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(1000));
            let input = EvmValue::from_numeric(Word {
                cancellation: Some(token.clone()),
                cancel_at: at,
                ..Word::private(64)
            });
            if at == CancelAt::Never {
                token.cancel();
            }
            RECEIPT.set(Receipt::default());
            let result =
                analysis_control::with_cancellation(&current, || clone_value(&input, &mut budget));
            assert_eq!(result, Err(CloneError::Cancelled));
            assert_eq!(RECEIPT.get().clones, usize::from(at == CancelAt::Hook));
            if at != CancelAt::Hook {
                assert_eq!(budget.used(), 0);
            }
        }
        for operation in 0..4 {
            let captured = Cancellation::new();
            let current = Cancellation::new();
            let token = if cancel_captured { &captured } else { &current };
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(1000));
            let input = EvmValue::from_numeric(Word {
                cancellation: Some(token.clone()),
                cancel_at: CancelAt::Hook,
                ..Word::private(64)
            });
            let domain = ValueAlgebra::new(Policy::default());
            RECEIPT.set(Receipt::default());
            let result = analysis_control::with_cancellation(&current, || match operation {
                0 => singleton(&input, &mut budget).map(|_| ()),
                1 => scalar(&input, domain.borrowed(), &mut budget).map(|_| ()),
                2 => equal(&input, &input, domain.borrowed(), &mut budget).map(|_| ()),
                3 => is_top(&input, domain.borrowed(), &mut budget).map(|_| ()),
                _ => unreachable!("four paid hook boundaries"),
            });
            assert_eq!(result, Err(CloneError::Cancelled));
            assert_eq!(RECEIPT.get().queries, usize::from(operation == 0));
            assert_eq!(RECEIPT.get().projections, usize::from(operation == 1));
            assert_eq!(RECEIPT.get().equalities, usize::from(operation == 2));
            assert_eq!(RECEIPT.get().top_queries, usize::from(operation == 3));
            assert_eq!(RECEIPT.get().canonicals, 0);
            assert_eq!(RECEIPT.get().canonical_tariff, 0);
        }
    }
}

#[test]
fn builtin_native_quotes_cover_retained_results_and_do_not_price_configured_future_sets() {
    let product = ProductAlgebra::default();
    let finite = FiniteSetAlgebra::default();
    let left = crate::domain::NumericValue::constant(U256::ONE);
    let right = crate::domain::NumericValue::constant(U256::from(2));
    let joined = product.join_numeric(&left, &right);
    assert!(product.summary_join_work(&left, &right, false) >= joined.work_size());
    assert!(product.summary_join_work(&left, &joined, true) >= joined.work_size());
    assert!(
        product.summary_canonical_work(&joined) >= joined.singleton_work() + joined.clone_work()
    );
    let left = FiniteConstantSet::constant(U256::ONE);
    let right = FiniteConstantSet::constant(U256::from(2));
    let joined = finite.join_numeric(&left, &right);
    assert!(finite.summary_join_work(&left, &right, false) >= WordElement::work_size(&joined));
    let policy = &finite;
    assert_eq!(
        EvmNumericAlgebra::summary_canonical_work(&policy, &joined),
        finite.summary_canonical_work(&joined)
    );
    assert_eq!(
        EvmNumericAlgebra::summary_construction_work(&policy, SummaryConstruction::Top),
        finite.summary_construction_work(SummaryConstruction::Top)
    );
    let large_spec = DomainSpec::new(
        std::num::NonZeroUsize::new(usize::MAX).unwrap(),
        std::num::NonZeroUsize::new(4).unwrap(),
        std::num::NonZeroUsize::new(usize::MAX).unwrap(),
    );
    let large = FiniteSetAlgebra::from_spec(large_spec);
    assert_eq!(
        large.summary_join_work(&left, &right, false),
        finite.summary_join_work(&left, &right, false)
    );
    let larger_input =
        FiniteConstantSet::try_from_values((0u64..32).map(U256::from).collect()).unwrap();
    assert!(
        large.summary_join_work(&left, &larger_input, false)
            > large.summary_join_work(&left, &right, false)
    );
    // Expression structure has its own metadata price, independently of N.
    let plain = EvmValue::from_numeric(Word::private(64));
    let expression = ExprId::constant(U256::ONE);
    let with_expression = plain.clone().with_expression(expression.clone());
    assert!(
        join_metadata_work(&with_expression, &with_expression)
            >= expression.comparison_work(&expression)
    );
}

#[test]
fn opaque_existing_algebras_decline_new_summary_hooks_by_default() {
    use crate::resource::clone_payment::tests::{
        Policy as Opaque, RECEIPT as OPAQUE_RECEIPT, Word as OpaqueWord,
    };
    let _scope = OPAQUE_RECEIPT.scope();
    let policy = Opaque(ProductAlgebra::default());
    let value = OpaqueWord::expensive(64);
    let borrowed = &policy;
    assert_eq!(
        EvmNumericAlgebra::summary_compact_work(&borrowed, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_restore_work(&borrowed, &value, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_observation_work(&borrowed, &WordObservation::top()),
        usize::MAX
    );
    assert_eq!(
        borrowed.summary_join_work(&value, &value, false),
        usize::MAX
    );
    assert_eq!(borrowed.summary_join_work(&value, &value, true), usize::MAX);
    assert_eq!(borrowed.summary_canonical_work(&value), usize::MAX);
    assert_eq!(
        borrowed.summary_construction_work(SummaryConstruction::Top),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_equal_work(&borrowed, &value, &value),
        usize::MAX
    );
    assert_eq!(
        EvmNumericAlgebra::summary_is_top_work(&borrowed, &value),
        usize::MAX
    );
    let domain = ValueAlgebra::new(policy);
    let value = EvmValue::from_numeric(value);
    assert_eq!(
        join(
            &value,
            &value,
            domain.borrowed(),
            false,
            &mut WorkBudget::new(10_000)
        ),
        Err(CloneError::Work)
    );
    assert_eq!(
        canonical(&value, domain.borrowed(), &mut WorkBudget::new(10_000)),
        Err(CloneError::Work)
    );
    assert_eq!(
        top(domain.borrowed(), &mut WorkBudget::new(10_000)),
        Err(CloneError::Work)
    );
    assert_eq!(
        (OPAQUE_RECEIPT.get().clones, OPAQUE_RECEIPT.get().queries),
        (0, 0)
    );
}

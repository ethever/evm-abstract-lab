//! Actual Reader entry points retain semantics and refuse native hooks in order.

use super::{Context, Facts, Reader, read_word_with_payment, slot, slot_with_payment};
use crate::{
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue, ProductAlgebra,
        SummaryConstruction, ValueAlgebra, WordElement, WordObservation,
        symbolic::{ByteStateId, ByteView, ExprId, ExprLimits, SlotStateId, StateLimits},
    },
    resource::{CloneError, SummaryError, WorkBudget},
    world::bytes::EvmByteArray,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use serde::Serialize;
use std::{fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    clones: usize,
    bad_clones: usize,
    queries: usize,
    bad_queries: usize,
    operations: usize,
    tops: usize,
    literals: usize,
    packs: usize,
    equalities: usize,
}
static COUNTS: crate::test_instrumentation::Counter<Counts> =
    crate::test_instrumentation::Counter::new(Counts {
        clones: 0,
        bad_clones: 0,
        queries: 0,
        bad_queries: 0,
        operations: 0,
        tops: 0,
        literals: 0,
        packs: 0,
        equalities: 0,
    });

#[derive(Debug, Serialize)]
struct Word {
    exact: Option<U256>,
    odd: bool,
    copy_fee: usize,
    query_fee: usize,
    #[serde(skip)]
    cancel: Option<analysis_control::Cancellation>,
}
impl Word {
    fn private() -> Self {
        Self {
            exact: None,
            odd: true,
            copy_fee: 64,
            query_fee: 64,
            cancel: None,
        }
    }
    fn plain(exact: Option<U256>) -> Self {
        Self {
            exact,
            odd: false,
            ..Self::private()
        }
    }
}
impl Clone for Word {
    fn clone(&self) -> Self {
        COUNTS.update(|n| {
            n.clones += 1;
            n.bad_clones += usize::from(self.copy_fee == usize::MAX);
        });
        if let Some(token) = &self.cancel {
            token.cancel();
        }
        Self {
            exact: self.exact,
            odd: self.odd,
            copy_fee: self.copy_fee,
            query_fee: self.query_fee,
            cancel: self.cancel.clone(),
        }
    }
}
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        COUNTS.update(|n| n.equalities += 1);
        (self.exact, self.odd, self.copy_fee, self.query_fee)
            == (other.exact, other.odd, other.copy_fee, other.query_fee)
    }
}
impl Eq for Word {}
impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}/odd={}", self.exact, self.odd)
    }
}
impl WordElement for Word {
    fn top() -> Self {
        COUNTS.update(|n| n.tops += 1);
        Self::plain(None)
    }
    fn constant(word: U256) -> Self {
        COUNTS.update(|n| n.literals += 1);
        Self::plain(Some(word))
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self::plain(value.singleton())
    }
    fn observation(&self) -> WordObservation {
        self.exact
            .map_or_else(WordObservation::top, WordObservation::constant)
    }
    fn singleton(&self) -> Option<U256> {
        COUNTS.update(|n| {
            n.queries += 1;
            n.bad_queries += usize::from(self.query_fee == usize::MAX);
        });
        self.exact
    }
    fn contains(&self, word: U256) -> bool {
        self.exact.is_none_or(|exact| exact == word) && (!self.odd || word.bit(0))
    }
    fn is_top(&self) -> bool {
        self.exact.is_none() && !self.odd
    }
    fn work_size(&self) -> usize {
        16
    }
    fn clone_work(&self) -> usize {
        self.copy_fee
    }
    fn singleton_work(&self) -> usize {
        self.query_fee
    }
}
#[derive(Debug)]
struct Policy {
    result_copy: usize,
    pack_query: usize,
    literal: usize,
    equality: usize,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            result_copy: 64,
            pack_query: 64,
            literal: 64,
            equality: 64,
        }
    }
}
impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        let spec = DomainSpec::default();
        let mut relations = spec.relations();
        relations.enabled = false;
        spec.with_relations(relations)
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Word {
            exact: left.exact.filter(|word| Some(*word) == right.exact),
            odd: left.odd && right.odd,
            copy_fee: self.result_copy,
            query_fee: 64,
            cancel: None,
        }
    }
    fn widen_numeric(&self, old: &Word, next: &Word) -> Word {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        COUNTS.update(|n| n.operations += 1);
        let args = args.collect::<Vec<_>>();
        let exact = match (op, args.as_slice()) {
            (opcode::AND, [a, b]) => a.exact.zip(b.exact).map(|(a, b)| a & b),
            (opcode::BYTE, [i, word]) => i.exact.zip(word.exact).map(|(i, word)| {
                if i < U256::from(32) {
                    (word >> ((31 - usize::try_from(i).unwrap()) * 8)) & U256::from(255)
                } else {
                    U256::ZERO
                }
            }),
            _ => None,
        };
        let odd = matches!((op,args.as_slice()),(opcode::AND,[a,b])if a.odd&&b.exact.is_some_and(|n|n.bit(0)));
        NumericReduction::stable(Word {
            exact,
            odd,
            copy_fee: self.result_copy,
            query_fee: 64,
            cancel: None,
        })
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        8192
    }
    fn projection_work(&self, _value: &Word) -> usize {
        32
    }
    fn refinement_work(&self, _value: &Word, _facts: usize) -> usize {
        usize::MAX
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        self.literal
    }
    fn summary_construction_work(&self, _kind: SummaryConstruction) -> usize {
        128
    }
    fn summary_join_work(&self, _left: &Word, _right: &Word, _widen: bool) -> usize {
        128
    }
    fn summary_equal_work(&self, _left: &Word, _right: &Word) -> usize {
        self.equality
    }
    fn summary_canonical_work(&self, value: &Word) -> usize {
        // The default Word canonicalizer performs exactly one native Clone.
        value.clone_work().saturating_add(value.work_size())
    }
    fn summary_is_top_work(&self, _value: &Word) -> usize {
        64
    }
    fn pack_word_work(&self, bytes: &[&Word; 32]) -> Option<usize> {
        bytes
            .iter()
            .all(|value| value.exact == Some(U256::ZERO))
            .then_some(128)
    }
    fn pack_word_numeric(&self, bytes: &[&Word; 32]) -> Option<Word> {
        if !bytes.iter().all(|value| value.exact == Some(U256::ZERO)) {
            return None;
        }
        COUNTS.update(|n| n.packs += 1);
        Some(Word {
            query_fee: self.pack_query,
            ..Word::plain(Some(U256::ZERO))
        })
    }
}

#[test]
fn reader_byte_word_copy_and_slot_paid_results_match_the_shared_pure_evaluator() {
    let domain = ValueAlgebra::new(ProductAlgebra::default());
    let key = ExprId::fresh().unwrap();
    let at = ExprId::fresh().unwrap();
    let facts = Facts::from([(
        key.clone(),
        EvmValue::from_numeric(
            NumericValue::unsigned_range(U256::from(257), U256::from(511)).unwrap(),
        ),
    )]);
    let root = ByteStateId::zero()
        .write_word(at.clone(), key.clone(), StateLimits::default())
        .unwrap();
    let copied = root
        .copy(
            ExprId::constant(U256::from(64)),
            ByteView::new(
                root.clone(),
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::from(32)),
            ),
            ExprId::constant(U256::from(32)),
            StateLimits::default(),
        )
        .unwrap();
    let mut budget = WorkBudget::new(usize::MAX);
    let mut paid = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    let mut pure = Reader::new(&facts, domain.borrowed());
    for root in [&root, &copied] {
        for index in [0, 31, 64, 95] {
            let at = ExprId::constant(U256::from(index));
            assert_eq!(
                paid.read_with_payment(root, &at, &mut budget).unwrap(),
                pure.read(root, &at)
            );
        }
    }
    let exact = ByteStateId::exact(Arc::from([0x71u8; 32]));
    let zero = ExprId::constant(U256::ZERO);
    assert_eq!(
        paid.word_with_payment(&exact, &zero, &mut budget).unwrap(),
        pure.word(&exact, &zero)
    );
    let slot_root = SlotStateId::zero()
        .store(zero.clone(), key, StateLimits::default())
        .unwrap();
    let context = Context::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    assert_eq!(
        slot_with_payment(&slot_root, &zero, &context, domain.borrowed(), &mut budget).unwrap(),
        slot(
            &slot_root,
            &zero,
            &Context::new(&facts, None, domain.borrowed()),
            domain.borrowed()
        )
    );
    let array = EvmByteArray::<NumericValue>::exact(&[0x31; 40]);
    let offset = EvmValue::constant(U256::ZERO);
    assert_eq!(
        read_word_with_payment(&array, &offset, domain.borrowed(), &mut budget).unwrap(),
        array.read_word_checked(&offset, domain.borrowed()).unwrap()
    );
}

#[test]
fn reader_private_predicate_survives_and_high_result_copy_refuses_before_clone() {
    let _scope = COUNTS.scope();
    let key = ExprId::fresh().unwrap();
    let at = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), EvmValue::from_numeric(Word::private()))]);
    let zero = ExprId::constant(U256::ZERO);
    let direct = ByteStateId::zero()
        .write_word(zero.clone(), key.clone(), StateLimits::default())
        .unwrap();
    let domain = ValueAlgebra::new(Policy::default());
    let mut budget = WorkBudget::new(200_000);
    let mut reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    let value = reader
        .word_with_payment(&direct, &zero, &mut budget)
        .unwrap();
    assert!(!value.contains(U256::from(2)));
    assert!(value.contains(U256::from(3)));
    let ambiguous = ByteStateId::zero()
        .write_word(at, key, StateLimits::default())
        .unwrap();
    let domain = ValueAlgebra::new(Policy {
        result_copy: usize::MAX,
        ..Policy::default()
    });
    let mut budget = WorkBudget::new(200_000);
    let mut reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    COUNTS.set(Counts::default());
    assert_eq!(
        reader.read_with_payment(&ambiguous, &zero, &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(COUNTS.get().operations, 1);
    assert_eq!(COUNTS.get().bad_clones, 0);
    assert_eq!(COUNTS.get().tops, 1);
    assert_eq!(reader.cache_stats(), (0, 0));
}

#[test]
fn retained_scalar_ite_preserves_private_membership_and_skips_unselected_native_values() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    for (condition_value, selected_word, expected) in [
        (Some(true), 2u8, None),
        (Some(true), 3, Some(3u8)),
        (Some(false), 2, None),
        (Some(false), 3, Some(3)),
        (None, 0, None),
    ] {
        let condition = ExprId::fresh().unwrap();
        let then_value = ExprId::fresh().unwrap();
        let else_value = ExprId::fresh().unwrap();
        let expression = ExprId::ite(
            condition.clone(),
            then_value.clone(),
            else_value.clone(),
            ExprLimits::default(),
        )
        .unwrap();
        let branch_value = |nonzero| {
            if condition_value == Some(nonzero) {
                Word::plain(Some(U256::from(selected_word)))
            } else {
                Word {
                    copy_fee: usize::MAX,
                    query_fee: usize::MAX,
                    ..Word::plain(None)
                }
            }
        };
        let facts = Facts::from([
            (
                condition,
                EvmValue::from_numeric(Word::plain(
                    condition_value.map(|nonzero| U256::from(u8::from(nonzero))),
                )),
            ),
            (then_value, EvmValue::from_numeric(branch_value(true))),
            (else_value, EvmValue::from_numeric(branch_value(false))),
            (expression.clone(), EvmValue::from_numeric(Word::private())),
        ]);
        let mut budget = WorkBudget::new(200_000);
        let context = Context::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
        COUNTS.set(Counts::default());
        let value =
            super::scalar_in_with_payment(&expression, &context, domain.borrowed(), &mut budget)
                .unwrap();
        assert_eq!(value.numeric().exact, expected.map(U256::from));
        assert_eq!(value.numeric().odd, expected.is_none());
        assert!(!value.contains(U256::from(2)));
        assert!(value.contains(U256::from(3)));
        assert_eq!((COUNTS.get().bad_clones, COUNTS.get().bad_queries), (0, 0));
        assert!(!budget.exhausted());
    }
}

#[test]
fn pack_result_query_max_stops_without_query_or_scalar_fallback() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy {
        pack_query: usize::MAX,
        ..Policy::default()
    });
    let facts = Facts::new();
    let mut budget = WorkBudget::new(200_000);
    let mut reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    let exact = ByteStateId::exact(Arc::from([0u8; 32]));
    let zero = ExprId::constant(U256::ZERO);
    COUNTS.set(Counts::default());
    assert_eq!(
        reader.word_with_payment(&exact, &zero, &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(
        (
            COUNTS.get().packs,
            COUNTS.get().bad_queries,
            COUNTS.get().operations
        ),
        (1, 0, 0)
    );
}

#[test]
fn warm_reader_cache_copy_is_paid_on_the_retained_native_result() {
    let _scope = COUNTS.scope();
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), EvmValue::from_numeric(Word::private()))]);
    let domain = ValueAlgebra::new(Policy {
        result_copy: 4096,
        ..Policy::default()
    });
    let mut setup = WorkBudget::new(1_000_000);
    let mut reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut setup).unwrap();
    reader
        .word_byte_union_paid(&key, &mut super::payment::ReadPayment::new(&mut setup))
        .unwrap();
    assert_eq!(reader.cache_stats().0, 1);
    COUNTS.set(Counts::default());
    // Exact lookup prefix: three shell units, one lookup unit and
    // two units for the pointer-equal expression key.
    // There is no remaining native clone allowance on this ledger.
    let mut budget = WorkBudget::new(3);
    let result =
        reader.word_byte_union_paid(&key, &mut super::payment::ReadPayment::new(&mut budget));
    assert_eq!(result, Err(SummaryError::Budget(CloneError::Work)));
    assert_eq!(COUNTS.get(), Counts::default());
    assert_eq!(reader.cache_stats().0, 1);
}

#[test]
fn zero_budget_and_selected_literal_max_construct_no_native_sentinel() {
    let _scope = COUNTS.scope();
    let facts = Facts::new();
    let domain = ValueAlgebra::new(Policy::default());
    COUNTS.set(Counts::default());
    assert!(matches!(
        Reader::with_payment(&facts, None, domain.borrowed(), &mut WorkBudget::new(0)),
        Err(SummaryError::Budget(CloneError::Work))
    ));
    assert_eq!(COUNTS.get(), Counts::default());
    let domain = ValueAlgebra::new(Policy {
        literal: usize::MAX,
        ..Policy::default()
    });
    let mut budget = WorkBudget::new(1000);
    let mut reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    assert_eq!(
        reader.read_with_payment(
            &ByteStateId::zero(),
            &ExprId::constant(U256::ZERO),
            &mut budget
        ),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(COUNTS.get(), Counts::default());
}

#[test]
fn cancellation_after_real_fact_copy_returns_typed_error_without_top() {
    let _scope = COUNTS.scope();
    for captured_owner in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let token = if captured_owner { &captured } else { &current };
        let key = ExprId::fresh().unwrap();
        let zero = ExprId::constant(U256::ZERO);
        let facts = Facts::from([(
            key.clone(),
            EvmValue::from_numeric(Word {
                cancel: Some(token.clone()),
                ..Word::private()
            }),
        )]);
        let root = ByteStateId::zero()
            .write_word(zero.clone(), key, StateLimits::default())
            .unwrap();
        let domain = ValueAlgebra::new(Policy::default());
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(100_000));
        let mut reader =
            Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
        COUNTS.set(Counts::default());
        let result = analysis_control::with_cancellation(&current, || {
            reader.word_with_payment(&root, &zero, &mut budget)
        });
        assert_eq!(result, Err(SummaryError::Budget(CloneError::Cancelled)));
        assert_eq!((COUNTS.get().clones, COUNTS.get().tops), (1, 0));
    }
}

fn scalar_and(key: &ExprId) -> ExprId {
    ExprId::operation(
        opcode::AND,
        &[key.clone(), ExprId::constant(U256::from(255))],
        ExprLimits::default(),
    )
    .unwrap()
}

#[test]
fn scalar_operation_memo_keeps_private_native_and_source_identity_and_declines_unavailable_equality()
 {
    let _scope = COUNTS.scope();
    let a = ExprId::fresh().unwrap();
    let b = ExprId::fresh().unwrap();
    let c = ExprId::fresh().unwrap();
    let facts = Facts::from([
        (
            a.clone(),
            EvmValue::from_numeric(Word::private()).with_identity(17, 1),
        ),
        (
            b.clone(),
            EvmValue::from_numeric(Word::plain(None)).with_identity(17, 1),
        ),
        (
            c.clone(),
            EvmValue::from_numeric(Word::private()).with_identity(17, 2),
        ),
    ]);
    let domain = ValueAlgebra::new(Policy::default());
    let mut budget = WorkBudget::new(1_000_000);
    let reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    COUNTS.set(Counts::default());
    let first = reader
        .scalar_paid(
            &scalar_and(&a),
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    let second = reader
        .scalar_paid(
            &scalar_and(&b),
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    reader
        .scalar_paid(
            &scalar_and(&c),
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    assert_eq!(COUNTS.get().operations, 3);
    assert_eq!(
        first.numeric().observation(),
        second.numeric().observation()
    );
    assert!(first.numeric().odd && !second.numeric().odd);
    reader
        .scalar_paid(
            &scalar_and(&a),
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    assert_eq!(COUNTS.get().operations, 3);
    assert!(reader.operations.borrow().entries <= super::WORD_CACHE_ENTRIES);
    assert!(reader.operations.borrow().work <= super::WORD_CACHE_WORK);

    let domain = ValueAlgebra::new(Policy {
        equality: usize::MAX,
        ..Policy::default()
    });
    let mut budget = WorkBudget::new(1_000_000);
    let reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    COUNTS.set(Counts::default());
    for _ in 0..2 {
        let value = reader
            .scalar_paid(
                &scalar_and(&a),
                &mut super::payment::ReadPayment::new(&mut budget),
            )
            .unwrap();
        assert!(value.numeric().odd);
    }
    assert_eq!(COUNTS.get().operations, 2);
    assert_eq!(COUNTS.get().equalities, 0);
    assert!(!budget.exhausted());
}

#[test]
fn scalar_operation_memo_hit_copy_refuses_before_clone_and_never_rescues_a_stopped_ledger() {
    let _scope = COUNTS.scope();
    let key = ExprId::fresh().unwrap();
    let facts = Facts::from([(key.clone(), EvmValue::from_numeric(Word::private()))]);
    let domain = ValueAlgebra::new(Policy {
        result_copy: 4096,
        ..Policy::default()
    });
    let mut setup = WorkBudget::new(1_000_000);
    let reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut setup).unwrap();
    let expression = scalar_and(&key);
    reader
        .scalar_paid(
            &expression,
            &mut super::payment::ReadPayment::new(&mut setup),
        )
        .unwrap();
    let before = setup.used();
    COUNTS.set(Counts::default());
    reader
        .scalar_paid(
            &expression,
            &mut super::payment::ReadPayment::new(&mut setup),
        )
        .unwrap();
    let fee = setup.used() - before;
    let complete = COUNTS.get();
    assert_eq!(complete.operations, 0);
    let mut denied = WorkBudget::new(fee - 1);
    COUNTS.set(Counts::default());
    assert_eq!(
        reader.scalar_paid(
            &expression,
            &mut super::payment::ReadPayment::new(&mut denied)
        ),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(COUNTS.get().operations, 0);
    assert_eq!(COUNTS.get().clones + 1, complete.clones);
    assert_eq!(COUNTS.get().bad_clones, 0);
    COUNTS.set(Counts::default());
    assert_eq!(
        reader.scalar_paid(
            &expression,
            &mut super::payment::ReadPayment::new(&mut denied)
        ),
        Err(SummaryError::Budget(CloneError::Work))
    );
    assert_eq!(COUNTS.get(), Counts::default());
    for captured in [false, true] {
        let token = analysis_control::Cancellation::new();
        let mut budget = if captured {
            analysis_control::with_cancellation(&token, || WorkBudget::new(1_000_000))
        } else {
            WorkBudget::new(1_000_000)
        };
        token.cancel();
        COUNTS.set(Counts::default());
        let mut run = || {
            reader.scalar_paid(
                &expression,
                &mut super::payment::ReadPayment::new(&mut budget),
            )
        };
        let result = if captured {
            run()
        } else {
            analysis_control::with_cancellation(&token, run)
        };
        assert_eq!(result, Err(SummaryError::Budget(CloneError::Cancelled)));
        assert_eq!(COUNTS.get(), Counts::default());
    }
}

#[test]
fn actual_guarded_reader_resolves_private_operands_before_shared_scalar_operation_reuse() {
    use crate::world::bytes::conditional::ConditionalFacts;
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let guard = ExprId::fresh().unwrap();
    let key = ExprId::fresh().unwrap();
    let common = Facts::new();
    let yes = Facts::from([(key.clone(), EvmValue::from_numeric(Word::private()))]);
    let no = Facts::from([(key.clone(), EvmValue::from_numeric(Word::plain(None)))]);
    let empty = ConditionalFacts::default();
    let conditional = ConditionalFacts::join(
        (&yes, &empty),
        (&no, &empty),
        &common,
        guard.clone(),
        ExprLimits::default(),
    )
    .unwrap();
    let length = ExprId::operation(
        opcode::AND,
        &[key, ExprId::constant(U256::ONE)],
        ExprLimits::default(),
    )
    .unwrap();
    let copy = ByteStateId::zero()
        .copy(
            ExprId::constant(U256::ZERO),
            ByteView::new(
                ByteStateId::exact(Arc::from([7u8])),
                ExprId::constant(U256::ZERO),
                ExprId::constant(U256::ONE),
            ),
            length,
            StateLimits::default(),
        )
        .unwrap();
    let later = copy
        .write_byte(
            ExprId::constant(U256::from(99)),
            ExprId::constant(U256::ONE),
            StateLimits::default(),
        )
        .unwrap();
    let root = copy
        .join_guarded(&later, guard, StateLimits::default())
        .unwrap();
    let mut budget = WorkBudget::new(1_000_000);
    let mut reader =
        Reader::with_payment(&common, Some(&conditional), domain.borrowed(), &mut budget).unwrap();
    COUNTS.set(Counts::default());
    let first = reader
        .read_with_payment(&root, &ExprId::constant(U256::ZERO), &mut budget)
        .unwrap();
    assert!(first.contains(U256::ZERO) && first.contains(U256::from(7)));
    assert_eq!(COUNTS.get().operations, 2);
    let second = reader
        .read_with_payment(&root, &ExprId::constant(U256::ZERO), &mut budget)
        .unwrap();
    assert!(second.contains(U256::ZERO) && second.contains(U256::from(7)));
    assert_eq!(COUNTS.get().operations, 2);
}

#[test]
fn scalar_memo_correlated_xor_cannot_be_reused_for_independent_runtime_definitions() {
    let _scope = COUNTS.scope();
    let keys: Vec<_> = (0..4).map(|_| ExprId::fresh().unwrap()).collect();
    let facts = Facts::from([
        (
            keys[0].clone(),
            EvmValue::from_numeric(Word::plain(None)).with_identity(71, 1),
        ),
        (
            keys[1].clone(),
            EvmValue::from_numeric(Word::plain(None)).with_identity(71, 1),
        ),
        (
            keys[2].clone(),
            EvmValue::from_numeric(Word::plain(None)).with_identity(71, 2),
        ),
        (
            keys[3].clone(),
            EvmValue::from_numeric(Word::plain(None)).with_identity(71, 3),
        ),
    ]);
    let domain = ValueAlgebra::new(Policy::default());
    let mut budget = WorkBudget::new(1_000_000);
    let reader = Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
    let correlated = ExprId::operation(opcode::XOR, &keys[..2], ExprLimits::default()).unwrap();
    let independent = ExprId::operation(opcode::XOR, &keys[2..], ExprLimits::default()).unwrap();
    let first = reader
        .scalar_paid(
            &correlated,
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    assert_eq!(first.numeric().exact, Some(U256::ZERO));
    let second = reader
        .scalar_paid(
            &independent,
            &mut super::payment::ReadPayment::new(&mut budget),
        )
        .unwrap();
    assert_eq!(second.numeric().exact, None);
    assert!(second.contains(U256::ONE));
    let expected = super::scalar_in_paid(
        &independent,
        &reader.context,
        reader.domain,
        &mut super::payment::ReadPayment::unpaid(),
    )
    .unwrap();
    assert_eq!(second, expected);
    let cached_unpaid = reader
        .scalar_paid(&independent, &mut super::payment::ReadPayment::unpaid())
        .unwrap();
    assert_eq!(cached_unpaid, expected);
}

#[test]
fn unpaid_operation_frontend_uses_original_context_without_memo_copies() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy {
        result_copy: 1_000_000,
        ..Policy::default()
    });
    let facts = Facts::new();
    let conditional = crate::world::bytes::conditional::ConditionalFacts::default();
    let offset = EvmValue::<Word>::constant(U256::ZERO);
    let preparation = crate::world::bytes::conditional::preparation(
        100,
        &offset,
        0,
        0,
        &conditional,
        domain.borrowed(),
    );
    let args = vec![
        EvmValue::from_numeric(Word::private()),
        EvmValue::constant(U256::from(255)),
    ];
    let mut budget = WorkBudget::new(100_000);
    let entries;
    let counts;
    {
        let context = Context::budgeted(
            &facts,
            &conditional,
            domain.borrowed(),
            &preparation,
            &mut budget,
        )
        .unwrap()
        .unwrap();
        let mut memo = super::OperationMemo {
            values: Vec::new(),
            entries: 0,
            work: 0,
        };
        COUNTS.set(Counts::default());
        let result = memo
            .apply(
                opcode::AND,
                args,
                &context,
                domain.borrowed(),
                &mut super::payment::ReadPayment::unpaid(),
            )
            .unwrap();
        assert!(result.contains(U256::from(3)) && !result.contains(U256::from(2)));
        entries = memo.values.len();
        counts = COUNTS.get();
        eprintln!(
            "original demand: entries={entries}, clones={}, payments={:?}",
            counts.clones,
            context.demand_payments()
        );
    }
    eprintln!(
        "original demand ledger: used={} maximum=100000, one result Clone quote=1000000",
        budget.used()
    );
    assert!(budget.used() < 1_000_000);
    assert_eq!(counts.clones, 0);
    assert_eq!(counts.equalities, 0);
    assert_eq!(
        entries, 0,
        "ReadPayment::unpaid may still carry an original paid Context; optional native retention cannot bypass its tariff"
    );
}

#[test]
fn copy_and_view_evaluate_guarded_length_before_unavailable_retained_query() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy::default());
    let condition = ExprId::fresh().unwrap();
    let len = ExprId::ite(
        condition.clone(),
        ExprId::constant(U256::ONE),
        ExprId::constant(U256::from(3)),
        ExprLimits::default(),
    )
    .unwrap();
    let facts = Facts::from([
        (
            condition,
            EvmValue::from_numeric(Word::plain(Some(U256::ONE))),
        ),
        (
            len.clone(),
            EvmValue::from_numeric(Word {
                query_fee: usize::MAX,
                ..Word::private()
            }),
        ),
    ]);
    let mut setup = WorkBudget::new(1_000_000);
    let context = Context::with_payment(&facts, None, domain.borrowed(), &mut setup).unwrap();
    // The ordinary scalar evaluator selects length1 and preserves the private
    // odd constraint without querying the opaque retained bound.
    COUNTS.set(Counts::default());
    let evaluated =
        super::scalar_in_with_payment(&len, &context, domain.borrowed(), &mut setup).unwrap();
    assert_eq!(evaluated.numeric().exact, Some(U256::ONE));
    assert!(evaluated.contains(U256::ONE) && !evaluated.contains(U256::from(2)));
    assert_eq!(COUNTS.get().bad_queries, 0);
    let zero = ExprId::constant(U256::ZERO);
    let at = ExprId::constant(U256::from(2));
    let previous = ByteStateId::zero()
        .write_byte(
            at.clone(),
            ExprId::constant(U256::from(7)),
            StateLimits::default(),
        )
        .unwrap();
    let backing = ByteStateId::exact(Arc::from([0xaau8, 0xbb, 0xcc]));
    let source = ByteView::new(
        backing.clone(),
        zero.clone(),
        ExprId::constant(U256::from(3)),
    );
    let copied = previous
        .clone()
        .copy(
            zero.clone(),
            source.clone(),
            len.clone(),
            StateLimits::default(),
        )
        .unwrap();
    let literal_copy = previous
        .copy(
            zero.clone(),
            source,
            ExprId::constant(U256::ONE),
            StateLimits::default(),
        )
        .unwrap();
    let sliced = backing
        .clone()
        .slice(zero.clone(), len, StateLimits::default())
        .unwrap();
    let literal_slice = backing
        .slice(zero, ExprId::constant(U256::ONE), StateLimits::default())
        .unwrap();
    let mut results = Vec::new();
    for (name, root, control, expected) in [
        ("COPY", copied, literal_copy, 7u8),
        ("view", sliced, literal_slice, 0u8),
    ] {
        let mut budget = WorkBudget::new(1_000_000);
        let mut reader =
            Reader::with_payment(&facts, None, domain.borrowed(), &mut budget).unwrap();
        let control = reader
            .read_with_payment(&control, &at, &mut budget)
            .unwrap();
        assert_eq!(control.numeric().exact, Some(U256::from(expected)));
        COUNTS.set(Counts::default());
        let result = reader.read_with_payment(&root, &at, &mut budget);
        eprintln!(
            "optional {name} bound: scalar length1 succeeds; read={result:?}; bad_queries={}",
            COUNTS.get().bad_queries
        );
        results.push((name, result, expected));
    }
    for (name, result, expected) in results {
        assert!(
            result.is_ok(),
            "optional {name} proof must not require an unavailable retained singleton query before normal ITE evaluation"
        );
        assert_eq!(result.unwrap().numeric().exact, Some(U256::from(expected)));
    }
}

#[test]
fn output_join_keeps_supported_original_path_when_standalone_equality_is_unavailable() {
    use crate::{
        analysis::{Acceleration, PolicyContext, ProcedureOutput},
        world::{EvmEntry, EvmStore, EvmWorld},
    };
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy {
        equality: usize::MAX,
        ..Policy::default()
    });
    let world = EvmWorld::<Word>::new(crate::Fork::Cancun, "empty output old-path control");
    let entry = EvmEntry::<Word>::new(alloy_primitives::Address::ZERO);
    let store = EvmStore::new(&world, domain.borrowed());
    let data = EvmByteArray::<Word>::empty();
    let mut setup = WorkBudget::new(1_000_000);
    let data = data
        .summary_projection_with_payment(domain.borrowed(), &mut setup)
        .unwrap();
    let store = store
        .summary_projection_with_payment(domain.borrowed(), &mut setup)
        .unwrap();
    let left = ProcedureOutput::from_summaries(
        data,
        store,
        &mut PolicyContext::new(domain.borrowed(), &entry, &mut setup),
    )
    .unwrap();
    let right = left.clone();
    assert!(!std::ptr::eq(&left, &right));
    // Exact pre-change body, with the old pointer fast path inapplicable.
    let mut old = WorkBudget::new(1_000_000);
    let old_result = (|| -> Result<ProcedureOutput<Word>, crate::analysis::MachineError> {
        let mut cx = PolicyContext::new(domain.borrowed(), &entry, &mut old);
        cx.reserve(
            left.work_size()
                .saturating_add(right.work_size())
                .saturating_mul(4),
        )?;
        let data = left
            .data()
            .join_with_payment(right.data(), cx.domain, false, cx.budget)?;
        let store = left
            .store()
            .join_with_payment(right.store(), cx.domain, false, cx.budget)?;
        ProcedureOutput::from_summaries(data, store, &mut cx)
    })();
    assert!(
        old_result.is_ok(),
        "do not flag an optional capability unless the exact old body is supported: {old_result:?}"
    );
    let mut current = WorkBudget::new(1_000_000);
    let result = left.covering_join(
        &right,
        Acceleration::Join,
        &mut PolicyContext::new(domain.borrowed(), &entry, &mut current),
    );
    eprintln!(
        "ProcedureOutput Eq MAX: exact old Join succeeds used={}; new shortcut={result:?} used={}",
        old.used(),
        current.used()
    );
    assert!(
        result.is_ok(),
        "optional Eq shortcut must not newly reject a supported original output Join"
    );
    assert_eq!(current.used(), old.used());
}

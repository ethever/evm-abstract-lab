//! A paid key census bounds the unchanged COPY without discovering N.

use super::*;
use crate::{
    domain::{
        Domain, DomainSpec, FiniteConstantSet, NumericReduction, NumericValue, ValueAlgebra,
        WordObservation,
        provenance::Origin,
        relational::RelationLimits,
        symbolic::{ByteStateId, ExprId, ExprLimits},
    },
    resource::WorkBudget,
    test_instrumentation::Counter,
};
use std::fmt;

#[derive(Clone, Copy, Default, Debug)]
struct Receipt {
    keys: usize,
    quotes: usize,
    views: usize,
    clones: usize,
    work: usize,
    queries: usize,
    query_quotes: usize,
    copy_quotes: usize,
    copied_bytes: usize,
    prepared_paid: usize,
    cancel: Option<bool>,
}

static RECEIPT: Counter<Receipt> = Counter::new(Receipt {
    keys: 0,
    quotes: 0,
    views: 0,
    clones: 0,
    work: 0,
    queries: 0,
    query_quotes: 0,
    copy_quotes: 0,
    copied_bytes: 0,
    prepared_paid: 0,
    cancel: None,
});

pub(super) fn key_seen(captured: &analysis_control::Cancellation) {
    let mut cancel = None;
    RECEIPT.update(|receipt| {
        receipt.keys += 1;
        if receipt.keys == 2 {
            cancel = receipt.cancel;
        }
    });
    if let Some(captured_cancel) = cancel {
        if captured_cancel {
            captured.cancel();
        } else {
            analysis_control::current_cancellation().cancel();
        }
    }
}

pub(super) fn quote_started(paid: usize) {
    RECEIPT.update(|receipt| {
        receipt.quotes += 1;
        receipt.prepared_paid = paid;
    });
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct Word(NumericValue, Vec<u8>);

impl Clone for Word {
    fn clone(&self) -> Self {
        RECEIPT.update(|receipt| {
            receipt.clones += 1;
            receipt.copied_bytes += self.1.len();
        });
        Self(self.0.clone(), self.1.clone())
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self(NumericValue::top(), Vec::new())
    }
    fn constant(word: U256) -> Self {
        Self(NumericValue::constant(word), Vec::new())
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self(NumericValue::from_observation(observation), Vec::new())
    }
    fn observation(&self) -> WordObservation {
        self.0.observation()
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        RECEIPT.update(|receipt| receipt.views += 1);
        self.0.finite_constants()
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|receipt| receipt.queries += 1);
        None
    }
    fn singleton_work(&self) -> usize {
        RECEIPT.update(|receipt| receipt.query_quotes += 1);
        usize::MAX
    }
    fn is_top(&self) -> bool {
        self.0.is_top()
    }
    fn work_size(&self) -> usize {
        RECEIPT.update(|receipt| receipt.work += 1);
        self.0.work_size().saturating_add(self.1.len())
    }
    fn clone_work(&self) -> usize {
        RECEIPT.update(|receipt| receipt.copy_quotes += 1);
        if self.1.len() == 4097 {
            return usize::MAX;
        }
        self.0.clone_work().saturating_add(self.1.len())
    }
}

#[derive(Debug)]
struct Policy;

impl EvmNumericAlgebra for Policy {
    type Element = Word;

    fn spec(&self) -> DomainSpec {
        Domain::default().spec().with_relations(RelationLimits {
            max_nodes: 65_536,
            max_depth: 512,
            ..Default::default()
        })
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Word(
            Domain::default().join_numeric(&left.0, &right.0),
            Vec::new(),
        )
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
    fn apply_numeric(&self, op: u8, args: &[Word]) -> NumericReduction<Word> {
        let args: Vec<_> = args.iter().map(|arg| arg.0.clone()).collect();
        NumericReduction::stable(Word(
            Domain::default().apply_numeric(op, &args).value,
            Vec::new(),
        ))
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        EvmNumericAlgebra::operation_work(&Domain::default(), args.map(|arg| &arg.0))
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.work_size()
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        EvmNumericAlgebra::refinement_work(&Domain::default(), &value.0, facts)
    }
}

type Bytes = EvmByteArray<Word>;
type Value = EvmValue<Word>;

fn literal(n: U256) -> Value {
    Value::constant(n).with_expression(ExprId::constant(n))
}

fn pair(depth: usize) -> (Bytes, Bytes) {
    let limits = ExprLimits {
        max_nodes: 65_536,
        max_depth: 512,
    };
    let mut key = ExprId::fresh().unwrap();
    for _ in 0..depth {
        key = ExprId::operation(
            revm_bytecode::opcode::ADD,
            &[key, ExprId::constant(U256::ONE)],
            limits,
        )
        .unwrap();
    }
    let mut target = Bytes::memory();
    target.length = literal(U256::from(96));
    target.operands.insert(
        key,
        Value::constant(U256::from(7))
            .with_origin(Origin::Storage)
            .with_identity(982_141, 1),
    );
    let mut source = Bytes::exact(&[0x11, 0x22, 0x33]);
    source.operands.insert(
        ExprId::fresh().unwrap(),
        Value::constant(U256::from(9))
            .with_origin(Origin::Calldata)
            .with_identity(982_141, 2),
    );
    (target, source)
}

#[test]
fn short_prefix_visits_no_keys_or_numeric_callbacks_and_full_short_retains_prefix() {
    let algebra = ValueAlgebra::new(Policy);
    let (target, source) = pair(48);
    let at = literal(U256::from(32));
    let size = literal(U256::from(3));
    let frozen = (target.clone(), source.clone());
    let _receipt = RECEIPT.scope();
    let mut pilot = WorkBudget::new(usize::MAX);
    assert!(
        target
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut pilot)
            .unwrap()
    );
    let total = pilot.used();
    let prepared = RECEIPT.get().prepared_paid;
    let conservative = target.copy_return_data_work_with(&source, &at, &size, algebra.borrowed());
    assert!(total < conservative);
    for maximum in [63, total - 1, total] {
        RECEIPT.set(Receipt::default());
        let mut budget = WorkBudget::new(maximum);
        let paid = target
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap();
        let receipt = RECEIPT.get();
        assert_eq!(paid, maximum == total);
        assert_eq!(receipt.clones, 0);
        assert_eq!((receipt.queries, receipt.query_quotes), (0, 0));
        if maximum < 64 {
            assert_eq!(receipt.copy_quotes, 0);
            assert_eq!(
                (
                    budget.used(),
                    receipt.keys,
                    receipt.quotes,
                    receipt.views,
                    receipt.work
                ),
                (0, 0, 0, 0, 0)
            );
        } else {
            assert_eq!((receipt.keys, receipt.quotes), (2, 1));
            assert_eq!(budget.used(), if paid { total } else { prepared });
        }
        assert_eq!((target.clone(), source.clone()), frozen);
    }
}

#[test]
fn current_and_captured_cancellation_after_census_precede_quotes_and_copies() {
    let algebra = ValueAlgebra::new(Policy);
    let (target, source) = pair(48);
    let at = literal(U256::from(32));
    let size = literal(U256::from(3));
    let prefix = 64 + 4 * (target.operands.len() + source.operands.len() + 1);
    let _receipt = RECEIPT.scope();
    for captured_cancel in [false, true] {
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        RECEIPT.set(Receipt {
            cancel: Some(captured_cancel),
            ..Receipt::default()
        });
        analysis_control::with_cancellation(&current, || {
            assert_eq!(
                target.pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget),
                Err(ExprError::Cancelled.into())
            );
            let receipt = RECEIPT.get();
            assert_eq!(receipt.copy_quotes, 0);
            assert_eq!(
                (budget.used(), receipt.keys, receipt.quotes),
                (prefix, 2, 0)
            );
            assert_eq!(
                (
                    receipt.work,
                    receipt.clones,
                    receipt.queries,
                    receipt.query_quotes
                ),
                (0, 0, 0, 0)
            );
        });
        assert_eq!(captured.is_cancelled(), captured_cancel);
        assert_eq!(current.is_cancelled(), !captured_cancel);
    }
}

#[test]
fn prepared_copy_keeps_complete_arrays_and_errors_for_partial_zero_and_overflow_ranges() {
    let algebra = ValueAlgebra::new(Policy);
    let (target, source) = pair(48);
    let _receipt = RECEIPT.scope();
    for (at, size) in [(32, 1), (32, 3), (32, 8), (32, 0)] {
        let at = literal(U256::from(at));
        let size = literal(U256::from(size));
        let mut expected = target.clone();
        expected
            .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
            .unwrap();
        let mut actual = target.clone();
        let mut budget = WorkBudget::new(usize::MAX);
        RECEIPT.set(Receipt::default());
        assert!(
            actual
                .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
                .unwrap()
        );
        assert_eq!((RECEIPT.get().queries, RECEIPT.get().query_quotes), (0, 0));
        actual
            .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
            .unwrap();
        assert_eq!(actual, expected);
        for (key, value) in &source.operands {
            if size.expression().and_then(ExprId::as_constant) != Some(U256::ZERO) {
                assert_eq!(actual.operands.get(key).unwrap().numeric(), value.numeric());
                assert_eq!(
                    actual.operands.get(key).unwrap().provenance(),
                    value.provenance()
                );
            }
        }
    }
    let at = literal(U256::MAX);
    let size = literal(U256::from(3));
    let mut expected = target.clone();
    let error = expected
        .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
        .unwrap_err();
    let mut actual = target.clone();
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(
        actual
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    assert_eq!(
        actual.copy_return_data_with(&at, &source, &size, 0, algebra.borrowed()),
        Err(error)
    );
    assert_eq!(actual, target);
}

#[test]
fn census_covers_keys_outside_the_content_root_and_preserves_invalid_key_rejection() {
    let algebra = ValueAlgebra::new(Policy);
    let (mut target, source) = pair(48);
    assert_eq!(target.root, ByteStateId::zero());
    let key_nodes = target.operands.keys().next().unwrap().nodes();
    assert!(key_nodes > target.root.nodes());
    let at = literal(U256::from(32));
    let size = literal(U256::from(3));
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(
        target
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    let normal = budget.used();
    let (deeper, _) = pair(96);
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(
        deeper
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    assert!(budget.used() > normal);
    let invalid = Value::constant(U256::ONE).with_symbolic_limit();
    target.operands.insert(ExprId::fresh().unwrap(), invalid);
    let frozen = target.clone();
    let mut budget = WorkBudget::new(usize::MAX);
    assert!(
        target
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    assert!(
        target
            .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
            .is_err()
    );
    assert_eq!(target, frozen);
}

#[test]
fn tiny_keys_with_deep_native_payloads_pay_copy_prices_before_the_same_complete_copy() {
    let algebra = ValueAlgebra::new(Policy);
    let (mut target, mut source) = pair(0);
    for value in target
        .operands
        .values_mut()
        .chain(source.operands.values_mut())
    {
        let numeric = Word(value.numeric().0.clone(), vec![0xab; 128 * 1024]);
        *value = value.clone().with_numeric(numeric);
    }
    let at = literal(U256::from(32));
    let size = literal(U256::from(3));
    let mut expected = target.clone();
    expected
        .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
        .unwrap();
    let mut actual = target.clone();
    let _receipt = RECEIPT.scope();
    let mut pilot = WorkBudget::new(usize::MAX);
    assert!(
        actual
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut pilot)
            .unwrap()
    );
    let total = pilot.used();
    let prepared = RECEIPT.get().prepared_paid;
    assert!(total >= 4 * 128 * 1024);
    RECEIPT.set(Receipt::default());
    let mut short = WorkBudget::new(total - 1);
    assert!(
        !actual
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut short)
            .unwrap()
    );
    assert_eq!(short.used(), prepared);
    assert_eq!(RECEIPT.get().copied_bytes, 0);
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(total);
    assert!(
        actual
            .pay_return_copy_with(&source, &at, &size, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    assert_eq!(RECEIPT.get().copied_bytes, 0);
    actual
        .copy_return_data_with(&at, &source, &size, 0, algebra.borrowed())
        .unwrap();
    assert_eq!(actual, expected);
    let receipt = RECEIPT.get();
    assert!(receipt.copied_bytes > 0 && receipt.copied_bytes <= budget.used());
    assert_eq!((receipt.queries, receipt.query_quotes), (0, 0));
}

#[test]
fn a_zero_request_never_censuses_or_clones_ignored_stale_source_facts() {
    let algebra = ValueAlgebra::new(Policy);
    let (target, mut source) = pair(48);
    for _ in 0..512 {
        source
            .operands
            .insert(ExprId::fresh().unwrap(), Value::top().with_symbolic_limit());
    }
    let at = literal(U256::MAX);
    let zero = literal(U256::ZERO);
    let mut actual = target.clone();
    let _receipt = RECEIPT.scope();
    let conservative = target.copy_return_data_work_with(&source, &at, &zero, algebra.borrowed());
    RECEIPT.set(Receipt::default());
    let mut pilot = WorkBudget::new(usize::MAX);
    assert!(
        actual
            .pay_return_copy_with(&source, &at, &zero, algebra.borrowed(), &mut pilot)
            .unwrap()
    );
    let total = pilot.used();
    RECEIPT.set(Receipt::default());
    let mut budget = WorkBudget::new(total);
    assert!(
        actual
            .pay_return_copy_with(&source, &at, &zero, algebra.borrowed(), &mut budget)
            .unwrap()
    );
    let receipt = RECEIPT.get();
    assert!(budget.used() > conservative);
    assert_eq!(
        (
            receipt.keys,
            receipt.clones,
            receipt.queries,
            receipt.query_quotes
        ),
        (0, 0, 0, 0)
    );
    actual
        .copy_return_data_with(&at, &source, &zero, 0, algebra.borrowed())
        .unwrap();
    assert_eq!(actual, target);
}

#[test]
fn native_copy_prices_do_not_depend_on_both_fact_maps_being_nonempty() {
    let algebra = ValueAlgebra::new(Policy);
    let (target, source) = pair(0);
    let _receipt = RECEIPT.scope();
    for lane in ["empty target", "empty source", "zero request"] {
        let mut target = target.clone();
        let mut source = source.clone();
        let mut destination = literal(U256::from(32));
        let mut requested = literal(U256::from(3));
        let expensive = |value: &Value| Word(value.numeric().0.clone(), vec![0; 4097]);
        match lane {
            "empty target" => {
                target.operands.clear();
                let value = source.operands.values_mut().next().unwrap();
                *value = value.clone().with_numeric(expensive(value));
            }
            "empty source" => {
                source = Bytes::empty();
                let value = target.operands.values_mut().next().unwrap();
                *value = value.clone().with_numeric(expensive(value));
            }
            "zero request" => {
                destination = destination.clone().with_numeric(expensive(&destination));
                requested = literal(U256::ZERO);
            }
            _ => unreachable!(),
        }
        RECEIPT.set(Receipt::default());
        let mut budget = WorkBudget::new(usize::MAX);
        assert!(
            !target
                .pay_return_copy_with(
                    &source,
                    &destination,
                    &requested,
                    algebra.borrowed(),
                    &mut budget
                )
                .unwrap(),
            "{lane}"
        );
        let receipt = RECEIPT.get();
        assert_eq!(
            (
                receipt.keys,
                receipt.clones,
                receipt.copied_bytes,
                receipt.queries,
                receipt.query_quotes
            ),
            (0, 0, 0, 0, 0),
            "{lane}"
        );
        assert!(receipt.copy_quotes > 0 && budget.used() > 0);
    }
}

//! COPY's independent zero proofs must select the body bought by its quote.

use super::*;
use crate::{
    analysis::{EvmContinuation, EvmMachinePayload},
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, FiniteConstantSet, NumericReduction, NumericValue,
        WordElement, WordObservation, symbolic::ByteStateKind,
    },
    test_instrumentation::Counter,
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use std::fmt;

static QUERIES: Counter<usize> = Counter::new(0);
static QUERY_QUOTES: Counter<usize> = Counter::new(0);

mod nonzero;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
struct Word(NumericValue);

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self(NumericValue::top())
    }
    fn constant(word: U256) -> Self {
        Self(NumericValue::constant(word))
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self(NumericValue::from_observation(facts))
    }
    fn observation(&self) -> WordObservation {
        self.0.observation()
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        self.0.finite_constants()
    }
    fn singleton(&self) -> Option<U256> {
        // Declining discovery remains legal even with a complete finite view.
        QUERIES.update(|queries| *queries += 1);
        None
    }
    fn singleton_work(&self) -> usize {
        QUERY_QUOTES.update(|quotes| *quotes += 1);
        usize::MAX
    }
    fn is_top(&self) -> bool {
        self.0.is_top()
    }
    fn work_size(&self) -> usize {
        self.0.work_size()
    }
}

#[derive(Debug)]
struct Policy;

impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        Domain::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Word(Domain::default().join_numeric(&left.0, &right.0))
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
        NumericReduction::stable(Word(Domain::default().apply_numeric(op, &args).value))
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

type Value = EvmValue<Word>;
type Bytes = EvmByteArray<Word>;

fn value(number: u64) -> Value {
    Value::constant(U256::from(number))
}

fn literal_zero() -> Value {
    Value::top().with_expression(ExprId::constant(U256::ZERO))
}

fn maybe_one() -> Value {
    Value::from_numeric(Policy.join_numeric(value(0).numeric(), value(1).numeric()))
        .with_expression(ExprId::fresh().unwrap())
}

fn memory() -> Bytes {
    let mut memory = Bytes::memory();
    memory
        .copy_from_with(
            &value(0),
            &Bytes::exact(&[0xaa, 0xbb]),
            &value(0),
            &value(2),
            0,
            ValueAlgebra::new(Policy).borrowed(),
        )
        .unwrap();
    memory
}

fn empty_sources() -> [Bytes; 3] {
    let algebra = ValueAlgebra::new(Policy);
    let history = Bytes::exact(&[0x11; 128]);
    [
        Bytes::empty(),
        // The nonliteral length keeps the captured source and its numeric fact.
        history
            .slice_with(
                &value(0),
                &value(0).with_expression(ExprId::fresh().unwrap()),
                0,
                algebra.borrowed(),
            )
            .unwrap(),
        history
            .slice_with(&value(0), &literal_zero(), 0, algebra.borrowed())
            .unwrap(),
    ]
}

#[test]
fn zero_request_body_reuses_finite_and_literal_proofs_without_discovery() {
    let algebra = ValueAlgebra::new(Policy);
    assert_eq!(value(0).constants().unwrap().len(), 1);
    assert!(value(0).expression().is_none());
    assert_eq!(value(0).singleton(), None);
    assert_eq!(value(0).singleton_work(), usize::MAX);
    assert!(
        !algebra
            .numeric_algebra()
            .supports_product_scalar_shortcuts()
    );
    let original = memory();
    let source = Bytes::exact(&[0x11; 128]);
    let _queries = QUERIES.scope();
    let _quotes = QUERY_QUOTES.scope();
    for requested in [value(0), literal_zero()] {
        for destination in [
            Value::constant(U256::MAX),
            Value::unknown_byte(),
            Value::top().with_expression(ExprId::fresh().unwrap()),
        ] {
            let fee = original.copy_return_data_work_with(
                &source,
                &destination,
                &requested,
                algebra.borrowed(),
            );
            assert_eq!(
                fee,
                Bytes::memory().copy_return_data_work_with(
                    &Bytes::empty(),
                    &destination,
                    &requested,
                    algebra.borrowed(),
                ),
            );
            let mut actual = original.clone();
            actual
                .copy_return_data_with(&destination, &source, &requested, 0, algebra.borrowed())
                .unwrap();
            assert_eq!(actual, original);
        }
    }
    assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
}

#[test]
fn empty_source_body_keeps_requested_expansion_and_original_bytes_without_discovery() {
    let algebra = ValueAlgebra::new(Policy);
    let original = memory();
    let sources = empty_sources();
    for source in sources {
        for requested in [value(64), maybe_one()] {
            let mut expected = original.clone();
            expected
                .expand_with(&value(32), &requested, 0, algebra.borrowed())
                .unwrap();
            let _queries = QUERIES.scope();
            let _quotes = QUERY_QUOTES.scope();
            let mut actual = original.clone();
            actual
                .copy_return_data_with(&value(32), &source, &requested, 0, algebra.borrowed())
                .unwrap();
            assert_eq!(actual.state_root(), original.state_root());
            if requested.constants().unwrap().len() == 1 {
                assert_eq!(actual, expected);
                assert_eq!(
                    actual.len().constants().unwrap().first(),
                    Some(&U256::from(96))
                );
            } else {
                assert_eq!(actual.len().constants(), expected.len().constants());
                assert!(actual.len().contains(U256::from(32)));
                assert!(actual.len().contains(U256::from(64)));
            }
            assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
        }
    }
}

#[test]
fn literal_empty_source_keeps_its_zero_proof_through_minimum_without_a_finite_view() {
    use crate::analysis::transfer::query::tests::{
        Policy as OpaquePolicy, RECEIPT, Word as OpaqueWord,
    };

    let algebra = ValueAlgebra::new(OpaquePolicy);
    let literal = |number: u64| {
        EvmValue::<OpaqueWord>::top().with_expression(ExprId::constant(U256::from(number)))
    };
    let source = EvmByteArray::<OpaqueWord>::exact(&[0x11; 128])
        .slice_with(&literal(0), &literal(0), 0, algebra.borrowed())
        .unwrap();
    assert!(source.len().constants().is_none());
    assert!(
        EvmValue::<OpaqueWord>::constant(U256::ZERO)
            .constants()
            .is_none()
    );
    let mut memory = EvmByteArray::<OpaqueWord>::memory();
    let original = memory.state_root().clone();
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = Some(U256::ZERO);
        receipt.fee = usize::MAX;
    });
    let quote =
        memory.copy_return_data_work_with(&source, &literal(32), &literal(64), algebra.borrowed());
    assert!(quote > 0 && quote < usize::MAX);
    memory
        .copy_return_data_with(&literal(32), &source, &literal(64), 0, algebra.borrowed())
        .unwrap();
    assert_eq!(memory.state_root(), &original);
    assert!(memory.len().contains(U256::from(96)));
    assert_eq!((RECEIPT.get().queries, RECEIPT.get().quotes), (0, 0));
}

#[test]
fn nonzero_and_uncertain_ranges_keep_the_complete_copy_and_concrete_witnesses() {
    let algebra = ValueAlgebra::new(Policy);
    let source = Bytes::exact(&[0x11]);
    let uncertain = source
        .slice_with(&value(0), &maybe_one(), 0, algebra.borrowed())
        .unwrap();
    for (source, requested, exact) in [
        (&source, value(1), true),
        (&source, maybe_one(), false),
        (&uncertain, value(1), false),
        (&uncertain, maybe_one(), false),
    ] {
        let mut actual = memory();
        let previous = actual.state_root().clone();
        actual
            .copy_return_data_with(&value(0), source, &requested, 0, algebra.borrowed())
            .unwrap();
        assert!(matches!(
            actual.state_root().kind(),
            ByteStateKind::Copy { previous: old, .. } if old == &previous
        ));
        let first = actual
            .read_byte_checked_with(&value(0), algebra.borrowed())
            .unwrap();
        assert!(first.contains(U256::from(0x11)));
        if exact {
            assert_eq!(first.constants().unwrap().first(), Some(&U256::from(0x11)));
        } else {
            assert!(first.contains(U256::from(0xaa)));
        }
        let suffix = actual
            .read_byte_checked_with(&value(1), algebra.borrowed())
            .unwrap();
        assert!(suffix.contains(U256::from(0xbb)));
    }
}

fn child(destination: Value, requested: Value) -> EvmMachinePayload<Word> {
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "COPY zero proof");
    world
        .insert(
            owner,
            EvmAccount::<Word>::from_hex("00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(owner, Address::ZERO, value(0), Bytes::empty());
    let algebra = ValueAlgebra::new(Policy);
    let mut parent = initial(&world, &entry, algebra.borrowed()).unwrap();
    parent.active_mut().memory = memory();
    let child = parent
        .call_stack
        .root()
        .clone()
        .into_child(EvmContinuation {
            return_block: Some(0),
            output_offset: destination,
            output_size: requested,
            creation: None,
        });
    parent.call_stack.push_child(child);
    parent
}

#[test]
fn production_finish_pays_the_full_quote_and_refuses_a_smaller_budget_atomically() {
    let algebra = ValueAlgebra::new(Policy);
    let config = ExecutionConfig::default();
    let mut cases = vec![
        (Bytes::exact(&[0x11; 128]), value(0)),
        (Bytes::exact(&[0x11; 128]), literal_zero()),
    ];
    cases.extend(empty_sources().map(|source| (source, value(64))));
    for (source, requested) in cases {
        let payload = child(value(32), requested.clone());
        let original = payload.call_stack.parent().unwrap().memory.clone();
        let mut expected = original.clone();
        if requested != value(0) && requested != literal_zero() {
            expected
                .expand_with(&value(32), &requested, 0, algebra.borrowed())
                .unwrap();
        }
        let _queries = QUERIES.scope();
        let _quotes = QUERY_QUOTES.scope();
        let fee = original.copy_return_data_work_with(
            &source,
            &value(32),
            &requested,
            algebra.borrowed(),
        );
        assert!(fee > 0 && fee < usize::MAX);
        for funded in [false, true] {
            let mut budget = WorkBudget::new(17 + fee - usize::from(!funded));
            assert!(budget.charge(17));
            let mut result = super::super::super::empty_execution(payload.clone());
            finish(
                &mut result,
                payload.clone(),
                OutcomeKind::Return,
                source.clone(),
                &mut TransferContext {
                    config: &config,
                    domain: algebra.borrowed(),
                    budget: &mut budget,
                    root_address: None,
                    input_scope: None,
                    query_cache: Default::default(),
                    procedure: false,
                    discover_code: false,
                },
                7,
            );
            assert_eq!(budget.used(), 17 + if funded { fee } else { 0 });
            assert_eq!(result.payload, payload);
            if funded {
                assert!(result.frontiers.is_empty());
                assert_eq!(result.successors.len(), 1);
                assert_eq!(result.completed_calls.len(), 1);
                let resumed = result.successors[0].payload.active();
                assert_eq!(resumed.memory, expected);
                assert_eq!(resumed.returndata, source);
                assert_eq!(resumed.memory.state_root(), original.state_root());
            } else {
                assert!(result.successors.is_empty());
                assert!(result.completed_calls.is_empty());
                assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
                assert!(budget.exhausted());
            }
        }
        assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
    }
}

#[test]
fn cancelled_production_finish_keeps_the_unresumed_parent() {
    let algebra = ValueAlgebra::new(Policy);
    let config = ExecutionConfig::default();
    let payload = child(value(32), value(64));
    let source = Bytes::empty();
    let owner = analysis_control::Cancellation::new();
    let mut budget = analysis_control::with_cancellation(&owner, || WorkBudget::new(usize::MAX));
    assert!(budget.charge(17));
    owner.cancel();
    let _queries = QUERIES.scope();
    let _quotes = QUERY_QUOTES.scope();
    let mut result = super::super::super::empty_execution(payload.clone());
    finish(
        &mut result,
        payload.clone(),
        OutcomeKind::Return,
        source,
        &mut TransferContext {
            config: &config,
            domain: algebra.borrowed(),
            budget: &mut budget,
            root_address: None,
            input_scope: None,
            query_cache: Default::default(),
            procedure: false,
            discover_code: false,
        },
        7,
    );
    assert_eq!(budget.used(), 17);
    assert_eq!(result.payload, payload);
    assert!(result.successors.is_empty());
    assert!(result.completed_calls.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!((QUERIES.get(), QUERY_QUOTES.get()), (0, 0));
}

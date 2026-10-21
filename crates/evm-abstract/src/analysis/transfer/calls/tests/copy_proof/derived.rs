//! Fresh range results have copy prices independent of the original operands.

use super::*;
use std::{
    sync::Mutex,
    thread::{self, ThreadId},
};

#[derive(Clone, Copy, Default)]
struct Receipt {
    quotes: usize,
    clones: usize,
}

static COPIES: Counter<Receipt> = Counter::new(Receipt {
    quotes: 0,
    clones: 0,
});

#[derive(Default)]
struct Factory {
    watched: Option<U256>,
    price: usize,
    cancel: Option<analysis_control::Cancellation>,
}

// Match the concrete per-thread registry used by analysis_control. Only
// fixture configuration/handle lookup holds the lock, never analysis work.
static FACTORY: Mutex<Vec<(ThreadId, Factory)>> = Mutex::new(Vec::new());

fn with_factory(operation: impl FnOnce(&mut Factory)) {
    let owner = thread::current().id();
    let mut factories = FACTORY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = match factories.iter().position(|(thread, _)| *thread == owner) {
        Some(index) => index,
        None => {
            factories.push((owner, Factory::default()));
            factories.len() - 1
        }
    };
    operation(&mut factories[index].1);
}

struct ResetFactory;
impl Drop for ResetFactory {
    fn drop(&mut self) {
        let owner = thread::current().id();
        let mut factories = FACTORY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(index) = factories.iter().position(|(thread, _)| *thread == owner) {
            factories.swap_remove(index);
        }
    }
}

#[derive(Debug, serde::Serialize)]
struct DerivedWord {
    native: NumericValue,
    price: usize,
    #[serde(skip)]
    cancel: Option<analysis_control::Cancellation>,
}

impl PartialEq for DerivedWord {
    fn eq(&self, other: &Self) -> bool {
        self.native == other.native && self.price == other.price
    }
}
impl Eq for DerivedWord {}

impl DerivedWord {
    fn cheap(native: NumericValue) -> Self {
        Self {
            native,
            price: 0,
            cancel: None,
        }
    }
}

impl Clone for DerivedWord {
    fn clone(&self) -> Self {
        if self.price != 0 {
            COPIES.update(|receipt| receipt.clones += 1);
        }
        Self {
            native: self.native.clone(),
            price: self.price,
            cancel: self.cancel.clone(),
        }
    }
}

impl fmt::Display for DerivedWord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.native.fmt(f)
    }
}

impl WordElement for DerivedWord {
    fn top() -> Self {
        Self::cheap(NumericValue::top())
    }
    fn constant(value: U256) -> Self {
        let mut word = Self::cheap(NumericValue::constant(value));
        with_factory(|factory| {
            if factory.watched == Some(value) {
                word.price = factory.price;
                word.cancel = factory.cancel.clone();
            }
        });
        word
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self::cheap(NumericValue::from_observation(facts))
    }
    fn observation(&self) -> WordObservation {
        self.native.observation()
    }
    fn finite_constants(&self) -> &FiniteConstantSet {
        self.native.finite_constants()
    }
    fn singleton(&self) -> Option<U256> {
        self.native.singleton()
    }
    fn is_top(&self) -> bool {
        self.native.is_top()
    }
    fn work_size(&self) -> usize {
        self.native.work_size().saturating_add(4)
    }
    fn clone_work(&self) -> usize {
        if self.price != 0 {
            COPIES.update(|receipt| receipt.quotes += 1);
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            }
            self.price
        } else {
            self.native.clone_work().saturating_add(4)
        }
    }
}

#[derive(Debug)]
struct DerivedPolicy;

impl EvmNumericAlgebra for DerivedPolicy {
    type Element = DerivedWord;

    fn spec(&self) -> DomainSpec {
        Domain::default().spec()
    }
    fn project_numeric(&self, value: &DerivedWord) -> DerivedWord {
        value.clone()
    }
    fn join_numeric(&self, left: &DerivedWord, right: &DerivedWord) -> DerivedWord {
        DerivedWord::cheap(Domain::default().join_numeric(&left.native, &right.native))
    }
    fn widen_numeric(&self, old: &DerivedWord, next: &DerivedWord) -> DerivedWord {
        self.join_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &DerivedWord, next: &DerivedWord) -> DerivedWord {
        self.join_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &DerivedWord) -> NumericReduction<DerivedWord> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric(&self, op: u8, args: &[DerivedWord]) -> NumericReduction<DerivedWord> {
        let args: Vec<_> = args.iter().map(|value| value.native.clone()).collect();
        NumericReduction::stable(DerivedWord::cheap(
            Domain::default().apply_numeric(op, &args).value,
        ))
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a DerivedWord>) -> usize {
        EvmNumericAlgebra::operation_work(&Domain::default(), args.map(|value| &value.native))
    }
    fn projection_work(&self, value: &DerivedWord) -> usize {
        value.clone_work()
    }
    fn refinement_work(&self, value: &DerivedWord, facts: usize) -> usize {
        value
            .clone_work()
            .saturating_add(EvmNumericAlgebra::refinement_work(
                &Domain::default(),
                &value.native,
                facts,
            ))
    }
    fn literal_quote_work(&self, _: U256) -> usize {
        // Constructing a small NumericValue and a cancellation handle is cheap;
        // the returned element's conservative copy bound remains independent.
        128
    }
}

type DerivedBytes = EvmByteArray<DerivedWord>;
type DerivedValue = EvmValue<DerivedWord>;

fn literal(value: u64) -> DerivedValue {
    DerivedValue::constant(U256::from(value)).with_expression(ExprId::constant(U256::from(value)))
}

fn fixture(requested: u64) -> EvmMachinePayload<DerivedWord> {
    let owner = Address::repeat_byte(0x71);
    let mut world = EvmWorld::new(Fork::Osaka, "derived COPY price");
    world
        .insert(owner, EvmAccount::from_hex("00", Fork::Osaka).unwrap())
        .unwrap();
    let entry = EvmEntry::concrete(owner, Address::ZERO, literal(0), DerivedBytes::empty());
    let algebra = ValueAlgebra::new(DerivedPolicy);
    let mut payload = initial(&world, &entry, algebra.borrowed()).unwrap();
    let child = payload
        .call_stack
        .root()
        .clone()
        .into_child(EvmContinuation {
            return_block: Some(0),
            output_offset: literal(32),
            output_size: literal(requested),
            creation: None,
        });
    payload.call_stack.push_child(child);
    payload
}

fn complete(
    payload: EvmMachinePayload<DerivedWord>,
    source: DerivedBytes,
    result: &mut crate::analysis::transfer::EvmExecution<DerivedWord>,
    budget: &mut WorkBudget,
) {
    finish(
        result,
        payload,
        OutcomeKind::Return,
        source,
        &mut TransferContext {
            config: &ExecutionConfig::default(),
            domain: ValueAlgebra::new(DerivedPolicy).borrowed(),
            budget,
            root_address: None,
            input_scope: None,
            query_cache: Default::default(),
            procedure: false,
            discover_code: false,
        },
        7,
    );
}

#[test]
fn actual_finish_refuses_new_msize_and_minimum_max_prices_before_their_first_clone() {
    let _scope = COPIES.scope();
    let _reset = ResetFactory;
    for (watched, requested) in [(64, 1), (3, 3)] {
        with_factory(|factory| *factory = Factory::default());
        // Old values are cheap, including the original request/source length.
        let payload = fixture(requested);
        let source = DerivedBytes::exact(&vec![0x11; requested as usize]);
        let original = payload.clone();
        let mut result = crate::analysis::transfer::empty_execution(payload.clone());
        with_factory(|factory| {
            factory.watched = Some(U256::from(watched));
            factory.price = usize::MAX;
        });
        COPIES.set(Receipt::default());
        let mut budget = WorkBudget::new(usize::MAX);
        complete(payload, source, &mut result, &mut budget);
        assert_eq!(COPIES.get().clones, 0, "fresh constant {watched}");
        assert_eq!(COPIES.get().quotes, 1, "fresh constant {watched}");
        assert_eq!(result.payload, original);
        assert!(result.successors.is_empty() && result.completed_calls.is_empty());
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        assert!(budget.used() > 0);
    }
}

#[test]
fn fresh_result_full_fee_preserves_copy_and_raw_capture_but_fee_minus_one_publishes_nothing() {
    let _scope = COPIES.scope();
    let _reset = ResetFactory;
    let algebra = ValueAlgebra::new(DerivedPolicy);
    for (watched, requested, source_len, copies) in [(64, 8, 1, 2), (3, 3, 3, 3)] {
        with_factory(|factory| *factory = Factory::default());
        let payload = fixture(requested);
        let source = DerivedBytes::exact(&vec![0x11; source_len]);
        with_factory(|factory| {
            factory.watched = Some(U256::from(watched));
            factory.price = 1_000_003;
        });
        let mut expected = payload.call_stack.parent().unwrap().memory.clone();
        expected
            .copy_return_data_with(
                &literal(32),
                &source,
                &literal(requested),
                0,
                algebra.borrowed(),
            )
            .unwrap();
        let (fee, prefix) =
            finish_prices(&payload, &source, OutcomeKind::Return, algebra.borrowed());
        assert!(fee >= copies * 1_000_003);
        for funded in [false, true] {
            let mut result = crate::analysis::transfer::empty_execution(payload.clone());
            let input = payload.clone();
            let data = source.clone();
            let mut budget = WorkBudget::new(fee - usize::from(!funded));
            COPIES.set(Receipt::default());
            complete(input, data, &mut result, &mut budget);
            let receipt = COPIES.get();
            assert_eq!(receipt.clones, if funded { copies } else { 0 });
            assert_eq!(budget.used(), if funded { fee } else { prefix });
            assert_eq!(result.payload, payload);
            if funded {
                assert!(result.frontiers.is_empty());
                assert_eq!(result.successors.len(), 1);
                assert_eq!(result.completed_calls.len(), 1);
                let raw = &result.completed_calls[0];
                assert_eq!(raw.payload, payload);
                assert_eq!(raw.data, source);
                assert_eq!(raw.output_data, source);
                let resumed = result.successors[0].payload.active();
                assert_eq!(resumed.memory, expected);
                assert_eq!(resumed.returndata, source);
            } else {
                assert!(result.successors.is_empty() && result.completed_calls.is_empty());
                assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
            }
        }
        // Inspect concrete byte witnesses outside the measured completion.
        assert!(expected.len().contains(U256::from(64)));
        assert!(
            expected
                .read_byte_checked_with(&literal(32), algebra.borrowed())
                .unwrap()
                .contains(U256::from(0x11))
        );
        assert!(
            expected
                .read_byte_checked_with(&literal(31), algebra.borrowed())
                .unwrap()
                .contains(U256::ZERO)
        );
    }
}

#[test]
fn fresh_result_price_hook_checks_current_and_captured_cancellation_before_copy() {
    let _scope = COPIES.scope();
    let _reset = ResetFactory;
    for watched in [64, 3] {
        for captured_cancel in [false, true] {
            with_factory(|factory| *factory = Factory::default());
            let payload = fixture(if watched == 64 { 1 } else { 3 });
            let source = DerivedBytes::exact(&[0x11; 3]);
            let mut result = crate::analysis::transfer::empty_execution(payload.clone());
            let original = payload.clone();
            let captured = analysis_control::Cancellation::new();
            let current = analysis_control::Cancellation::new();
            let mut budget =
                analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
            with_factory(|factory| {
                *factory = Factory {
                    watched: Some(U256::from(watched)),
                    price: 1_000_003,
                    cancel: Some(if captured_cancel {
                        captured.clone()
                    } else {
                        current.clone()
                    }),
                };
            });
            COPIES.set(Receipt::default());
            analysis_control::with_cancellation(&current, || {
                complete(payload, source, &mut result, &mut budget);
            });
            assert_eq!(COPIES.get().quotes, 1);
            assert_eq!(COPIES.get().clones, 0);
            assert_eq!(captured.is_cancelled(), captured_cancel);
            assert_eq!(current.is_cancelled(), !captured_cancel);
            assert_eq!(result.payload, original);
            assert!(result.successors.is_empty() && result.completed_calls.is_empty());
            assert_eq!(
                result.frontiers[0].reason,
                FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
            );
            assert!(budget.used() > 0);
        }
    }
}

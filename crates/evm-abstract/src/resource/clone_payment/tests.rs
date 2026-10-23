//! Private predicates have independent retention and declared-copy envelopes.

use crate::{
    Fork, U256,
    analysis::ExecutionConfig,
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue, ProductAlgebra,
        ValueAlgebra, WordElement, WordObservation,
    },
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;
use revm_bytecode::opcode;
use serde::Serialize;
use std::{collections::BTreeSet, fmt, num::NonZeroUsize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Receipt {
    pub clones: usize,
    pub bytes: usize,
    pub quotes: usize,
    pub private_quotes: usize,
    pub queries: usize,
    pub views: usize,
    pub exports: usize,
    pub armed: bool,
}

pub(crate) static RECEIPT: crate::test_instrumentation::Counter<Receipt> =
    crate::test_instrumentation::Counter::new(Receipt {
        clones: 0,
        bytes: 0,
        quotes: 0,
        private_quotes: 0,
        queries: 0,
        views: 0,
        exports: 0,
        armed: false,
    });

#[derive(Debug, Serialize)]
pub(crate) struct Word {
    native: NumericValue,
    // An owned exclusion predicate deliberately absent from the observation.
    pub private: Vec<u8>,
    pub declared_clone: usize,
    #[serde(skip)]
    pub cancel_price: Option<analysis_control::Cancellation>,
}

impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        self.native == other.native
            && self.private == other.private
            && self.declared_clone == other.declared_clone
    }
}
impl Eq for Word {}

impl Word {
    const WRAPPER_WORK: usize = 16;

    pub(crate) fn expensive(work: usize) -> Self {
        Self {
            native: NumericValue::unsigned_range(U256::ZERO, U256::from(32)).unwrap(),
            private: vec![7; 4096],
            declared_clone: work,
            cancel_price: None,
        }
    }

    pub(crate) fn with_predicate(mut self, work: usize) -> Self {
        self.private = vec![7; 4096];
        self.declared_clone = work;
        self
    }
}

impl Clone for Word {
    fn clone(&self) -> Self {
        RECEIPT.update(|receipt| {
            receipt.clones += 1;
            receipt.bytes += self.private.len();
        });
        Self {
            native: self.native.clone(),
            private: self.private.clone(),
            declared_clone: self.declared_clone,
            cancel_price: self.cancel_price.clone(),
        }
    }
}

impl fmt::Display for Word {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.native, f)
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self {
            native: NumericValue::top(),
            private: Vec::new(),
            declared_clone: 64,
            cancel_price: None,
        }
    }
    fn constant(value: U256) -> Self {
        Self {
            native: NumericValue::constant(value),
            ..Self::top()
        }
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            native: NumericValue::from_observation(value),
            ..Self::top()
        }
    }
    fn observation(&self) -> WordObservation {
        RECEIPT.update(|r| r.exports += 1);
        self.native.observation()
    }
    fn singleton(&self) -> Option<U256> {
        RECEIPT.update(|r| r.queries += 1);
        self.native.singleton().filter(|word| {
            !self
                .private
                .iter()
                .any(|excluded| U256::from(*excluded) == *word)
        })
    }
    fn contains(&self, word: U256) -> bool {
        RECEIPT.update(|r| r.queries += 1);
        self.native.contains(word)
            && !self
                .private
                .iter()
                .any(|excluded| U256::from(*excluded) == word)
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        RECEIPT.update(|r| r.views += 1);
        self.native.constants()
    }
    fn is_top(&self) -> bool {
        self.private.is_empty() && self.native.is_top()
    }
    fn work_size(&self) -> usize {
        self.native
            .work_size()
            .saturating_add(self.private.len())
            .saturating_add(Self::WRAPPER_WORK)
    }
    fn clone_work(&self) -> usize {
        let mut armed = false;
        RECEIPT.update(|r| {
            r.quotes += 1;
            r.private_quotes += usize::from(!self.private.is_empty());
            armed = r.armed;
        });
        if armed && let Some(cancel) = &self.cancel_price {
            cancel.cancel();
        }
        self.declared_clone
    }
    fn requires_retained_binding(&self) -> bool {
        !self.private.is_empty()
    }
}

#[derive(Debug)]
pub(crate) struct Policy(pub ProductAlgebra);
impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        Word {
            native: self.0.join_numeric(&left.native, &right.native),
            ..Word::top()
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
        opcode: u8,
        args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        let args: Vec<&Word> = args.collect();
        let args = args
            .iter()
            .map(|arg| arg.native.clone())
            .collect::<Vec<_>>();
        NumericReduction::stable(Word {
            native: self.0.apply_numeric(opcode, args.iter()).value,
            ..Word::top()
        })
    }
    fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        // Transfer and facade literals return an empty predicate. Price the
        // wrapper fields separately from the actual selected native policy.
        let mut wrapper_work = Word::WRAPPER_WORK;
        let expressions = self.spec().relations().enabled;
        let native_work = self.0.operation_work(
            op,
            args.map(|arg| {
                // Two temporary vectors stage/release their slots. Native
                // copies release shared handles while the original inputs
                // remain alive, so use their cheap copy cost, not work_size.
                wrapper_work = wrapper_work
                    .saturating_add(arg.native.clone_work().saturating_mul(2))
                    .saturating_add(4);
                // Facade expression discovery may query each input once; the
                // private exclusion scan is additional to the native query.
                if expressions {
                    wrapper_work = wrapper_work.saturating_add(arg.private.len());
                }
                &arg.native
            }),
        );
        native_work.saturating_add(wrapper_work)
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        128
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.work_size().saturating_mul(8)
    }
    fn refinement_work(&self, value: &Word, facts: usize) -> usize {
        value.work_size().saturating_add(facts).saturating_mul(8)
    }
}

#[derive(Debug)]
pub(crate) struct SummaryPolicy(pub Policy);
impl EvmNumericAlgebra for SummaryPolicy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.0.spec()
    }
    fn project_numeric(&self, v: &Word) -> Word {
        self.0.project_numeric(v)
    }
    fn join_numeric(&self, l: &Word, r: &Word) -> Word {
        self.0.join_numeric(l, r)
    }
    fn widen_numeric(&self, l: &Word, r: &Word) -> Word {
        self.0.widen_numeric(l, r)
    }
    fn widen_procedure_numeric(&self, l: &Word, r: &Word) -> Word {
        self.0.widen_procedure_numeric(l, r)
    }
    fn reduce_numeric(&self, v: &Word) -> NumericReduction<Word> {
        self.0.reduce_numeric(v)
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
    fn literal_quote_work(&self, v: U256) -> usize {
        self.0.literal_quote_work(v)
    }
    fn projection_work(&self, v: &Word) -> usize {
        self.0.projection_work(v)
    }
    fn refinement_work(&self, v: &Word, f: usize) -> usize {
        self.0.refinement_work(v, f)
    }
    fn summary_join_work(&self, l: &Word, r: &Word, w: bool) -> usize {
        self.0
            .0
            .summary_join_work(&l.native, &r.native, w)
            .saturating_add(
                self.0
                    .0
                    .summary_construction_work(crate::domain::SummaryConstruction::Top),
            )
            .saturating_add(Word::WRAPPER_WORK)
    }
    fn summary_canonical_work(&self, v: &Word) -> usize {
        v.clone_work()
    }
    fn summary_construction_work(&self, c: crate::domain::SummaryConstruction) -> usize {
        self.0
            .0
            .summary_construction_work(c)
            .saturating_add(Word::WRAPPER_WORK)
    }
    fn summary_equal_work(&self, l: &Word, r: &Word) -> usize {
        self.0
            .0
            .summary_equal_work(&l.native, &r.native)
            .saturating_add(l.private.len())
            .saturating_add(r.private.len())
            .saturating_add(Word::WRAPPER_WORK)
    }
    fn summary_is_top_work(&self, v: &Word) -> usize {
        self.0
            .0
            .summary_is_top_work(&v.native)
            .saturating_add(Word::WRAPPER_WORK)
    }
    fn summary_compact_work(&self, v: &Word) -> usize {
        v.clone_work()
    }
    fn summary_restore_work(&self, v: &Word, _: &Word) -> usize {
        v.clone_work()
    }
    fn summary_observation_work(&self, v: &crate::domain::WordObservation) -> usize {
        self.0
            .0
            .summary_observation_work(v)
            .saturating_add(Word::WRAPPER_WORK)
    }
}

pub(crate) fn fixture(
    code: &str,
) -> (
    EvmWorld<Word>,
    EvmEntry<Word>,
    ExecutionConfig,
    ValueAlgebra<Policy>,
) {
    let address = Address::repeat_byte(0x11);
    let mut world = EvmWorld::new(Fork::Osaka, "owned snapshot payment");
    world
        .insert(address, EvmAccount::from_hex(code, world.fork()).unwrap())
        .unwrap();
    let config = ExecutionConfig {
        root_terminal_coverage: false,
        use_summaries: false,
        ..ExecutionConfig::default()
    };
    let policy = ValueAlgebra::new(Policy(ProductAlgebra::from_spec(DomainSpec::default())));
    (world, EvmEntry::new(address), config, policy)
}

pub(crate) fn summary_fixture(
    code: &str,
) -> (
    EvmWorld<Word>,
    EvmEntry<Word>,
    ExecutionConfig,
    ValueAlgebra<SummaryPolicy>,
) {
    let (world, entry, config, opaque) = fixture(code);
    let policy = ValueAlgebra::new(SummaryPolicy(Policy(ProductAlgebra::from_spec(
        opaque.spec(),
    ))));
    (world, entry, config, policy)
}

pub(crate) fn reset() {
    RECEIPT.set(Receipt::default());
}
pub(crate) fn assert_unobserved() {
    let receipt = RECEIPT.get();
    assert_eq!(
        (
            receipt.clones,
            receipt.bytes,
            receipt.queries,
            receipt.views,
            receipt.exports
        ),
        (0, 0, 0, 0, 0)
    );
}

pub(crate) fn private_value(work: usize) -> EvmValue<Word> {
    EvmValue::constant(U256::ONE)
        .with_numeric(Word::expensive(work))
        .with_origin(crate::domain::provenance::Origin::Environment)
        .with_identity(138, 7)
}

#[test]
fn operation_quote_covers_the_configured_product_output() {
    let _scope = RECEIPT.scope();
    let capacity = 4096;
    let policy = Policy(ProductAlgebra::from_spec(DomainSpec::new(
        NonZeroUsize::new(capacity).unwrap(),
        NonZeroUsize::new(4).unwrap(),
        // The finite cover and its other semantic atoms must both fit.
        NonZeroUsize::new(capacity * 2).unwrap(),
    )));
    let args = [Word::top(), Word::constant(U256::from(capacity - 1))];
    reset();
    let quote = policy.operation_work(opcode::AND, args.iter());
    let result = policy.apply_numeric(opcode::AND, args.iter()).value;
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert_eq!(result.native.constants().map(BTreeSet::len), Some(capacity));
    assert!(result.private.is_empty());
    assert_eq!(result.work_size(), capacity + 12 + Word::WRAPPER_WORK);
    let old_quote = args.iter().fold(1024usize, |work, arg| {
        work.saturating_add(arg.work_size().saturating_mul(8))
    });
    assert!(result.work_size() > old_quote);
    assert!(quote >= result.work_size());
}

#[test]
fn operation_quotes_follow_native_capacity_and_max_without_copy_or_query_hooks() {
    let _scope = RECEIPT.scope();
    let cancellation = analysis_control::Cancellation::new();
    // The declared copy refusal is independent of this borrowed operation.
    let mut word = Word::top().with_predicate(usize::MAX);
    word.cancel_price = Some(cancellation.clone());
    let args = [word, Word::constant(U256::from(131_071))];
    let receipt = Receipt {
        armed: true,
        ..Receipt::default()
    };
    RECEIPT.set(receipt);
    let quotes = [8, 131_072, usize::MAX].map(|capacity| {
        let policy = Policy(ProductAlgebra::from_spec(DomainSpec::new(
            NonZeroUsize::new(capacity).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            NonZeroUsize::new(capacity.saturating_mul(2)).unwrap(),
        )));
        let native = policy
            .0
            .operation_work(opcode::AND, args.iter().map(|arg| &arg.native));
        let quote = policy.operation_work(opcode::AND, args.iter());
        assert!(
            quote
                >= native
                    .saturating_add(Word::WRAPPER_WORK)
                    .saturating_add(args[0].private.len())
        );
        if capacity == usize::MAX {
            assert_eq!(native, usize::MAX);
        } else {
            assert_ne!(quote, usize::MAX);
        }
        assert_eq!(RECEIPT.get(), receipt);
        assert!(!cancellation.is_cancelled());
        quote
    });
    assert!(quotes[1] > quotes[0]);
    // Native AND Top with 2^17-1 has 2^17 candidates. The configured capacity
    // and fact limit admit that cover; NumericValue stores r+12 work, and this
    // wrapper adds 16 with no output predicate. No large result is constructed.
    assert!(quotes[1] >= 131_072 + 12 + Word::WRAPPER_WORK);
    assert_eq!(quotes[2], usize::MAX);
}

#[test]
fn operation_quote_covers_private_expression_queries_and_facade_literal() {
    let _scope = RECEIPT.scope();
    let policy = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    let args = [
        EvmValue::from_numeric(Word::constant(U256::from(3)).with_predicate(usize::MAX)),
        EvmValue::top().with_expression(crate::domain::symbolic::ExprId::constant(U256::from(3))),
    ];
    reset();
    let numeric = policy.numeric_algebra();
    let quote = numeric.operation_work(opcode::SUB, args.iter().map(EvmValue::numeric));
    assert_eq!(RECEIPT.get(), Receipt::default());
    assert!(
        numeric
            .apply_numeric(opcode::SUB, args.iter().map(EvmValue::numeric))
            .value
            .native
            .is_top()
    );
    let result = policy.apply(opcode::SUB, &args);
    assert_eq!(result.numeric().native.singleton(), Some(U256::ZERO));
    assert!(result.numeric().private.is_empty());
    assert!(quote >= result.numeric().work_size());
    assert_eq!(RECEIPT.get().queries, 3);
    assert_eq!((RECEIPT.get().clones, RECEIPT.get().quotes), (0, 0));
}

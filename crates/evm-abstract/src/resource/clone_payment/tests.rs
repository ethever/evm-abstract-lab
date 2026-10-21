//! Private predicates have independent retention and declared-copy envelopes.

use crate::{
    Fork, U256,
    analysis::ExecutionConfig,
    domain::{
        Domain, DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue,
        ValueAlgebra, WordElement, WordObservation,
    },
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::Address;
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

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
            .saturating_add(16)
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
pub(crate) struct Policy(pub Domain);
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
    fn apply_numeric(&self, opcode: u8, args: &[Word]) -> NumericReduction<Word> {
        let args = args
            .iter()
            .map(|arg| arg.native.clone())
            .collect::<Vec<_>>();
        NumericReduction::stable(Word {
            native: self.0.apply_numeric(opcode, &args).value,
            ..Word::top()
        })
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a Word>) -> usize {
        args.fold(1024usize, |work, value| {
            work.saturating_add(value.work_size().saturating_mul(8))
        })
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
    let policy = ValueAlgebra::new(Policy(config.domain().unwrap()));
    (world, EvmEntry::new(address), config, policy)
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

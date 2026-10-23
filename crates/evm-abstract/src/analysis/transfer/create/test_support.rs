//! Native factory/returned-value tariffs with separate quote and hook receipts.

use crate::{
    Fork,
    analysis::{EvmMachinePayload, ExecutionConfig, transfer::calls::initial},
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue, ProductAlgebra,
        SummaryConstruction, ValueAlgebra, WordElement, WordObservation,
    },
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::{Address, U256};
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Copy, Default, Debug)]
pub(in crate::analysis::transfer) struct Receipt {
    pub armed: bool,
    pub target: U256,
    pub fresh_nonce_fee: usize,
    pub target_quotes: usize,
    pub target_factories: usize,
    pub factories: usize,
    pub nonce_factories: usize,
    pub fresh_nonce_quotes: usize,
    pub fresh_nonce_clones: usize,
    pub top_quotes: usize,
    pub top_factories: usize,
}

pub(in crate::analysis::transfer) static RECEIPT: crate::test_instrumentation::Counter<Receipt> =
    crate::test_instrumentation::Counter::new(Receipt {
        armed: false,
        target: U256::ZERO,
        fresh_nonce_fee: 0,
        target_quotes: 0,
        target_factories: 0,
        factories: 0,
        nonce_factories: 0,
        fresh_nonce_quotes: 0,
        fresh_nonce_clones: 0,
        top_quotes: 0,
        top_factories: 0,
    });

#[derive(Debug, PartialEq, Eq, Serialize)]
pub(in crate::analysis::transfer) struct Word {
    native: NumericValue,
    fresh_nonce: bool,
    clone_fee: usize,
}

impl Clone for Word {
    fn clone(&self) -> Self {
        if self.fresh_nonce {
            RECEIPT.update(|r| r.fresh_nonce_clones += 1);
        }
        Self {
            native: self.native.clone(),
            fresh_nonce: self.fresh_nonce,
            clone_fee: self.clone_fee,
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
        RECEIPT.update(|r| r.top_factories += usize::from(r.armed));
        Self {
            native: NumericValue::top(),
            fresh_nonce: false,
            clone_fee: 64,
        }
    }
    fn constant(value: U256) -> Self {
        let mut fresh_nonce = false;
        let mut clone_fee = 64;
        RECEIPT.update(|r| {
            if r.armed {
                r.factories += 1;
                r.target_factories += usize::from(value == r.target);
                if value == U256::ONE {
                    r.nonce_factories += 1;
                    fresh_nonce = true;
                    clone_fee = r.fresh_nonce_fee;
                }
            }
        });
        Self {
            native: NumericValue::constant(value),
            fresh_nonce,
            clone_fee,
        }
    }
    fn from_observation(value: &WordObservation) -> Self {
        Self {
            native: NumericValue::from_observation(value),
            fresh_nonce: false,
            clone_fee: 64,
        }
    }
    fn observation(&self) -> WordObservation {
        self.native.observation()
    }
    fn singleton(&self) -> Option<U256> {
        self.native.singleton()
    }
    fn contains(&self, value: U256) -> bool {
        self.native.contains(value)
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.native.constants()
    }
    fn is_top(&self) -> bool {
        self.native.is_top()
    }
    fn work_size(&self) -> usize {
        self.native.work_size().saturating_add(16)
    }
    fn clone_work(&self) -> usize {
        if self.fresh_nonce {
            RECEIPT.update(|r| r.fresh_nonce_quotes += 1);
        }
        self.clone_fee
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct Policy {
    pub inner: ProductAlgebra,
    pub target: U256,
    pub target_fee: usize,
    pub top_fee: usize,
    pub cancel: Option<analysis_control::Cancellation>,
}

macro_rules! native_algebra {
    () => {
        type Element = Word;
        fn spec(&self) -> DomainSpec {
            self.inner().spec()
        }
        fn project_numeric(&self, value: &Word) -> Word {
            value.clone()
        }
        fn join_numeric(&self, left: &Word, right: &Word) -> Word {
            Word {
                native: self.inner().join_numeric(&left.native, &right.native),
                fresh_nonce: false,
                clone_fee: 64,
            }
        }
        fn widen_numeric(&self, left: &Word, right: &Word) -> Word {
            self.join_numeric(left, right)
        }
        fn widen_procedure_numeric(&self, left: &Word, right: &Word) -> Word {
            self.join_numeric(left, right)
        }
        fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
            NumericReduction::stable(value.clone())
        }
        fn apply_numeric<'a>(
            &self,
            op: u8,
            args: impl ExactSizeIterator<Item = &'a Word>,
        ) -> NumericReduction<Word> {
            NumericReduction::stable(Word {
                native: self
                    .inner()
                    .apply_numeric(op, args.map(|value| &value.native))
                    .value,
                fresh_nonce: false,
                clone_fee: 64,
            })
        }
        fn operation_work<'a>(&self, op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
            self.inner()
                .operation_work(op, args.map(|value| &value.native))
        }
        fn projection_work(&self, value: &Word) -> usize {
            value.clone_work()
        }
        fn refinement_work(&self, value: &Word, facts: usize) -> usize {
            value.work_size().saturating_add(facts)
        }
    };
}

impl Policy {
    fn inner(&self) -> &ProductAlgebra {
        &self.inner
    }
}
impl EvmNumericAlgebra for Policy {
    native_algebra!();
    fn literal_quote_work(&self, value: U256) -> usize {
        if value == self.target {
            RECEIPT.update(|r| r.target_quotes += 1);
            if let Some(cancel) = &self.cancel {
                cancel.cancel();
            }
            self.target_fee
        } else {
            128
        }
    }
    fn summary_construction_work(&self, construction: SummaryConstruction) -> usize {
        if matches!(construction, SummaryConstruction::Top) {
            RECEIPT.update(|r| r.top_quotes += 1);
            self.top_fee
        } else {
            self.inner.summary_construction_work(construction)
        }
    }
    fn summary_join_work(&self, left: &Word, right: &Word, widen: bool) -> usize {
        self.inner
            .summary_join_work(&left.native, &right.native, widen)
    }
}

#[derive(Debug)]
pub(in crate::analysis::transfer) struct NoFactoryPrices(pub ProductAlgebra);
impl NoFactoryPrices {
    fn inner(&self) -> &ProductAlgebra {
        &self.0
    }
}
impl EvmNumericAlgebra for NoFactoryPrices {
    native_algebra!();
    // Deliberately retain the actual default MAX literal/construction quotes.
}

pub(in crate::analysis::transfer) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}
pub(in crate::analysis::transfer) fn value(value: u64) -> EvmValue<Word> {
    EvmValue::constant(U256::from(value))
}

pub(in crate::analysis::transfer) fn fixture() -> (
    EvmWorld<Word>,
    ExecutionConfig,
    ValueAlgebra<Policy>,
    EvmMachinePayload<Word>,
) {
    let owner = address(0x101);
    let target = U256::from_be_slice(owner.create(0).as_slice());
    let domain = ValueAlgebra::new(Policy {
        inner: ProductAlgebra::from_spec(DomainSpec::default()),
        target,
        target_fee: 128,
        top_fee: 128,
        cancel: None,
    });
    let mut world = EvmWorld::new(Fork::Osaka, "native creation payer");
    let mut creator = EvmAccount::from_hex("f000", world.fork()).unwrap();
    creator.balance = value(100);
    creator.nonce = value(0);
    world.insert(owner, creator).unwrap();
    world.insert(owner.create(0), EvmAccount::absent()).unwrap();
    let entry = EvmEntry::new(owner);
    let payload = initial(&world, &entry, domain.borrowed()).unwrap();
    (world, ExecutionConfig::default(), domain, payload)
}

pub(in crate::analysis::transfer) fn arm(target: U256, fresh_nonce_fee: usize) {
    RECEIPT.set(Receipt {
        armed: true,
        target,
        fresh_nonce_fee,
        ..Receipt::default()
    });
}

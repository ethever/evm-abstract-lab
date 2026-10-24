//! A cheap Top factory can return privately owned, expensive copies/queries.

use super::super::{EvmStoreSummary, InitialIndependence, OrderedMap, SummaryPlane};
use crate::{
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue, ProductAlgebra,
        SummaryConstruction, ValueAlgebra, WordElement, WordObservation,
    },
    resource::{CloneError, SummaryError, WorkBudget},
};
use alloy_primitives::U256;
use serde::Serialize;
use std::{collections::BTreeSet, fmt};

#[derive(Clone, Copy)]
struct Counts {
    copy_fee: usize,
    query_fee: usize,
    constructors: usize,
    copies: usize,
    queries: usize,
}
static COUNTS: crate::test_instrumentation::Counter<Counts> =
    crate::test_instrumentation::Counter::new(Counts {
        copy_fee: 32,
        query_fee: 32,
        constructors: 0,
        copies: 0,
        queries: 0,
    });

#[derive(Debug, Serialize)]
struct Word {
    numeric: NumericValue,
    private: Vec<u8>,
    copy_fee: usize,
    query_fee: usize,
}
impl Clone for Word {
    fn clone(&self) -> Self {
        if !self.private.is_empty() {
            COUNTS.update(|counts| counts.copies += 1);
        }
        Self {
            numeric: self.numeric.clone(),
            private: self.private.clone(),
            copy_fee: self.copy_fee,
            query_fee: self.query_fee,
        }
    }
}
impl PartialEq for Word {
    fn eq(&self, other: &Self) -> bool {
        self.numeric == other.numeric
            && self.private == other.private
            && self.copy_fee == other.copy_fee
            && self.query_fee == other.query_fee
    }
}
impl Eq for Word {}
impl fmt::Display for Word {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.numeric, out)
    }
}
impl WordElement for Word {
    fn top() -> Self {
        let config = COUNTS.get();
        COUNTS.update(|counts| counts.constructors += 1);
        Self {
            numeric: NumericValue::top(),
            private: vec![9; 64],
            copy_fee: config.copy_fee,
            query_fee: config.query_fee,
        }
    }
    fn constant(word: U256) -> Self {
        Self {
            numeric: NumericValue::constant(word),
            private: Vec::new(),
            copy_fee: 32,
            query_fee: 32,
        }
    }
    fn from_observation(observation: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(observation),
            private: Vec::new(),
            copy_fee: 32,
            query_fee: 32,
        }
    }
    fn observation(&self) -> WordObservation {
        self.numeric.observation()
    }
    fn singleton(&self) -> Option<U256> {
        if !self.private.is_empty() {
            COUNTS.update(|counts| counts.queries += 1);
        }
        self.numeric.singleton()
    }
    fn singleton_work(&self) -> usize {
        self.query_fee
    }
    fn contains(&self, word: U256) -> bool {
        self.numeric.contains(word)
    }
    fn constants(&self) -> Option<&BTreeSet<U256>> {
        self.numeric.constants()
    }
    fn is_top(&self) -> bool {
        self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        self.numeric
            .work_size()
            .saturating_add(self.private.len())
            .saturating_add(32)
    }
    fn clone_work(&self) -> usize {
        self.copy_fee
    }
}

#[derive(Debug)]
struct Policy(ProductAlgebra);
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
            numeric: self.0.join_numeric(&left.numeric, &right.numeric),
            private: Vec::new(),
            copy_fee: 32,
            query_fee: 32,
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
        _op: u8,
        _args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, _op: u8, _args: impl Iterator<Item = &'a Word>) -> usize {
        128
    }
    fn literal_quote_work(&self, _value: U256) -> usize {
        32
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.copy_fee
    }
    fn refinement_work(&self, value: &Word, _facts: usize) -> usize {
        value.copy_fee
    }
    fn summary_construction_work(&self, _kind: SummaryConstruction) -> usize {
        32
    }
    fn summary_equal_work(&self, left: &Word, right: &Word) -> usize {
        left.query_fee.saturating_add(right.query_fee)
    }
    fn summary_is_top_work(&self, value: &Word) -> usize {
        value.query_fee
    }
    fn summary_canonical_work(&self, value: &Word) -> usize {
        value.copy_fee
    }
    fn summary_compact_work(&self, value: &Word) -> usize {
        value.copy_fee
    }
    fn summary_restore_work(&self, value: &Word, _previous: &Word) -> usize {
        value.copy_fee
    }
    fn summary_observation_work(&self, value: &WordObservation) -> usize {
        value.work_size().saturating_mul(4).saturating_add(32)
    }
}

fn input() -> EvmStoreSummary<Word> {
    let zero = || EvmValue::<Word>::constant(U256::ZERO);
    let plane = || SummaryPlane {
        slots: OrderedMap::new(),
        defaults: OrderedMap::new(),
        global_default: zero(),
        initial_independence: InitialIndependence::new(false),
    };
    EvmStoreSummary {
        persistent: plane(),
        transient: plane(),
        balances: OrderedMap::new(),
        balance_default: zero(),
        codes: OrderedMap::new(),
        nonces: OrderedMap::new(),
        existence: OrderedMap::new(),
        created: OrderedMap::new(),
        created_default: Some(false),
        pending_destruction: OrderedMap::new(),
        pending_destruction_default: Some(false),
        touched: OrderedMap::new(),
        touched_default: Some(false),
        possible_logs: OrderedMap::new(),
        logs_unknown: false,
    }
}

#[test]
fn aliases_cover_both_storage_planes_nonce_and_created_lifecycle_before_rollback() {
    use crate::{
        analysis::ProcessOwner,
        bytecode::Program,
        world::{Code, EvmSnapshot},
    };
    use alloy_primitives::Address;
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    let a = Address::repeat_byte(0xa1);
    let u = Address::repeat_byte(0xb1);
    let destination = Address::repeat_byte(0xc1);
    let names = BTreeSet::from([a, u, destination]);
    let mut initial = input();
    for owner in [a, u] {
        initial.codes.insert(
            owner,
            Code::Runtime(Program::decode(&[revm_bytecode::opcode::STOP]).unwrap()),
        );
        initial.nonces.insert(owner, EvmValue::constant(U256::ZERO));
        initial
            .persistent
            .slots
            .insert((owner, U256::ZERO), EvmValue::constant(U256::from(5)));
        initial
            .transient
            .slots
            .insert((owner, U256::ZERO), EvmValue::constant(U256::ZERO));
    }
    let mut budget = WorkBudget::new(10_000_000);
    let outer = initial
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    // An actually initialized fresh constructor is a legal created owner;
    // marking the pre-existing root created would be an inconsistent witness.
    let checkpoint = initial
        .process_begin_constructor(
            ProcessOwner::Named(a),
            ProcessOwner::Named(destination),
            domain.borrowed(),
            &mut budget,
        )
        .unwrap();
    let child_checkpoint = checkpoint
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    for writer in [a, u, destination] {
        let effect = checkpoint
            .process_effects_with_payment(
                &names,
                Some(writer),
                true,
                false,
                domain.borrowed(),
                &mut budget,
            )
            .unwrap();
        let mut current = effect
            .instantiate_with_payment(domain.borrowed(), &mut budget)
            .unwrap();
        for owner in [a, u] {
            assert!(
                current
                    .read(owner, &EvmValue::constant(U256::ZERO), domain.borrowed())
                    .contains(U256::from(11))
            );
            assert!(
                current
                    .read_transient(owner, &EvmValue::constant(U256::ZERO), domain.borrowed())
                    .contains(U256::from(11))
            );
            assert!(current.nonce(owner).contains(U256::from(7)));
            assert_eq!(current.created_in_transaction(owner), Some(false));
        }
        assert_eq!(current.created_in_transaction(destination), Some(true));
        assert_eq!(
            current.pending_destruction(destination),
            None,
            "possible alias must not keep false destruction"
        );
        assert!(current.nonce(destination).contains(U256::ONE));
        assert!(effect.logs_unknown);
        current.restore(EvmSnapshot::from_state(child_checkpoint.clone()));
        assert_eq!(current.nonce(destination).singleton(), Some(U256::ONE));
        assert_eq!(current.created_in_transaction(destination), Some(true));
        assert_eq!(current.pending_destruction(destination), Some(false));
        current.restore(EvmSnapshot::from_state(outer.clone()));
        for owner in [a, u] {
            assert_eq!(
                current
                    .read(owner, &EvmValue::constant(U256::ZERO), domain.borrowed())
                    .singleton(),
                Some(U256::from(5))
            );
            assert_eq!(
                current
                    .read_transient(owner, &EvmValue::constant(U256::ZERO), domain.borrowed())
                    .singleton(),
                Some(U256::ZERO)
            );
            assert_eq!(current.nonce(owner).singleton(), Some(U256::ZERO));
        }
        assert_eq!(current.created_in_transaction(destination), Some(false));
        assert_eq!(current.pending_destruction(destination), Some(false));
    }
    let unchanged = checkpoint
        .process_effects_with_payment(&names, Some(a), true, true, domain.borrowed(), &mut budget)
        .unwrap();
    assert_eq!(unchanged.persistent, checkpoint.persistent);
    assert_eq!(unchanged.transient, checkpoint.transient);
    assert_eq!(unchanged.nonces, checkpoint.nonces);
    assert_eq!(unchanged.codes, checkpoint.codes);
    assert_eq!(unchanged.created, checkpoint.created);
    assert_eq!(
        unchanged.pending_destruction,
        checkpoint.pending_destruction
    );
}

#[test]
fn cheap_constructor_cannot_prepay_its_actual_high_private_result_copy() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    let original = input();
    COUNTS.set(Counts {
        copy_fee: 1_000_000,
        query_fee: 32,
        constructors: 0,
        copies: 0,
        queries: 0,
    });
    let mut budget = WorkBudget::new(20_000);
    let staged = original
        .process_effects_with_payment(
            &BTreeSet::new(),
            None,
            false,
            false,
            domain.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert!(
        COUNTS.get().constructors >= 3,
        "the actual returned Tops were constructed"
    );
    assert_eq!(COUNTS.get().copies, 0);
    assert!(matches!(
        staged.instantiate_with_payment(domain.borrowed(), &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    ));
    assert_eq!(
        COUNTS.get().copies,
        0,
        "refused returned Top was never cloned"
    );
    assert_eq!(COUNTS.get().queries, 0);
    assert!(original.persistent.global_default.contains(U256::ZERO));
    assert!(!original.persistent.global_default.contains(U256::ONE));
}

#[test]
fn paid_result_copy_does_not_prepay_its_actual_high_query() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    COUNTS.set(Counts {
        copy_fee: 32,
        query_fee: 1_000_000,
        constructors: 0,
        copies: 0,
        queries: 0,
    });
    let mut budget = WorkBudget::new(20_000);
    let staged = input()
        .process_effects_with_payment(
            &BTreeSet::new(),
            None,
            false,
            false,
            domain.borrowed(),
            &mut budget,
        )
        .unwrap();
    assert!(matches!(
        staged.instantiate_with_payment(domain.borrowed(), &mut budget),
        Err(SummaryError::Budget(CloneError::Work))
    ));
    assert!(
        COUNTS.get().copies > 0,
        "the cheap actual Top copy executed"
    );
    assert_eq!(
        COUNTS.get().queries,
        0,
        "its independently expensive query was refused"
    );
}

#[test]
fn funded_actual_result_hooks_run_and_original_cancellation_stays_typed() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    COUNTS.set(Counts {
        copy_fee: 3_000,
        query_fee: 4_000,
        constructors: 0,
        copies: 0,
        queries: 0,
    });
    let mut budget = WorkBudget::new(1_000_000);
    let staged = input()
        .process_effects_with_payment(
            &BTreeSet::new(),
            None,
            false,
            false,
            domain.borrowed(),
            &mut budget,
        )
        .unwrap();
    let store = staged
        .instantiate_with_payment(domain.borrowed(), &mut budget)
        .unwrap();
    assert!(COUNTS.get().copies > 0 && COUNTS.get().queries > 0);
    assert!(budget.used() >= COUNTS.get().copies.saturating_mul(3_000));
    assert!(store.global_storage_default().contains(U256::MAX));
    let payer = analysis_control::Cancellation::new();
    let mut cancelled = analysis_control::with_cancellation(&payer, || WorkBudget::new(1_000_000));
    payer.cancel();
    assert!(matches!(
        staged.process_effects_with_payment(
            &BTreeSet::new(),
            None,
            false,
            false,
            domain.borrowed(),
            &mut cancelled
        ),
        Err(CloneError::Cancelled)
    ));
}

#[test]
fn constructor_transfer_requotes_its_actual_new_balance_top() {
    let _scope = COUNTS.scope();
    let domain = ValueAlgebra::new(Policy(ProductAlgebra::default()));
    COUNTS.set(Counts {
        copy_fee: 1_000_000,
        query_fee: 32,
        constructors: 0,
        copies: 0,
        queries: 0,
    });
    let mut budget = WorkBudget::new(20_000);
    let creator = alloy_primitives::Address::from_word(U256::from(0x101).into());
    let destination = alloy_primitives::Address::from_word(U256::from(0x201).into());
    let result = input().process_begin_constructor(
        crate::analysis::ProcessOwner::Named(creator),
        crate::analysis::ProcessOwner::Named(destination),
        domain.borrowed(),
        &mut budget,
    );
    assert!(matches!(result, Err(CloneError::Work)));
    assert!(
        COUNTS.get().constructors >= 2,
        "both actual balance Tops were constructed"
    );
    assert_eq!(
        COUNTS.get().copies,
        0,
        "the old input invoice cannot pay the new balance clone"
    );
}

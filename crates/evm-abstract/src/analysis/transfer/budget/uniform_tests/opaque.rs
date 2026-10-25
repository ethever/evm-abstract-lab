//! Uniformity is a law of the actual carrier, including unexported predicates.

use super::{
    Cell, DomainSpec, EvmAccount, EvmEntry, EvmNumericAlgebra, EvmValue, EvmWorld, ExecutionConfig,
    ExprId, Fork, FrontierReason, NumericReduction, NumericValue, ProductAlgebra, Program,
    RecentDistinctJumps, U256, ValueAlgebra, WorkBudget, address, opcode, operation_work_budgeted,
    transfer, writer_work,
};
use crate::{
    domain::{WordElement, WordObservation},
    test_instrumentation::Counter,
    world::EvmByteArray,
};
use serde::Serialize;
use std::fmt;

static CLONES: Counter<usize> = Counter::new(0);
static LITERALS: Counter<usize> = Counter::new(0);
static QUERIES: Counter<usize> = Counter::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
enum Hidden {
    None,
    OddWord,
    AlternatingWord,
    OddByte,
    EvenByte,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
struct Word {
    numeric: NumericValue,
    hidden: Hidden,
    payload: Vec<u8>,
}

impl Clone for Word {
    fn clone(&self) -> Self {
        CLONES.update(|n| *n += 1);
        Self {
            numeric: self.numeric.clone(),
            hidden: self.hidden,
            payload: self.payload.clone(),
        }
    }
}

impl fmt::Display for Word {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.numeric.fmt(out)
    }
}

impl WordElement for Word {
    fn top() -> Self {
        Self::from_observation(&WordObservation::top())
    }
    fn constant(n: U256) -> Self {
        LITERALS.update(|count| *count += 1);
        Self {
            numeric: NumericValue::constant(n),
            hidden: Hidden::None,
            payload: Vec::new(),
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(facts),
            hidden: Hidden::None,
            payload: Vec::new(),
        }
    }
    fn observation(&self) -> WordObservation {
        // Parity of individual bytes cannot be recovered from this projection.
        self.numeric.observation()
    }
    fn finite_constants(&self) -> &crate::domain::FiniteConstantSet {
        self.numeric.finite_constants()
    }
    fn known_bits(&self) -> &crate::domain::known_bits::KnownBits {
        self.numeric.known_bits()
    }
    fn interval(&self) -> &crate::domain::interval::Interval {
        self.numeric.interval()
    }
    fn congruence(&self) -> &crate::domain::congruence::Congruence {
        self.numeric.congruence()
    }
    fn singleton(&self) -> Option<U256> {
        QUERIES.update(|n| *n += 1);
        self.numeric.singleton().filter(|n| self.contains(*n))
    }
    fn contains(&self, n: U256) -> bool {
        self.numeric.contains(n)
            && match self.hidden {
                Hidden::None => true,
                Hidden::OddWord => n.to_be_bytes::<32>().iter().all(|byte| byte & 1 == 1),
                Hidden::AlternatingWord => n
                    .to_be_bytes::<32>()
                    .iter()
                    .enumerate()
                    .all(|(i, byte)| usize::from(byte & 1) == i % 2),
                Hidden::OddByte => n.bit(0),
                Hidden::EvenByte => !n.bit(0),
            }
    }
    fn is_top(&self) -> bool {
        self.hidden == Hidden::None && self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        self.numeric.work_size().saturating_add(self.payload.len())
    }
    fn clone_work(&self) -> usize {
        self.numeric.clone_work().saturating_add(self.payload.len())
    }
}

// Neither Clone nor Copy; ValueAlgebra must retain this exact borrowed policy.
#[derive(Debug, Default)]
struct Policy {
    default: ProductAlgebra,
    applies: Cell<usize>,
    quoted_payloads: Cell<Option<(usize, usize)>>,
    refuse: bool,
    cancel: bool,
}

impl EvmNumericAlgebra for Policy {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        self.default.spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, left: &Word, right: &Word) -> Word {
        if left == right {
            left.clone()
        } else {
            Word::from_observation(
                &self
                    .default
                    .join_numeric(&left.numeric, &right.numeric)
                    .observation(),
            )
        }
    }
    fn widen_numeric(&self, _old: &Word, _next: &Word) -> Word {
        Word::top()
    }
    fn widen_procedure_numeric(&self, old: &Word, next: &Word) -> Word {
        self.widen_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &Word) -> NumericReduction<Word> {
        NumericReduction::stable(value.clone())
    }
    fn apply_numeric<'a>(
        &self,
        op: u8,
        mut args: impl ExactSizeIterator<Item = &'a Word>,
    ) -> NumericReduction<Word> {
        if op == opcode::BYTE && args.len() == 2 {
            self.applies.set(self.applies.get() + 1);
            if self.cancel {
                analysis_control::current_cancellation().cancel();
            }
            let index = args.next().unwrap().singleton();
            let word = args.next().unwrap();
            if let Some(i) = index
                .and_then(|i| usize::try_from(i).ok())
                .filter(|i| *i < 32)
            {
                let hidden = match word.hidden {
                    Hidden::OddWord => Hidden::OddByte,
                    Hidden::AlternatingWord if i % 2 == 0 => Hidden::EvenByte,
                    Hidden::AlternatingWord => Hidden::OddByte,
                    _ => Hidden::None,
                };
                return NumericReduction::stable(Word {
                    numeric: NumericValue::unknown_byte(),
                    hidden,
                    payload: word.payload.clone(),
                });
            }
        }
        NumericReduction::stable(Word::top())
    }
    fn operation_work<'a>(&self, _op: u8, args: impl Iterator<Item = &'a Word>) -> usize {
        args.fold(4096usize, |fee, value| {
            fee.saturating_add(value.work_size().saturating_mul(256))
        })
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        64
    }
    fn projection_work(&self, value: &Word) -> usize {
        value.clone_work()
    }
    fn refinement_work(&self, value: &Word, _facts: usize) -> usize {
        value.work_size().saturating_mul(256)
    }
    fn uniform_word_bytes(&self, word: &Word) -> bool {
        word.hidden == Hidden::OddWord
    }
    fn uniform_word_numeric_work(&self, word: &Word, length: &Word) -> usize {
        self.quoted_payloads
            .set(Some((word.payload.len(), length.payload.len())));
        if self.refuse {
            return usize::MAX;
        }
        // Complete numeric copies/queries, literals, BYTE results and length
        // joins. This test policy has no scalar reduction graph; 256 lifetimes
        // cover the bounded 32-cell writer and its temporary numeric values.
        word.work_size()
            .saturating_add(length.work_size())
            .saturating_add(4096)
            .saturating_mul(256)
    }
}

fn source(hidden: Hidden, payload: usize) -> EvmValue<Word> {
    EvmValue::from_numeric(Word {
        numeric: NumericValue::top(),
        hidden,
        payload: vec![7; payload],
    })
    .with_expression(ExprId::fresh().unwrap())
}

#[test]
fn opaque_quote_borrows_actual_elements_without_queries_literals_or_copies() {
    let _clones = CLONES.scope();
    let _literals = LITERALS.scope();
    let _queries = QUERIES.scope();
    let value = source(Hidden::OddWord, 8192);
    let offset = EvmValue::constant(U256::ZERO);
    let memory = EvmByteArray::memory();
    for refuse in [false, true] {
        let policy = Policy {
            refuse,
            ..Default::default()
        };
        let algebra = ValueAlgebra::new(&policy);
        CLONES.set(0);
        LITERALS.set(0);
        QUERIES.set(0);
        let work = memory
            .uniform_word_write_work(&offset, &value, algebra.borrowed())
            .unwrap();
        assert_eq!(work == usize::MAX, refuse);
        assert_eq!(policy.quoted_payloads.get(), Some((8192, 0)));
        assert_eq!((CLONES.get(), LITERALS.get(), QUERIES.get()), (0, 0, 0));
        assert_eq!(policy.applies.get(), 0);
    }
}

#[test]
fn opaque_uniform_write_retains_hidden_predicates_and_refuses_before_touch() {
    let policy = Policy::default();
    let algebra = ValueAlgebra::new(&policy);
    let domain = algebra.borrowed();
    let mut world = EvmWorld::new(Fork::Osaka, "opaque uniform BYTE");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("525b00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::new(address(0x101));
    let config = ExecutionConfig::default();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let value = source(Hidden::OddWord, 1024);
    let offset = EvmValue::constant(U256::ZERO);
    let mut payload = transfer::initial(&world, &entry, domain).unwrap();
    payload.active_mut().stack = vec![value.clone(), offset.clone()];
    let execution = transfer::empty_execution(payload.clone());
    let fee = operation_work_budgeted(
        &execution,
        opcode::MSTORE,
        &[offset, value],
        &program,
        domain,
        &config,
        &entry,
        &mut WorkBudget::new(config.max_work),
    )
    .map(writer_work)
    .unwrap();
    let mut expected = payload.clone();
    expected.active_mut().stack.clear();
    let mut refused = WorkBudget::new(fee - 1);
    let result = transfer::execute(
        &world,
        &entry,
        payload.clone(),
        &config,
        domain,
        &RecentDistinctJumps,
        &mut refused,
        transfer::StorageReadPolicy::Abstract,
    );
    assert_eq!(result.payload, expected);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(policy.applies.get(), 0);
    let mut budget = WorkBudget::new(config.max_work);
    let result = transfer::execute(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &RecentDistinctJumps,
        &mut budget,
        transfer::StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    assert_eq!(policy.applies.get(), 1);
    let memory = &result.payload.active().memory;
    assert_eq!(memory.stored_bytes().len(), 32);
    for (_, byte) in memory.stored_bytes() {
        assert_eq!(byte.numeric().hidden, Hidden::OddByte);
        // Independent concrete word witnesses have every byte odd. Check the
        // actual carrier and the distinction omitted by its public observation.
        for n in 0..=255u16 {
            assert_eq!(byte.contains(U256::from(n)), n & 1 == 1);
            assert!(byte.numeric().observation().contains(U256::from(n)));
        }
        assert_eq!(byte.numeric().payload.len(), 1024);
    }
}

#[test]
fn equal_observations_do_not_admit_hidden_nonuniform_bytes() {
    let policy = Policy::default();
    let algebra = ValueAlgebra::new(&policy);
    let domain = algebra.borrowed();
    let mut memory = EvmByteArray::memory();
    let offset = EvmValue::constant(U256::ZERO);
    let value = source(Hidden::AlternatingWord, 16);
    assert!(
        memory
            .uniform_word_write_work(&offset, &value, domain)
            .is_none()
    );
    memory.write_word(&offset, &value, domain).unwrap();
    assert_eq!(policy.applies.get(), 32);
    let mut observation = None;
    for (index, byte) in memory.stored_bytes() {
        let public = byte.numeric().observation();
        assert_eq!(observation.get_or_insert_with(|| public.clone()), &public);
        assert!(byte.contains(U256::from(index % 2)));
        assert!(!byte.contains(U256::from((index + 1) % 2)));
    }
}

#[test]
fn opaque_singleton_certificate_also_funds_the_provenance_byte_pass() {
    let policy = Policy::default();
    let algebra = ValueAlgebra::new(&policy);
    let domain = algebra.borrowed();
    let offset = EvmValue::constant(U256::ZERO);
    let concrete = U256::from_be_bytes([1; 32]);
    let mut value = source(Hidden::OddWord, 128);
    let mut word = value.numeric().clone();
    word.numeric = NumericValue::constant(concrete);
    value = value.with_numeric(word);
    let mut memory = EvmByteArray::memory();
    let fee = memory
        .uniform_word_write_work(&offset, &value, domain)
        .unwrap();
    let mut budget = WorkBudget::new(fee);
    assert!(budget.charge(fee));
    assert!(
        memory
            .write_word_budgeted(&offset, &value, domain, &mut budget)
            .unwrap()
    );
    assert!(
        policy.applies.get() >= 32,
        "retain provenance staging for all bytes"
    );
    for (_, byte) in memory.stored_bytes() {
        assert!(byte.contains(U256::ONE));
    }
    assert_eq!(budget.used(), fee);
}

#[test]
fn cancellation_during_opaque_byte_keeps_the_paid_write_unpublished() {
    let policy = Policy {
        cancel: true,
        ..Default::default()
    };
    let algebra = ValueAlgebra::new(&policy);
    let domain = algebra.borrowed();
    let offset = EvmValue::constant(U256::ZERO);
    let value = source(Hidden::OddWord, 128);
    let mut memory = EvmByteArray::memory();
    let before = memory.clone();
    let fee = memory
        .uniform_word_write_work(&offset, &value, domain)
        .unwrap();
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(fee);
        assert!(budget.charge(fee));
        assert_eq!(
            memory.write_word_budgeted(&offset, &value, domain, &mut budget),
            Err(crate::world::RangeError::Expression(
                crate::domain::symbolic::ExprError::Cancelled
            ))
        );
        assert!(budget.exhausted());
        assert_eq!(budget.used(), fee);
    });
    assert_eq!(memory, before);
    assert_eq!(policy.applies.get(), 1);
}

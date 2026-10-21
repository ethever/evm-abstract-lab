//! Real memory-quotation entry with an index-sensitive independent backend.

use super::{byte_work, operation_work_budgeted, quotation_work};
use crate::domain::EvmNumericAlgebra as _;
use crate::{
    Fork,
    analysis::{ExecutionConfig, machine::fixtures::address, transfer},
    bytecode::Program,
    domain::{
        DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue, ProductAlgebra,
        ValueAlgebra, WordElement, WordObservation, symbolic::ExprId,
    },
    resource::WorkBudget,
    test_instrumentation::Counter,
    world::{EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

static COPIES: Counter<usize> = Counter::new(0);
static LITERALS: Counter<usize> = Counter::new(0);
static INDICES: Counter<u32> = Counter::new(0);
static LITERAL_BYTES: Counter<usize> = Counter::new(0);
static APPLIES: Counter<usize> = Counter::new(0);
static APPLIED_INDICES: Counter<u32> = Counter::new(0);
static PUSH_LITERALS: Counter<usize> = Counter::new(0);
static PUSH_COPIES: Counter<usize> = Counter::new(0);

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
struct Word {
    numeric: NumericValue,
    payload: Vec<u8>,
}
impl Clone for Word {
    fn clone(&self) -> Self {
        COPIES.update(|copies| *copies += 1);
        if self.numeric.singleton() == Some(U256::from(0xbeef)) {
            PUSH_COPIES.update(|copies| *copies += 1);
        }
        Self {
            numeric: self.numeric.clone(),
            payload: self.payload.clone(),
        }
    }
}
impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.numeric, f)
    }
}
impl WordElement for Word {
    fn top() -> Self {
        Self {
            numeric: NumericValue::top(),
            payload: vec![],
        }
    }
    fn constant(word: U256) -> Self {
        LITERALS.update(|count| *count += 1);
        if word == U256::from(0xbeef) {
            PUSH_LITERALS.update(|count| *count += 1);
        }
        let mut bytes = 0;
        LITERAL_BYTES.update(|size| bytes = *size);
        Self {
            numeric: NumericValue::constant(word),
            payload: vec![0; bytes],
        }
    }
    fn from_observation(facts: &WordObservation) -> Self {
        Self {
            numeric: NumericValue::from_observation(facts),
            payload: vec![],
        }
    }
    fn observation(&self) -> WordObservation {
        self.numeric.observation()
    }
    fn constants(&self) -> Option<&std::collections::BTreeSet<U256>> {
        self.numeric.constants()
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
        self.numeric.singleton()
    }
    fn is_top(&self) -> bool {
        self.numeric.is_top()
    }
    fn work_size(&self) -> usize {
        64 + self.numeric.work_size() + self.payload.len()
    }
    fn singleton_work(&self) -> usize {
        self.numeric.singleton_work()
    }
}

#[derive(Debug)]
struct Indexed {
    literal_bytes: Option<usize>,
}
impl Indexed {
    fn projection_work(index: usize) -> usize {
        if index == 31 { 65_536 } else { 1024 }
    }
}
impl EvmNumericAlgebra for Indexed {
    type Element = Word;
    fn spec(&self) -> DomainSpec {
        ProductAlgebra::default().spec()
    }
    fn project_numeric(&self, value: &Word) -> Word {
        value.clone()
    }
    fn join_numeric(&self, _left: &Word, _right: &Word) -> Word {
        Word::top()
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
        let args: Vec<&Word> = args.collect();
        if op == opcode::BYTE && args.len() == 2 {
            let index = args[0]
                .singleton()
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(32);
            APPLIES.update(|calls| *calls += 1);
            if index < 32 {
                APPLIED_INDICES.update(|indices| *indices |= 1 << index);
            }
            // One projection needs a large independently owned representation;
            // every valid index still denotes an unconstrained byte.
            return NumericReduction::stable(Word {
                numeric: NumericValue::unknown_byte(),
                payload: if index == 31 {
                    vec![31; 32_768]
                } else {
                    vec![]
                },
            });
        }
        let numeric =
            ProductAlgebra::default().apply_numeric(op, args.iter().map(|value| &value.numeric));
        NumericReduction {
            value: Word {
                numeric: numeric.value,
                payload: vec![],
            },
            status: numeric.status,
            rounds: numeric.rounds,
            strengthened: numeric.strengthened,
        }
    }
    fn operation_work<'a>(&self, op: u8, mut args: impl Iterator<Item = &'a Word>) -> usize {
        // The facade may replace the native result with Word::constant. Its
        // declared heap construction/release fee is additional operation work.
        // The native reservation covers Word's fixed fields; adding two visits
        // to the declared heap also bounds the final numeric work_size.
        let literal = self.literal_quote_work(U256::ZERO);
        if literal == usize::MAX {
            return usize::MAX;
        }
        let native = if op == opcode::BYTE {
            if let Some(index) = args
                .next()
                .and_then(Word::singleton)
                .and_then(|word| usize::try_from(word).ok())
                .filter(|index| *index < 32)
            {
                INDICES.update(|indices| *indices |= 1 << index);
                Self::projection_work(index)
            } else {
                200_000
            }
        } else {
            args.fold(200_000usize, |work, value| {
                work.saturating_add(value.work_size())
            })
        };
        native.saturating_add(literal)
    }
    fn literal_quote_work(&self, _literal: U256) -> usize {
        self.literal_bytes.map_or(usize::MAX, |bytes| {
            64usize.saturating_add(bytes.saturating_mul(2))
        })
    }
    fn projection_work(&self, _value: &Word) -> usize {
        0
    }
    fn refinement_work(&self, value: &Word, _facts: usize) -> usize {
        value.work_size() * 16 + 1024
    }
}

#[test]
fn operation_quote_covers_facade_literal_heap_and_refuses_unknown_or_overflow() {
    let _heap = LITERAL_BYTES.scope();
    let _copies = COPIES.scope();
    let _literals = LITERALS.scope();
    let _applies = APPLIES.scope();
    LITERAL_BYTES.set(4096);
    let args = [
        EvmValue::<Word>::constant(U256::ZERO).with_expression(ExprId::constant(U256::ZERO)),
        EvmValue::<Word>::constant(U256::ZERO).with_expression(ExprId::constant(U256::ZERO)),
    ];
    LITERALS.set(0);
    let algebra = ValueAlgebra::new(Indexed {
        literal_bytes: Some(4096),
    });
    let native = algebra
        .numeric_algebra()
        .operation_work(opcode::BYTE, args.iter().map(EvmValue::numeric));
    assert_eq!(native, 1024 + 64 + 2 * 4096);
    let complete = algebra.operation_work(opcode::BYTE, &args);
    assert!(complete >= native);
    assert_eq!((LITERALS.get(), APPLIES.get(), COPIES.get()), (0, 0, 0));

    let mut short = WorkBudget::new(complete - 1);
    assert!(
        algebra
            .apply_budgeted(opcode::BYTE, &args, &mut short)
            .is_err()
    );
    assert_eq!(short.used(), 0);
    assert_eq!((LITERALS.get(), APPLIES.get(), COPIES.get()), (0, 0, 0));

    let mut paid = WorkBudget::new(complete);
    let result = algebra
        .apply_budgeted(opcode::BYTE, &args, &mut paid)
        .unwrap();
    assert_eq!(paid.used(), complete);
    assert!(!paid.exhausted());
    assert_eq!((LITERALS.get(), APPLIES.get(), COPIES.get()), (1, 1, 0));
    assert_eq!(result.value.singleton(), Some(U256::ZERO));
    assert_eq!(result.value.numeric().payload.len(), 4096);
    assert_eq!(result.value.numeric().work_size(), 4173);
    assert!(native >= result.value.numeric().work_size());

    for literal_bytes in [None, Some(usize::MAX / 2)] {
        let unknown = ValueAlgebra::new(Indexed { literal_bytes });
        LITERALS.set(0);
        APPLIES.set(0);
        assert_eq!(unknown.literal_quote_work(U256::ZERO), usize::MAX);
        assert_eq!(
            unknown
                .numeric_algebra()
                .operation_work(opcode::BYTE, args.iter().map(EvmValue::numeric)),
            usize::MAX
        );
        let mut maximum = WorkBudget::new(usize::MAX);
        assert!(
            unknown
                .apply_budgeted(opcode::BYTE, &args, &mut maximum)
                .is_err()
        );
        assert_eq!(maximum.used(), 0);
        assert_eq!((LITERALS.get(), APPLIES.get(), COPIES.get()), (0, 0, 0));
    }
}

#[test]
fn mstore_prices_every_real_index_and_refuses_before_quote_construction_or_deep_clone() {
    let _copies = COPIES.scope();
    let _literals = LITERALS.scope();
    let _indices = INDICES.scope();
    let algebra = ValueAlgebra::new(Indexed {
        literal_bytes: Some(0),
    });
    let domain = algebra.borrowed();
    let mut world = EvmWorld::<Word>::new(Fork::Osaka, "index-sensitive quotation");
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("525b00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::new(address(0x101));
    let payload = transfer::initial(&world, &entry, domain).unwrap();
    let execution = transfer::empty_execution(payload);
    let config = ExecutionConfig::default();
    let value = EvmValue::from_numeric(Word {
        numeric: NumericValue::constant(U256::from(7)),
        payload: vec![9; 512],
    })
    .with_expression(ExprId::fresh().unwrap());
    let offset = EvmValue::unsigned_range(U256::ZERO, U256::from(31))
        .unwrap()
        .with_expression(ExprId::fresh().unwrap());
    let args = [offset, value];
    let before = execution.payload.clone();
    let frame = execution.payload.active();
    let bytes = byte_work(
        &[&args[0], &args[1]],
        32,
        frame.memory.work_size() + frame.calldata.work_size() + frame.returndata.work_size(),
        domain,
    );
    // The contextual numeric prices and common copy/metadata envelope are
    // independent of the production batch implementation under test.
    let literal = EvmValue::<Word>::constant(U256::ZERO);
    let operands = algebra
        .numeric_algebra()
        .operand_work([&literal, &args[1]].into_iter());
    let passes = 2; // Opaque backend retains both possible writer passes.
    let symbolic = 3 * 256; // one quote header and two one-node operand expressions.
    let expected = bytes
        + passes
            * (32 * 64
                + 32 * args[1].clone_work()
                + (0..32)
                    // Each operation can also install its own zero-heap literal;
                    // this is separate from constructing the quoted index.
                    .map(|index| Indexed::projection_work(index) + 64 + operands + symbolic)
                    .sum::<usize>());
    let prepare = 32 * 64;
    for maximum in [prepare - 1, prepare] {
        COPIES.set(0);
        LITERALS.set(0);
        INDICES.set(0);
        let mut budget = WorkBudget::new(maximum);
        let result = operation_work_budgeted(
            &execution,
            opcode::MSTORE,
            &args,
            &program,
            domain,
            &config,
            &entry,
            &mut budget,
        );
        if maximum < prepare {
            assert!(result.is_err());
            assert_eq!(budget.used(), 0);
            assert_eq!(LITERALS.get(), 0);
            assert_eq!(INDICES.get(), 0);
        } else {
            assert_eq!(result.unwrap().work, expected);
            assert_eq!(budget.used(), prepare);
            assert!(!budget.charge(expected));
            assert_eq!(budget.used(), prepare);
            assert_eq!(LITERALS.get(), 32);
            assert_eq!(INDICES.get(), u32::MAX);
        }
        assert_eq!(COPIES.get(), 0);
        assert_eq!(execution.payload, before);
    }
    assert!(expected > bytes + 32 * 64 + 32 * (Indexed::projection_work(0) + operands));
    let mut full = WorkBudget::new(prepare + expected);
    let quoted = operation_work_budgeted(
        &execution,
        opcode::MSTORE,
        &args,
        &program,
        domain,
        &config,
        &entry,
        &mut full,
    )
    .unwrap();
    assert!(full.charge(quoted.work));
    assert_eq!(full.used(), prepare + expected);
    assert!(!full.exhausted());
    let _applies = APPLIES.scope();
    let _applied_indices = APPLIED_INDICES.scope();
    let mut memory = execution.payload.active().memory.clone();
    memory.write_word(&args[0], &args[1], domain).unwrap();
    assert_eq!(APPLIES.get(), 64, "both real writer loops are covered");
    assert_eq!(APPLIED_INDICES.get(), u32::MAX);

    COPIES.set(0);
    LITERALS.set(0);
    let preparation = quotation_work(&execution, opcode::MSTORE8, &args, domain);
    assert_eq!(preparation, 64);
    let mut denied = WorkBudget::new(preparation - 1);
    assert!(
        operation_work_budgeted(
            &execution,
            opcode::MSTORE8,
            &args,
            &program,
            domain,
            &config,
            &entry,
            &mut denied
        )
        .is_err()
    );
    assert_eq!(LITERALS.get(), 0);
    assert_eq!(COPIES.get(), 0);
    assert_eq!(execution.payload, before);

    // An input can be tiny while its literal constructor owns a large heap.
    // Its separate declaration is paid before constructing the quote operand.
    let _heap = LITERAL_BYTES.scope();
    LITERAL_BYTES.set(4096);
    let expensive = ValueAlgebra::new(Indexed {
        literal_bytes: Some(4096),
    });
    let prepare = 64 + 2 * 4096;
    LITERALS.set(0);
    COPIES.set(0);
    let mut short = WorkBudget::new(prepare - 1);
    assert!(
        operation_work_budgeted(
            &execution,
            opcode::MSTORE8,
            &args,
            &program,
            expensive.borrowed(),
            &config,
            &entry,
            &mut short
        )
        .is_err()
    );
    assert_eq!(short.used(), 0);
    assert_eq!(LITERALS.get(), 0);
    assert_eq!(COPIES.get(), 0);
    let mut paid = WorkBudget::new(prepare);
    assert!(
        operation_work_budgeted(
            &execution,
            opcode::MSTORE8,
            &args,
            &program,
            expensive.borrowed(),
            &config,
            &entry,
            &mut paid
        )
        .is_ok()
    );
    assert_eq!(paid.used(), prepare);
    assert_eq!(LITERALS.get(), 1);
    assert_eq!(COPIES.get(), 0);
    let unknown = ValueAlgebra::new(Indexed {
        literal_bytes: None,
    });
    LITERALS.set(0);
    let mut maximum = WorkBudget::new(usize::MAX);
    assert!(
        operation_work_budgeted(
            &execution,
            opcode::MSTORE8,
            &args,
            &program,
            unknown.borrowed(),
            &config,
            &entry,
            &mut maximum
        )
        .is_err()
    );
    assert_eq!(maximum.used(), 0);
    assert_eq!(LITERALS.get(), 0);
    assert_eq!(COPIES.get(), 0);
    assert_eq!(execution.payload, before);
}

#[test]
fn push_reserves_the_declared_heap_constructor_before_literal_and_receipt_creation() {
    let _heap = LITERAL_BYTES.scope();
    let _constructors = PUSH_LITERALS.scope();
    let _copies = PUSH_COPIES.scope();
    let algebra = ValueAlgebra::new(Indexed {
        literal_bytes: Some(4096),
    });
    let domain = algebra.borrowed();
    let mut world = EvmWorld::<Word>::new(Fork::Osaka, "metered PUSH constructor");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("61beef00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::new(address(0x101));
    let original = transfer::initial(&world, &entry, domain).unwrap();
    let config = ExecutionConfig::default();
    let construction = 64 + 2 * 4096;
    let receipt = 8 + 64 + 13 + 4096 + 4;
    let admission = (usize::BITS as usize + 1) * 16 + 1; // untouched-RIPEMD lookup + opcode visit.
    LITERAL_BYTES.set(4096);
    for (maximum, prefix, constructed) in [
        (admission + construction - 1, admission, 0),
        (
            admission + construction + receipt - 1,
            admission + construction,
            1,
        ),
    ] {
        let payload = original.clone();
        PUSH_LITERALS.set(0);
        PUSH_COPIES.set(0);
        let mut budget = WorkBudget::new(maximum);
        let result = transfer::execute(
            &world,
            &entry,
            payload,
            &config,
            domain,
            &crate::analysis::engine::policy::RecentDistinctJumps,
            &mut budget,
            transfer::StorageReadPolicy::Abstract,
        );
        assert_eq!(budget.used(), prefix);
        assert_eq!(PUSH_LITERALS.get(), constructed);
        assert_eq!(PUSH_COPIES.get(), 0);
        assert!(result.payload.active().stack.is_empty());
        assert!(result.instruction_values.is_empty());
        assert_eq!(
            result.instruction_progress,
            [crate::analysis::InstructionProgress::Started]
        );
        assert_eq!(result.executed_pcs, [0]);
        assert!(result.frontiers.iter().any(|frontier| frontier.pc == 0
            && matches!(frontier.reason, crate::analysis::FrontierReason::Work)));
    }
    let unknown = ValueAlgebra::new(Indexed {
        literal_bytes: None,
    });
    let payload = original.clone();
    PUSH_LITERALS.set(0);
    PUSH_COPIES.set(0);
    let mut maximum = WorkBudget::new(usize::MAX);
    let result = transfer::execute(
        &world,
        &entry,
        payload,
        &config,
        unknown.borrowed(),
        &crate::analysis::engine::policy::RecentDistinctJumps,
        &mut maximum,
        transfer::StorageReadPolicy::Abstract,
    );
    assert_eq!(maximum.used(), admission);
    assert_eq!(PUSH_LITERALS.get(), 0);
    assert_eq!(PUSH_COPIES.get(), 0);
    assert!(result.instruction_values.is_empty());
    assert!(result.payload.active().stack.is_empty());
}

//! A more precise BYTE policy must keep its per-index facts and paid work.

use super::{operation_work_budgeted, quotation_work};
use crate::{
    Fork,
    analysis::{
        ExecutionConfig, FrontierReason, InstructionProgress, RecentDistinctJumps,
        machine::fixtures::address, transfer,
    },
    bytecode::Program,
    domain::{
        Domain, DomainSpec, EvmNumericAlgebra, EvmValue, NumericReduction, NumericValue,
        ValueAlgebra, symbolic::ExprId,
    },
    resource::WorkBudget,
    world::{ByteArray, EvmAccount, EvmEntry, EvmWorld},
};
use alloy_primitives::U256;
use revm_bytecode::opcode;
use std::cell::Cell;

const EXTRA_WORK: usize = 4096;

#[derive(Debug, Default)]
struct BytePolicy<const PRECISE: bool> {
    default: Domain,
    applies: Cell<usize>,
    quoted_indices: Cell<u32>,
    cancel_on_byte: bool,
    word_surcharge: Option<usize>,
}

impl<const PRECISE: bool> EvmNumericAlgebra for BytePolicy<PRECISE> {
    type Element = NumericValue;
    fn spec(&self) -> DomainSpec {
        self.default.spec()
    }
    fn project_numeric(&self, value: &NumericValue) -> NumericValue {
        self.default.project_numeric(value)
    }
    fn join_numeric(&self, left: &NumericValue, right: &NumericValue) -> NumericValue {
        self.default.join_numeric(left, right)
    }
    fn widen_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.default.widen_numeric(old, next)
    }
    fn widen_procedure_numeric(&self, old: &NumericValue, next: &NumericValue) -> NumericValue {
        self.default.widen_procedure_numeric(old, next)
    }
    fn reduce_numeric(&self, value: &NumericValue) -> NumericReduction<NumericValue> {
        self.default.reduce_numeric(value)
    }
    fn apply_numeric(&self, op: u8, args: &[NumericValue]) -> NumericReduction<NumericValue> {
        if op == opcode::BYTE && args.len() == 2 {
            self.applies.set(self.applies.get() + 1);
            if self.cancel_on_byte {
                analysis_control::current_cancellation().cancel();
            }
            if PRECISE
                && let Some(index) = args[0]
                    .singleton()
                    .and_then(|index| usize::try_from(index).ok())
                    .filter(|index| *index < 32)
            {
                let (lo, hi) = args[1].interval().unsigned_bounds();
                let shift = (31 - index) * 8;
                // Equal shifted endpoints prove every word in the interval has
                // this byte, independently of the default transfer algorithm.
                if lo >> shift == hi >> shift {
                    return NumericReduction::stable(NumericValue::constant(
                        (lo >> shift) & U256::from(255),
                    ));
                }
            }
        }
        self.default.apply_numeric(op, args)
    }
    fn operation_work<'a>(&self, args: impl Iterator<Item = &'a NumericValue>) -> usize {
        EvmNumericAlgebra::operation_work(&self.default, args).saturating_add(EXTRA_WORK)
    }
    fn operation_work_for<'a>(
        &self,
        op: u8,
        mut args: impl Iterator<Item = &'a NumericValue>,
    ) -> usize {
        let first = args.next();
        if op == opcode::BYTE
            && let Some(index) = first
                .and_then(NumericValue::singleton)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < 32)
        {
            self.quoted_indices
                .set(self.quoted_indices.get() | (1 << index));
        }
        self.operation_work(first.into_iter().chain(args))
    }
    fn literal_quote_work(&self, literal: U256) -> usize {
        self.default.literal_quote_work(literal)
    }
    fn uniform_word_bytes(&self, word: &NumericValue) -> bool {
        !PRECISE && self.default.uniform_word_bytes(word)
    }
    fn uniform_word_write_work(
        &self,
        value: &EvmValue<NumericValue>,
        length: &EvmValue<NumericValue>,
        default: &EvmValue<NumericValue>,
        allocations: usize,
    ) -> usize {
        if PRECISE {
            return usize::MAX;
        }
        self.word_surcharge.map_or(usize::MAX, |surcharge| {
            self.default
                .uniform_word_write_work(value, length, default, allocations)
                .saturating_add(EXTRA_WORK)
                .saturating_add(surcharge)
        })
    }
    fn projection_work(&self, value: &NumericValue) -> usize {
        self.default.projection_work(value)
    }
    fn refinement_work(&self, value: &NumericValue, facts: usize) -> usize {
        EvmNumericAlgebra::refinement_work(&self.default, value, facts)
    }
}

fn input() -> EvmValue<NumericValue> {
    EvmValue::from_numeric(
        NumericValue::unsigned_range(U256::from(0x100), U256::from(0x1ff)).unwrap(),
    )
    .with_expression(ExprId::fresh().unwrap())
}

fn native_word(value: &serde_json::Value) -> U256 {
    serde_json::from_value(value.clone()).unwrap()
}

fn assert_native_contains(byte: &serde_json::Value, concrete: U256) {
    if let Some(constants) = byte["Constants"].as_array() {
        assert!(constants.iter().any(|value| native_word(value) == concrete));
    }
    assert_eq!(
        concrete & native_word(&byte["known_bits"]["zero"]),
        U256::ZERO
    );
    let one = native_word(&byte["known_bits"]["one"]);
    assert_eq!(concrete & one, one);
    let interval = &byte["interval"];
    assert!(native_word(&interval["unsigned_lo"]) <= concrete);
    assert!(concrete <= native_word(&interval["unsigned_hi"]));
    let biased = concrete ^ (U256::ONE << 255);
    assert!(native_word(&interval["signed_lo"]) <= biased);
    assert!(biased <= native_word(&interval["signed_hi"]));
    let congruence = &byte["congruence"];
    if congruence != "Top" {
        if let Some(exact) = congruence.get("Exact") {
            assert_eq!(native_word(exact), concrete);
        } else {
            let modulo = &congruence["Modulo"];
            assert_eq!(
                concrete % native_word(&modulo["modulus"]),
                native_word(&modulo["residue"])
            );
        }
    }
    assert!(!byte["nonzero"].as_bool().unwrap() || concrete != U256::ZERO);
}

fn assert_witnesses(memory: &ByteArray) {
    let json = serde_json::to_value(memory).unwrap();
    assert_eq!(memory.stored_bytes().len(), 32);
    assert_eq!(json["bytes"].as_object().unwrap().len(), 32);
    // Check every concrete word against both public projections without using
    // either abstract transfer algorithm as the expected result.
    for witness in 0x100..=0x1ff {
        let concrete = U256::from(witness).to_be_bytes::<32>();
        for (index, stored) in memory.stored_bytes() {
            assert!(
                stored.contains(U256::from(concrete[index])),
                "word {witness:#x}, byte {index}, stored {stored}"
            );
            assert_native_contains(
                &json["bytes"][index.to_string()],
                U256::from(concrete[index]),
            );
        }
    }
}

#[test]
fn precise_byte_write_preserves_concrete_witnesses_and_native_serialized_facts() {
    let algebra = ValueAlgebra::new(BytePolicy::<true>::default());
    let domain = algebra.borrowed();
    let value = input();
    let offset = EvmValue::constant(U256::ZERO);
    let mut memory = ByteArray::memory();
    memory.write_word_with(&offset, &value, 96, domain).unwrap();
    let json = serde_json::to_value(&memory).unwrap();
    assert_witnesses(&memory);
    assert_eq!(json["bytes"]["30"]["Constants"], serde_json::json!(["0x1"]));
    assert_eq!(json["bytes"]["30"]["congruence"]["Exact"], "0x1");
    assert_eq!(algebra.numeric_algebra().applies.get(), 32);
    let stored = memory
        .stored_bytes()
        .find(|(index, _)| *index == 30)
        .unwrap()
        .1;
    assert_eq!(stored.singleton(), Some(U256::ONE));
    assert_eq!(memory.byte_at_with(30, domain).singleton(), Some(U256::ONE));
    let default = ValueAlgebra::new(Domain::default());
    let ordinary = default.apply(opcode::BYTE, &[EvmValue::constant(U256::from(30)), value]);
    assert!(ordinary.contains(U256::ONE));
    assert!(
        ordinary.singleton().is_none(),
        "retain the custom precision gain"
    );
}

#[test]
fn precise_first_mstore_quotes_real_indices_and_reserves_each_phase_once() {
    let algebra = ValueAlgebra::new(BytePolicy::<true>::default());
    let domain = algebra.borrowed();
    let mut world = EvmWorld::new(Fork::Osaka, "precise first MSTORE");
    world.insert(address(0x101), EvmAccount::empty()).unwrap();
    let entry = EvmEntry::new(address(0x101));
    let execution = transfer::empty_execution(transfer::initial(&world, &entry, domain).unwrap());
    let config = ExecutionConfig::default();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let args = [EvmValue::constant(U256::ZERO), input()];
    let before = execution.payload.clone();
    let prepare = quotation_work(&execution, opcode::MSTORE, &args, domain);
    assert_eq!(prepare, 32 * 64);
    assert!(
        execution
            .payload
            .active()
            .memory
            .uniform_word_write_work_with(&args[0], &args[1], domain)
            .is_none()
    );
    let quote = |budget: &mut WorkBudget| {
        operation_work_budgeted(
            &execution,
            opcode::MSTORE,
            &args,
            &program,
            domain,
            &config,
            &entry,
            budget,
        )
        .map(|quote| quote.work)
    };
    let mut refused = WorkBudget::new(prepare - 1);
    assert!(quote(&mut refused).is_err());
    assert_eq!(refused.used(), 0);
    assert_eq!(algebra.numeric_algebra().quoted_indices.get(), 0);
    assert_eq!(algebra.numeric_algebra().applies.get(), 0);
    let mut preparation_only = WorkBudget::new(prepare);
    let work = quote(&mut preparation_only).unwrap();
    assert_eq!(preparation_only.used(), prepare);
    assert_eq!(algebra.numeric_algebra().quoted_indices.get(), u32::MAX);
    assert!(work >= 32 * EXTRA_WORK);
    assert!(!preparation_only.charge(work));
    assert_eq!(preparation_only.used(), prepare);
    assert_eq!(execution.payload, before);

    let mut full = WorkBudget::new(prepare + work);
    assert_eq!(quote(&mut full).unwrap(), work);
    assert!(full.charge(work));
    let mut memory = execution.payload.active().memory.clone();
    // A separately funded ordinary refresh may decline its optional Reader
    // reuse at zero remaining work, then run the already-paid scalar path.
    assert!(
        memory
            .write_word_budgeted_with(&args[0], &args[1], 96, domain, &mut full)
            .unwrap()
    );
    assert_eq!(full.used(), prepare + work);
    assert!(!full.exhausted());
    assert_eq!(algebra.numeric_algebra().applies.get(), 32);
    assert_eq!(memory.byte_at_with(30, domain).singleton(), Some(U256::ONE));
}

fn assert_cancelled_write<const PRECISE: bool>() {
    let algebra = ValueAlgebra::new(BytePolicy::<PRECISE> {
        cancel_on_byte: true,
        word_surcharge: Some(100_000),
        ..Default::default()
    });
    let domain = algebra.borrowed();
    let offset = EvmValue::constant(U256::ZERO);
    let value = input();
    let config = ExecutionConfig::default();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let mut world = EvmWorld::new(Fork::Osaka, "cancel custom word write");
    world.insert(address(0x101), EvmAccount::empty()).unwrap();
    let entry = EvmEntry::new(address(0x101));
    let execution = transfer::empty_execution(transfer::initial(&world, &entry, domain).unwrap());
    let mut budget = WorkBudget::new(config.max_work);
    let work = operation_work_budgeted(
        &execution,
        opcode::MSTORE,
        &[offset.clone(), value.clone()],
        &program,
        domain,
        &config,
        &entry,
        &mut budget,
    )
    .map(|quote| quote.work)
    .unwrap();
    let total = budget.used() + work;
    let mut memory = ByteArray::memory();
    let frozen = memory.clone();
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(total);
        assert!(budget.charge(total));
        let result = memory.write_word_budgeted_with(&offset, &value, 96, domain, &mut budget);
        assert_eq!(
            result,
            Err(crate::world::RangeError::Expression(
                crate::domain::symbolic::ExprError::Cancelled,
            ))
        );
        assert!(token.is_cancelled());
        assert!(budget.exhausted());
        assert_eq!(
            budget.used(),
            total,
            "cancelled work retains its full payment"
        );
    });
    assert_eq!(algebra.numeric_algebra().applies.get(), 1);
    assert_eq!(memory, frozen);
}

#[test]
fn cancellation_during_byte_aborts_both_precise_and_certified_unpublished_writes() {
    assert_cancelled_write::<true>();
    assert_cancelled_write::<false>();
}

#[test]
fn uniform_certificate_uses_the_injected_full_fee_or_honestly_refuses() {
    let value = input();
    let args = [EvmValue::constant(U256::ZERO), value];
    let config = ExecutionConfig::default();
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let default_fee = ByteArray::memory()
        .uniform_word_write_work(&args[0], &args[1], Domain::default())
        .unwrap();
    for surcharge in [None, Some(usize::MAX), Some(100_000)] {
        let policy = BytePolicy::<false> {
            word_surcharge: surcharge,
            ..Default::default()
        };
        // Borrow the non-Copy policy twice to exercise reference delegation.
        let algebra = ValueAlgebra::new(&policy);
        let domain = algebra.borrowed();
        let mut world = EvmWorld::new(Fork::Osaka, "certified word fee");
        world.insert(address(0x101), EvmAccount::empty()).unwrap();
        let entry = EvmEntry::new(address(0x101));
        let execution =
            transfer::empty_execution(transfer::initial(&world, &entry, domain).unwrap());
        let before = execution.payload.clone();
        assert_eq!(quotation_work(&execution, opcode::MSTORE, &args, domain), 0);
        let quote = |budget: &mut WorkBudget| {
            operation_work_budgeted(
                &execution,
                opcode::MSTORE,
                &args,
                &program,
                domain,
                &config,
                &entry,
                budget,
            )
            .map(|quote| quote.work)
        };
        let mut budget = WorkBudget::new(config.max_work);
        let quoted = quote(&mut budget);
        assert_eq!(
            budget.used(),
            0,
            "cached quotation needs no temporary literals"
        );
        assert_eq!(policy.applies.get(), 0);
        assert_eq!(policy.quoted_indices.get(), 0);
        assert_eq!(execution.payload, before);
        if surcharge != Some(100_000) {
            assert!(
                quoted.is_err(),
                "unavailable or saturated fees refuse before effects"
            );
            let mut pure = ByteArray::memory();
            pure.write_word_with(&args[0], &args[1], 96, domain)
                .unwrap();
            assert_eq!(policy.applies.get(), 1, "pure callers fund their own work");
            assert_witnesses(&pure);
            continue;
        }
        let fee = quoted.unwrap();
        assert_eq!(fee, default_fee + EXTRA_WORK + 100_000);
        let mut refused = WorkBudget::new(fee - 1);
        assert_eq!(quote(&mut refused).unwrap(), fee);
        assert!(!refused.charge(fee));
        assert_eq!(refused.used(), 0);
        assert_eq!(policy.applies.get(), 0);
        let mut exact = WorkBudget::new(fee);
        assert_eq!(quote(&mut exact).unwrap(), fee);
        assert!(exact.charge(fee));
        let mut memory = execution.payload.active().memory.clone();
        assert!(
            memory
                .write_word_budgeted_with(&args[0], &args[1], 96, domain, &mut exact)
                .unwrap()
        );
        assert_eq!(
            exact.used(),
            fee,
            "the full writer fee has exactly one owner"
        );
        assert!(!exact.exhausted());
        assert_eq!(policy.applies.get(), 1, "retain the default fast path");
        assert_witnesses(&memory);
    }
}

fn assert_transfer_write<const PRECISE: bool>() {
    let policy = BytePolicy::<PRECISE> {
        word_surcharge: Some(100_000),
        ..Default::default()
    };
    let algebra = ValueAlgebra::new(&policy);
    let domain = algebra.borrowed();
    let config = ExecutionConfig::default();
    let mut world = EvmWorld::new(Fork::Osaka, "custom first MSTORE transfer");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("525b00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::new(address(0x101));
    let mut payload = transfer::initial(&world, &entry, domain).unwrap();
    let value = input();
    let offset = EvmValue::constant(U256::ZERO);
    payload.active_mut().stack = vec![value.clone(), offset.clone()];
    let execution = transfer::empty_execution(payload.clone());
    let program = Program::from_hex_with_fork("525b00", Fork::Osaka).unwrap();
    let mut quote_budget = WorkBudget::new(config.max_work);
    let fee = operation_work_budgeted(
        &execution,
        opcode::MSTORE,
        &[offset, value],
        &program,
        domain,
        &config,
        &entry,
        &mut quote_budget,
    )
    .map(|quote| quote.work)
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
    assert_eq!(result.payload, expected, "refusal precedes touch_memory");
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(policy.applies.get(), 0);
    assert!(refused.exhausted());
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
    assert_eq!(
        result.instruction_progress,
        [InstructionProgress::Completed]
    );
    assert_eq!(policy.applies.get(), if PRECISE { 32 } else { 1 });
    assert_witnesses(&result.payload.active().memory);
    if PRECISE {
        assert_eq!(
            result
                .payload
                .active()
                .memory
                .byte_at_with(30, domain)
                .singleton(),
            Some(U256::ONE)
        );
    }
}

#[test]
fn transfer_keeps_custom_admission_and_payment_stable_across_memory_touch() {
    assert_transfer_write::<true>();
    assert_transfer_write::<false>();
}

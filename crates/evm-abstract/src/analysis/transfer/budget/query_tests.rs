//! Quotation and executable dispatch with a carrier that has no finite view.

#[cfg(test)]
use crate::analysis::testing::TestValue;
use crate::domain::EvmNumericAlgebra as _;

use super::{arithmetic_work, byte_work, external_code_fee, operation_work_budgeted};
use crate::{
    Fork, U256,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::{
            self, EvmExecution,
            query::tests::{LITERALS, Policy, RECEIPT, Word},
        },
    },
    bytecode::Program,
    domain::{
        EvmValue, ProductAlgebra, SingletonProof, ValueAlgebra, WordElement, symbolic::ExprId,
    },
    resource::WorkBudget,
    world::{AddressInput, EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};
use alloy_primitives::{Address, keccak256};
use revm_bytecode::opcode;

const TARGET: u64 = 0x7171;
const CODE_BYTES: usize = 4096;

struct Fixture {
    world: EvmWorld<Word>,
    entry: EvmEntry<Word>,
    program: Program,
    config: ExecutionConfig,
    execution: EvmExecution<Word>,
}

fn fixture(op: u8) -> Fixture {
    let owner = Address::repeat_byte(0x11);
    let code = format!("{op:02x}5b00");
    let program = Program::from_hex_with_fork(&code, Fork::Osaka).unwrap();
    let mut world = EvmWorld::new(Fork::Osaka, "paid quote discovery");
    world
        .insert(owner, EvmAccount::from_hex(&code, Fork::Osaka).unwrap())
        .unwrap();
    world
        .insert(
            Address::from_word(U256::from(TARGET).into()),
            EvmAccount::from_hex(&"aa".repeat(CODE_BYTES), Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        owner,
        Address::ZERO,
        EvmValue::constant(U256::ZERO),
        EvmByteArray::empty(),
    );
    let algebra = ValueAlgebra::new(Policy);
    let execution =
        transfer::empty_execution(transfer::initial(&world, &entry, algebra.borrowed()).unwrap());
    Fixture {
        world,
        entry,
        program,
        config: ExecutionConfig::default(),
        execution,
    }
}

#[test]
fn optional_metadata_never_discovers_an_opaque_singleton() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let algebra = ValueAlgebra::new(Policy);
    for flagged in [false, true] {
        let mut value = EvmValue::<Word>::constant(U256::from(7));
        if flagged {
            value = value.with_symbolic_limit();
        }
        for result in [
            algebra.join(&value, &value),
            algebra.widen(&value, &value),
            algebra.widen_procedure(&value, &value),
            value.clone().with_numeric(value.numeric().clone()),
        ] {
            assert!(result.contains(U256::from(7)));
            assert!(!result.contains(U256::from(8)));
            assert_eq!(result.symbolic_limit_reached(), flagged);
        }
        let proved =
            value.with_numeric_after_singleton(Word::constant(U256::from(7)), Some(U256::from(7)));
        assert!(!proved.symbolic_limit_reached());
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
}

#[test]
fn cached_counts_and_transitive_byte_quotes_need_no_singleton_discovery() {
    let algebra = ValueAlgebra::new(Policy);
    let source = EvmByteArray::<Word>::exact(&[7; 64]);
    let memory = EvmByteArray::<Word>::memory();
    let value = EvmValue::<Word>::constant(U256::from(7));
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    assert_eq!(arithmetic_work(std::slice::from_ref(&value)), 1);
    let byte_fee = byte_work(&[&value, &value], 32, 64, algebra.borrowed());
    assert!(byte_fee > 32);
    assert!(source.slice_work(&value, &value, algebra.borrowed()) > 0);
    assert!(memory.copy_work(&source, &value, &value, &value, algebra.borrowed()) > 0);
    assert!(memory.copy_code_work(CODE_BYTES, 1, &value, &value, &value, algebra.borrowed()) > 0);
    assert!(memory.copy_return_data_work(&source, &value, &value, algebra.borrowed()) > 0);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));

    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let finite = domain.join(
        &TestValue::constant(U256::ONE),
        &TestValue::constant(U256::from(2)),
    );
    assert_eq!(
        arithmetic_work(&[finite.clone(), TestValue::top(), finite]),
        4
    );
}

#[test]
fn hidden_extent_is_paid_before_quote_and_saved_for_range_admission() {
    let fixture = fixture(opcode::KECCAK256);
    let algebra = ValueAlgebra::new(Policy);
    let args = [
        EvmValue::constant(U256::ZERO),
        EvmValue::constant(U256::from(CODE_BYTES)),
    ];
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = Some(U256::from(CODE_BYTES));
        receipt.fee = 100_003;
    });
    let mut budget = WorkBudget::new(10_000_000);
    assert!(budget.charge(17));
    let quote = operation_work_budgeted(
        &fixture.execution,
        opcode::KECCAK256,
        &args,
        &fixture.program,
        algebra.borrowed(),
        &fixture.config,
        &fixture.entry,
        &mut budget,
    )
    .unwrap();
    let frame = fixture.execution.payload.active();
    let retained =
        frame.memory.work_size() + frame.calldata.work_size() + frame.returndata.work_size();
    assert_eq!(
        quote.work,
        byte_work(
            &[&args[0], &args[1]],
            CODE_BYTES,
            retained,
            algebra.borrowed()
        )
    );
    assert!(quote.work > byte_work(&[&args[0], &args[1]], 0, retained, algebra.borrowed()));
    assert_eq!(
        quote.size,
        SingletonProof::Answer(Some(U256::from(CODE_BYTES)))
    );
    assert_eq!(
        (budget.used(), RECEIPT.get().quotes, RECEIPT.get().queries),
        (100_020, 1, 1)
    );
    assert!(budget.charge(quote.work));
    let mut saved = quote.size;
    assert_eq!(
        transfer::query::resolve(args[1].numeric(), &mut saved, &mut budget),
        Ok(Some(U256::from(CODE_BYTES)))
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
}

#[test]
fn literal_expression_extent_prices_real_hash_work_when_numeric_discovery_declines() {
    let mut fixture = fixture(opcode::KECCAK256);
    // Admitted by the actual symbolic folding limit, above the host extraction
    // cap: that separate cap cannot justify quoting zero byte work here.
    let bytes = 16;
    fixture.config.max_memory_bytes = 8;
    let source = vec![0xaa; bytes];
    fixture.execution.payload.active_mut().memory = EvmByteArray::exact(&source);
    let algebra = ValueAlgebra::new(Policy);
    let args = [
        EvmValue::constant(U256::ZERO),
        EvmValue::<Word>::top().with_expression(ExprId::constant(U256::from(bytes))),
    ];
    let _receipt = RECEIPT.scope();
    let _literals = LITERALS.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let mut budget = WorkBudget::new(10_000_000);
    let quote = operation_work_budgeted(
        &fixture.execution,
        opcode::KECCAK256,
        &args,
        &fixture.program,
        algebra.borrowed(),
        &fixture.config,
        &fixture.entry,
        &mut budget,
    )
    .unwrap();
    let frame = fixture.execution.payload.active();
    let retained =
        frame.memory.work_size() + frame.calldata.work_size() + frame.returndata.work_size();
    assert_eq!(
        quote.work,
        byte_work(&[&args[0], &args[1]], bytes, retained, algebra.borrowed())
    );
    assert!(quote.work > byte_work(&[&args[0], &args[1]], 0, retained, algebra.borrowed()));
    assert_eq!(quote.size, SingletonProof::Answer(Some(U256::from(bytes))));
    assert_eq!(
        (
            budget.used(),
            RECEIPT.get().quotes,
            RECEIPT.get().queries,
            LITERALS.get()
        ),
        (0, 0, 0, 0)
    );
    assert!(budget.charge(quote.work));
    // This is the actual symbolic hash consumer: it can materialize the exact
    // source from the literal view length without asking N for a singleton.
    let view = crate::domain::symbolic::ByteView::new(
        frame.memory.state_root().clone(),
        ExprId::constant(U256::ZERO),
        args[1].expression().unwrap().clone(),
    );
    let hash = ExprId::keccak(view, algebra.spec().relations().expressions()).unwrap();
    assert_eq!(
        hash.as_constant(),
        Some(U256::from_be_slice(keccak256(&source).as_slice()))
    );
}

#[test]
fn extent_and_target_discovery_refuse_max_or_insufficient_work_without_effects() {
    let algebra = ValueAlgebra::new(Policy);
    for op in [opcode::KECCAK256, opcode::EXTCODEHASH, opcode::EXTCODECOPY] {
        let fixture = fixture(op);
        let mut args = vec![EvmValue::constant(U256::from(TARGET))];
        if op == opcode::KECCAK256 {
            args = vec![
                EvmValue::constant(U256::ZERO),
                EvmValue::constant(U256::from(TARGET)),
            ];
        }
        if op == opcode::EXTCODECOPY {
            args.extend([
                EvmValue::constant(U256::ZERO),
                EvmValue::constant(U256::ZERO),
                EvmValue::constant(U256::ONE),
            ]);
        }
        let before = fixture.execution.payload.clone();
        let _receipt = RECEIPT.scope();
        for (fee, maximum) in [(100_003, 17 + 100_002), (usize::MAX, usize::MAX)] {
            RECEIPT.update(|receipt| {
                receipt.watched = Some(U256::from(TARGET));
                receipt.fee = fee;
            });
            let mut budget = WorkBudget::new(maximum);
            assert!(budget.charge(17));
            assert!(matches!(
                operation_work_budgeted(
                    &fixture.execution,
                    op,
                    &args,
                    &fixture.program,
                    algebra.borrowed(),
                    &fixture.config,
                    &fixture.entry,
                    &mut budget
                ),
                Err(FrontierReason::Work)
            ));
            assert_eq!(budget.used(), 17);
            assert_eq!(RECEIPT.get().queries, 0);
            assert_eq!(fixture.execution.payload, before);
        }
        assert_eq!(RECEIPT.get().quotes, 2);
    }
}

#[test]
fn hidden_external_target_prices_real_code_and_dispatch_reuses_the_cover() {
    let algebra = ValueAlgebra::new(Policy);
    let mut fixture = fixture(opcode::EXTCODEHASH);
    // Native identification keeps opaque inputs query-free; dispatch must
    // still reuse the one paid target answer with relations enabled.
    assert!(algebra.spec().relations().enabled);
    let args = [EvmValue::constant(U256::from(TARGET))];
    fixture.execution.payload.active_mut().stack = args.to_vec();
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = Some(U256::from(TARGET));
        receipt.fee = 100_003;
    });
    let mut budget = WorkBudget::new(10_000_000);
    let quote = operation_work_budgeted(
        &fixture.execution,
        opcode::EXTCODEHASH,
        &args,
        &fixture.program,
        algebra.borrowed(),
        &fixture.config,
        &fixture.entry,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        quote.work,
        external_code_fee(CODE_BYTES, 1, algebra.borrowed(), false)
    );
    assert!(quote.work > external_code_fee(0, 0, algebra.borrowed(), false));
    assert_eq!(
        quote
            .targets
            .as_ref()
            .unwrap()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        [U256::from(TARGET)]
    );
    assert_eq!(
        (budget.used(), RECEIPT.get().quotes, RECEIPT.get().queries),
        (100_003, 1, 1)
    );
    RECEIPT.update(|receipt| {
        receipt.quotes = 0;
        receipt.queries = 0;
    });
    let mut budget = WorkBudget::new(10_000_000);
    let result = transfer::execute(
        &fixture.world,
        &fixture.entry,
        fixture.execution.payload,
        &fixture.config,
        algebra.borrowed(),
        &crate::analysis::RecentDistinctJumps,
        &mut budget,
        transfer::StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    let expected = U256::from_be_slice(keccak256(vec![0xaa; CODE_BYTES]).as_slice());
    let result = &result.successors[0].payload.active().stack[0];
    assert_eq!(
        (result.numeric().lower, result.numeric().upper),
        (expected, expected)
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
}

#[test]
fn symbolic_self_quote_uses_host_address_without_constructing_or_querying_n() {
    let mut fixture = fixture(opcode::EXTCODEHASH);
    fixture.entry.environment.to = AddressInput::unknown_to();
    let scope = fixture.entry.environment.input_scope.id();
    let target =
        EvmValue::<Word>::unknown_address().with_symbol(crate::domain::identity::Symbol::To, scope);
    let args = [target.with_expression(ExprId::fresh().unwrap())];
    let algebra = ValueAlgebra::new(Policy);
    let _receipt = RECEIPT.scope();
    let _literals = LITERALS.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let mut budget = WorkBudget::new(usize::MAX);
    let quote = operation_work_budgeted(
        &fixture.execution,
        opcode::EXTCODEHASH,
        &args,
        &fixture.program,
        algebra.borrowed(),
        &fixture.config,
        &fixture.entry,
        &mut budget,
    )
    .unwrap();
    assert_eq!(
        quote.work,
        external_code_fee(3, 1, algebra.borrowed(), false)
    );
    assert_eq!(
        (
            budget.used(),
            RECEIPT.get().quotes,
            RECEIPT.get().queries,
            LITERALS.get()
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn quotation_preserves_cancellation_reason_and_original_ledger_payment() {
    let fixture = fixture(opcode::EXTCODEHASH);
    let before = fixture.execution.payload.clone();
    let algebra = ValueAlgebra::new(Policy);
    let _receipt = RECEIPT.scope();
    for cancel_owner in [false, true] {
        for stage in 0..3 {
            let owner = analysis_control::Cancellation::new();
            let active = analysis_control::Cancellation::new();
            let mut budget =
                analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
            assert!(budget.charge(17));
            let args = [EvmValue::from_numeric(Word {
                cancel_quote_owner: (cancel_owner && stage == 1).then(|| owner.clone()),
                cancel_query_owner: (cancel_owner && stage == 2).then(|| owner.clone()),
                ..Word::constant(U256::from(TARGET))
            })];
            RECEIPT.update(|receipt| {
                receipt.watched = Some(U256::from(TARGET));
                receipt.quotes = 0;
                receipt.queries = 0;
                receipt.cancel_on_quote = !cancel_owner && stage == 1;
                receipt.cancel_on_query = !cancel_owner && stage == 2;
            });
            if stage == 0 {
                if cancel_owner {
                    owner.cancel();
                } else {
                    active.cancel();
                }
            }
            analysis_control::with_cancellation(&active, || {
                assert!(matches!(
                    operation_work_budgeted(
                        &fixture.execution,
                        opcode::EXTCODEHASH,
                        &args,
                        &fixture.program,
                        algebra.borrowed(),
                        &fixture.config,
                        &fixture.entry,
                        &mut budget
                    ),
                    Err(FrontierReason::Relations(
                        crate::domain::relational::QueryReason::Cancelled
                    ))
                ));
            });
            assert_eq!(budget.used(), 17 + if stage == 2 { 61 } else { 0 });
            assert_eq!(
                (RECEIPT.get().quotes, RECEIPT.get().queries),
                (usize::from(stage != 0), usize::from(stage == 2))
            );
            assert_eq!(fixture.execution.payload, before);
        }
    }
}

#[test]
fn result_proof_tracks_expression_folding_without_a_second_query_fee() {
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.watched = None;
        receipt.fee = usize::MAX;
    });
    let algebra = ValueAlgebra::new(Policy);
    let args = [
        EvmValue::<Word>::top().with_expression(ExprId::constant(U256::from(31))),
        EvmValue::<Word>::top().with_expression(ExprId::constant(U256::ONE)),
    ];
    let fee = algebra.operation_work(opcode::ADD, &args);
    let mut budget = WorkBudget::new(fee);
    assert!(budget.charge(fee));
    let (result, proof) = algebra.apply_detailed_with_singleton(opcode::ADD, &args);
    assert_eq!(proof, SingletonProof::Answer(Some(U256::from(32))));
    assert_eq!(
        (result.value.numeric().lower, result.value.numeric().upper),
        (U256::from(32), U256::from(32))
    );
    assert_eq!((budget.used(), RECEIPT.get().quotes), (fee, 0));
    assert!(RECEIPT.get().queries > 0);
}

//! Real CALL admission must carry its paid private numeric answers into COPY.

use super::*;
use crate::{
    analysis::transfer::{
        empty_execution,
        query::tests::{Policy, RECEIPT, Word, context},
    },
    domain::{EvmValue, WordElement, provenance::Origin},
    world::{EvmAccount, EvmByteArray, EvmEntry, EvmWorld},
};

#[test]
fn paid_call_output_range_survives_continuation_and_completion_without_rediscovery() {
    let config = ExecutionConfig {
        max_work: 1_000_000_000,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(Policy);
    let value = |n: u64| EvmValue::<Word>::constant(U256::from(n));
    let mut world = EvmWorld::new(Fork::Osaka, "paid CALL output proof");
    world
        .insert(
            address(0x101),
            EvmAccount::from_hex("f100", Fork::Osaka).unwrap(),
        )
        .unwrap();
    world
        .insert(
            address(0x201),
            EvmAccount::from_hex("00", Fork::Osaka).unwrap(),
        )
        .unwrap();
    let entry = EvmEntry::concrete(
        address(0x101),
        Address::ZERO,
        value(0),
        EvmByteArray::empty(),
    );
    let input = initial(&world, &entry, algebra.borrowed()).unwrap();
    let program = input.active().program.clone().unwrap();
    let offset = value(0).with_origin(Origin::Memory).with_identity(91, 1);
    let size = EvmValue::from_numeric(Word {
        query_tag: Some(138),
        excluded: Some(U256::from(7)),
        ..Word::constant(U256::from(32))
    })
    .with_origin(Origin::Calldata)
    .with_identity(91, 2);
    assert!(size.constants().is_none());
    assert!(size.expression().is_none());
    let args = [
        value(1_000_000),
        value(0x201).with_expression(ExprId::constant(U256::from(0x201))),
        value(0),
        value(0),
        value(0),
        offset.clone(),
        size.clone(),
    ];
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|r| {
        r.watched = Some(U256::from(32));
        r.query_tag = Some(138);
        r.fee = 100_003;
    });
    let mut budget = WorkBudget::new(config.max_work);
    let mut result = empty_execution(input);
    call(
        &mut result,
        &world,
        opcode::CALL,
        &args,
        &program,
        &mut context(&config, &mut budget, algebra.borrowed()),
        0,
    );
    let child = result
        .successors
        .iter()
        .find(|s| {
            s.kind == MachineEdgeKind::Call && s.payload.active().key.code_address == address(0x201)
        })
        .expect("actual CALL must enter the callee")
        .payload
        .clone();
    let continuation = &child.call_stack.active_child().unwrap().continuation;
    for (prepared, original, word) in [
        (&continuation.output_offset, &offset, U256::ZERO),
        (&continuation.output_size, &size, U256::from(32)),
    ] {
        assert_eq!(prepared.numeric(), original.numeric());
        assert_eq!(prepared.provenance(), original.provenance());
        assert_eq!(prepared.identity(), original.identity());
        assert_eq!(
            prepared.expression().and_then(ExprId::as_constant),
            Some(word)
        );
    }
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    eprintln!(
        "CALL_ADMISSION used={} range_quotes=1 range_queries=1",
        budget.used()
    );
    let mut bytes = [0u8; 32];
    bytes[31] = 42;
    let mut data = EvmByteArray::exact(&bytes);
    data.retain_length_literal(ExprId::constant(U256::from(bytes.len())));
    let fee = child
        .call_stack
        .parent()
        .unwrap()
        .memory
        .copy_return_data_work_with(
            &data,
            &continuation.output_offset,
            &continuation.output_size,
            algebra.borrowed(),
        );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    for maximum in [fee - 1, fee] {
        RECEIPT.update(|r| {
            r.quotes = 0;
            r.queries = 0;
            r.fee = usize::MAX;
        });
        let mut result = empty_execution(child.clone());
        let mut budget = WorkBudget::new(maximum);
        finish(
            &mut result,
            child.clone(),
            OutcomeKind::Return,
            data.clone(),
            &mut context(&config, &mut budget, algebra.borrowed()),
            0,
        );
        assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
        assert_eq!(budget.used(), if maximum == fee { fee } else { 0 });
        eprintln!(
            "CALL_RESUME fee={fee} used={} queries=0 quotes=0 completed={}",
            budget.used(),
            result.completed_calls.len()
        );
        if maximum == fee {
            assert!(result.frontiers.is_empty());
            let resumed = result
                .successors
                .iter()
                .find(|s| s.kind == MachineEdgeKind::Return)
                .unwrap();
            assert_eq!(resumed.payload.active().returndata, data);
            // This reads the retained complete COPY, independent of its optional cache.
            assert_eq!(
                resumed
                    .payload
                    .active()
                    .memory
                    .read_word_with(&value(0), algebra.borrowed())
                    .singleton(),
                Some(U256::from(42))
            );
        } else {
            assert!(result.completed_calls.is_empty() && result.successors.is_empty());
            assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
        }
    }
}

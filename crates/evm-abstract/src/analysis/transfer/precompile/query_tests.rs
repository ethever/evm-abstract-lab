//! Native extraction pays independent length and returned-byte discoveries.

use super::materialize;
use crate::{
    U256,
    analysis::{
        ExecutionConfig, FrontierReason,
        transfer::query::tests::{Policy, RECEIPT, Word, context},
    },
    domain::{EvmValue, ValueAlgebra, relational::QueryReason, symbolic::ByteStateKind},
    resource::WorkBudget,
    world::EvmByteArray,
};

#[test]
fn native_length_query_pays_once_and_never_materializes_a_refused_answer() {
    let _receipt = RECEIPT.scope();
    let input = EvmByteArray::<Word>::exact(&[7; 7]);
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(Some(vec![7; 7]))
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));

    let mut limited = WorkBudget::new(60);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut limited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(
        (RECEIPT.get().quotes, RECEIPT.get().queries, limited.used()),
        (2, 1, 0)
    );
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut unlimited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(
        (
            RECEIPT.get().quotes,
            RECEIPT.get().queries,
            unlimited.used()
        ),
        (3, 1, 0)
    );

    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(1_000_000);
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(budget.used(), 61);
    });
}

#[test]
fn native_byte_query_pays_for_the_actual_opaque_read_result() {
    let config = ExecutionConfig::default();
    let algebra = ValueAlgebra::new(Policy);
    // Build a real lazy view through the public API, outside the measured call.
    let input = EvmByteArray::<Word>::exact(&[0, 7])
        .slice_with(
            &EvmValue::constant(U256::ONE),
            &EvmValue::constant(U256::ONE),
            config.max_memory_bytes,
            algebra.borrowed(),
        )
        .unwrap();
    assert!(!matches!(
        input.state_root().kind(),
        ByteStateKind::Exact(_)
    ));
    let _receipt = RECEIPT.scope();
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(Some(vec![7]))
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (1, 1));
    let paid_prefix = budget.used() - 61;

    let mut limited = WorkBudget::new(budget.used() - 1);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut limited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(limited.used(), paid_prefix);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (2, 1));
    RECEIPT.update(|receipt| receipt.fee = usize::MAX);
    let mut unlimited = WorkBudget::new(usize::MAX);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut unlimited, algebra.borrowed())
        ),
        Err(FrontierReason::Work)
    );
    assert_eq!(unlimited.used(), paid_prefix);
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (3, 1));

    RECEIPT.update(|receipt| {
        receipt.fee = 61;
        receipt.cancel_on_query = true;
    });
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut budget = WorkBudget::new(1_000_000);
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
        assert_eq!(budget.used(), paid_prefix + 61);
    });
}

#[test]
fn native_literal_length_can_decline_extraction_without_an_opaque_query() {
    let config = ExecutionConfig {
        max_memory_bytes: 6,
        ..ExecutionConfig::default()
    };
    let algebra = ValueAlgebra::new(Policy);
    let input = EvmByteArray::<Word>::exact(&[1; 8])
        .slice_with(
            &EvmValue::constant(U256::ZERO),
            &EvmValue::constant(U256::from(7)),
            config.max_memory_bytes,
            algebra.borrowed(),
        )
        .unwrap();
    let _receipt = RECEIPT.scope();
    RECEIPT.update(|receipt| {
        receipt.fee = usize::MAX;
        receipt.cancel_on_quote = true;
        receipt.cancel_on_query = true;
    });
    let mut budget = WorkBudget::new(1_000_000);
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut budget, algebra.borrowed())
        ),
        Ok(None)
    );
    assert_eq!((RECEIPT.get().quotes, RECEIPT.get().queries), (0, 0));
    let owner = analysis_control::Cancellation::new();
    let mut captured = analysis_control::with_cancellation(&owner, || WorkBudget::new(1_000_000));
    owner.cancel();
    assert_eq!(
        materialize(
            &input,
            &mut context(&config, &mut captured, algebra.borrowed())
        ),
        Err(FrontierReason::Relations(QueryReason::Cancelled))
    );
    assert_eq!(captured.used(), 0);
    let current = analysis_control::Cancellation::new();
    current.cancel();
    analysis_control::with_cancellation(&current, || {
        assert_eq!(
            materialize(
                &input,
                &mut context(&config, &mut budget, algebra.borrowed())
            ),
            Err(FrontierReason::Relations(QueryReason::Cancelled))
        );
    });
}

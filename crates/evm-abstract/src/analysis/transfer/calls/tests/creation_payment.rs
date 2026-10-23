use crate::{
    analysis::{
        EvmMachinePayload, FrontierReason, MachineEdgeKind, OutcomeKind,
        transfer::{
            EvmExecution,
            create::{
                create,
                test_support::{Policy, RECEIPT, Word, arm, fixture, value},
            },
            empty_execution,
            testing::context,
        },
    },
    domain::{DomainSpec, ProductAlgebra, ValueAlgebra},
    resource::WorkBudget,
    world::EvmByteArray,
};
use alloy_primitives::U256;
use revm_bytecode::opcode;

fn entered() -> (crate::analysis::ExecutionConfig, EvmMachinePayload<Word>) {
    let (world, config, domain, payload) = fixture();
    let program = payload.active().program.clone().unwrap();
    let mut result = empty_execution(payload);
    let args = [value(0), value(0), value(0)];
    let mut budget = WorkBudget::new(usize::MAX);
    create(
        &mut result,
        &world,
        opcode::CREATE,
        &args,
        &program,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    assert!(result.frontiers.is_empty(), "{:?}", result.frontiers);
    (
        config,
        result
            .successors
            .into_iter()
            .find(|edge| edge.kind == MachineEdgeKind::Call)
            .unwrap()
            .payload,
    )
}

fn complete(
    payload: EvmMachinePayload<Word>,
    kind: OutcomeKind,
    target: U256,
    fee: usize,
    maximum: usize,
    cancel: Option<analysis_control::Cancellation>,
) -> (EvmExecution<Word>, usize) {
    let domain = ValueAlgebra::new(Policy {
        inner: ProductAlgebra::from_spec(DomainSpec::default()),
        target,
        target_fee: fee,
        top_fee: 128,
        cancel,
    });
    let config = crate::analysis::ExecutionConfig::default();
    let mut result = empty_execution(payload.clone());
    let data = EvmByteArray::empty();
    let mut budget = WorkBudget::new(maximum);
    arm(target, 64);
    super::super::finish(
        &mut result,
        payload,
        kind,
        data,
        &mut context(&config, &mut budget, domain.borrowed()),
        0,
    );
    (result, budget.used())
}

#[test]
fn max_derived_address_literal_fee_reaches_actual_result_and_refuses_factory() {
    let _scope = RECEIPT.scope();
    let (_, payload) = entered();
    let address = payload
        .call_stack
        .active_child()
        .unwrap()
        .continuation
        .creation
        .unwrap()
        .native_address()
        .unwrap();
    let target = U256::from_be_slice(address.as_slice());
    let (result, _) = complete(
        payload,
        OutcomeKind::Return,
        target,
        usize::MAX,
        usize::MAX,
        None,
    );
    let receipt = RECEIPT.get();
    assert_eq!((receipt.target_quotes, receipt.target_factories), (1, 0));
    println!("native-created-address-MAX {receipt:?}");
    assert!(
        receipt.factories >= 2,
        "actual deployment empty-data factories precede the result quote"
    );
    assert!(result.completed_calls.is_empty() && result.successors.is_empty());
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn finite_address_literal_fee_is_exact_and_budget_refusal_publishes_no_resume() {
    let _scope = RECEIPT.scope();
    let (_, payload) = entered();
    let target = U256::from_be_slice(
        payload
            .call_stack
            .active_child()
            .unwrap()
            .continuation
            .creation
            .unwrap()
            .native_address()
            .unwrap()
            .as_slice(),
    );
    let (cheap, cheap_work) = complete(
        payload.clone(),
        OutcomeKind::Return,
        target,
        128,
        usize::MAX,
        None,
    );
    assert!(cheap.frontiers.is_empty());
    let (high, high_work) = complete(
        payload.clone(),
        OutcomeKind::Return,
        target,
        100_003,
        usize::MAX,
        None,
    );
    assert!(high.frontiers.is_empty(), "{:?}", high.frontiers);
    assert_eq!(high_work - cheap_work, 100_003 - 128);
    assert_eq!(
        (RECEIPT.get().target_quotes, RECEIPT.get().target_factories),
        (1, 1)
    );
    assert_eq!(
        high.successors[0]
            .payload
            .active()
            .stack
            .last()
            .unwrap()
            .singleton(),
        Some(target)
    );
    assert_eq!(high.completed_calls[0].output_kind, OutcomeKind::Return);
    let (refused, _) = complete(
        payload,
        OutcomeKind::Return,
        target,
        100_003,
        high_work - 1,
        None,
    );
    assert!(refused.completed_calls.is_empty() && refused.successors.is_empty());
    assert_eq!(refused.frontiers[0].reason, FrontierReason::Work);
}

#[test]
fn zero_creation_result_for_revert_and_failure_has_its_own_native_literal_quote() {
    let _scope = RECEIPT.scope();
    for kind in [OutcomeKind::Revert, OutcomeKind::Failure] {
        RECEIPT.set(Default::default());
        let (_, payload) = entered();
        let (result, _) = complete(payload, kind, U256::ZERO, usize::MAX, usize::MAX, None);
        assert_eq!(
            (RECEIPT.get().target_quotes, RECEIPT.get().target_factories),
            (1, 0)
        );
        assert_eq!(
            RECEIPT.get().factories,
            0,
            "no earlier unrelated factory may reject this control"
        );
        assert!(result.completed_calls.is_empty() && result.successors.is_empty());
        assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    }
}

#[test]
fn result_literal_quote_cancellation_checks_current_and_captured_owners() {
    let _scope = RECEIPT.scope();
    for cancel_captured in [false, true] {
        RECEIPT.set(Default::default());
        let (_, payload) = entered();
        let target = U256::from_be_slice(
            payload
                .call_stack
                .active_child()
                .unwrap()
                .continuation
                .creation
                .unwrap()
                .native_address()
                .unwrap()
                .as_slice(),
        );
        let captured = analysis_control::Cancellation::new();
        let current = analysis_control::Cancellation::new();
        let cancelled = if cancel_captured {
            captured.clone()
        } else {
            current.clone()
        };
        let domain = ValueAlgebra::new(Policy {
            inner: ProductAlgebra::from_spec(DomainSpec::default()),
            target,
            target_fee: 100_003,
            top_fee: 128,
            cancel: Some(cancelled),
        });
        let config = crate::analysis::ExecutionConfig::default();
        let mut result = empty_execution(payload.clone());
        let data = EvmByteArray::empty();
        let mut budget =
            analysis_control::with_cancellation(&captured, || WorkBudget::new(usize::MAX));
        arm(target, 64);
        analysis_control::with_cancellation(&current, || {
            super::super::finish(
                &mut result,
                payload,
                OutcomeKind::Return,
                data,
                &mut context(&config, &mut budget, domain.borrowed()),
                0,
            )
        });
        assert_eq!(
            (RECEIPT.get().target_quotes, RECEIPT.get().target_factories),
            (1, 0)
        );
        assert_eq!(captured.is_cancelled(), cancel_captured);
        assert_eq!(current.is_cancelled(), !cancel_captured);
        assert!(result.completed_calls.is_empty() && result.successors.is_empty());
        assert_eq!(
            result.frontiers[0].reason,
            FrontierReason::Relations(crate::domain::relational::QueryReason::Cancelled)
        );
    }
}

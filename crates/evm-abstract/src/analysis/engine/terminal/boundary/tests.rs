//! Admission needs actual guards or raw finite retention, with failure vetoes.

use super::Boundary;
use crate::domain::EvmNumericAlgebra as _;
use crate::{
    analysis::{
        FrontierReason, JoinBoundary, Limit, MachineError, RootTerminalRetention,
        RootTerminalTrigger,
        machine::fixtures,
        transfer::{Execution, ExecutionFrontier, RetentionCause},
    },
    domain::{ProductAlgebra, relational::QueryReason, symbolic::ExprError},
    resource::WorkBudget,
    world::rpc::{Failure, RpcContext, RpcFailureKind},
};

fn retention(cause: Option<RetentionCause>) -> Execution {
    Execution {
        payload: fixtures::payload(ProductAlgebra::default()),
        executed_pcs: Vec::new(),
        instruction_progress: Vec::new(),
        instruction_values: Vec::new(),
        successors: Vec::new(),
        diagnostics: Vec::new(),
        frontiers: vec![ExecutionFrontier {
            pc: 23,
            reason: FrontierReason::Relations(QueryReason::ExpressionLimit),
            target: None,
            range: None,
            payload: None,
        }],
        outcomes: Vec::new(),
        completed_calls: Vec::new(),
        retention_failures: cause.into_iter().map(|cause| (0, cause)).collect(),
    }
}

fn acquisition_failure() -> FrontierReason {
    FrontierReason::RpcAcquisition {
        address: fixtures::address(0x101),
        failure: Box::new(Failure {
            context: RpcContext {
                chain_id: None,
                block_hash: None,
                method: "eth_getCode",
                account: Some(fixtures::address(0x101)),
                slot: None,
            },
            kind: RpcFailureKind::Transport,
            cause: None,
            resource: None,
            limit: None,
            http_status: None,
            rpc_code: None,
            json_line: None,
            json_column: None,
            message: "isolated boundary witness".to_owned(),
        }),
    }
}

#[test]
fn work_reasons_require_refusal_on_the_original_ledger() {
    for (reason, expected) in [
        (FrontierReason::Work, RootTerminalTrigger::Work),
        (
            FrontierReason::Budget(Limit::Work),
            RootTerminalTrigger::Work,
        ),
        (
            FrontierReason::SummaryWork,
            RootTerminalTrigger::SummaryWork,
        ),
    ] {
        let mut budget = WorkBudget::new(1);
        let mut boundary = Boundary::new(true);
        boundary.observe(&reason, budget.exhausted(), true, true);
        assert!(boundary.trigger.is_none());
        assert!(!budget.charge(2));
        assert!(budget.exhausted());
        boundary.observe(&reason, budget.exhausted(), false, false);
        assert_eq!(boundary.into_trigger(), Some(expected));
    }
}

#[test]
fn counter_reasons_require_the_corresponding_reached_counter() {
    let mut states = Boundary::new(true);
    states.observe(&FrontierReason::Budget(Limit::States), true, false, true);
    assert!(states.trigger.is_none());
    states.observe(&FrontierReason::Budget(Limit::States), false, true, false);
    assert_eq!(states.into_trigger(), Some(RootTerminalTrigger::States));

    let mut transfers = Boundary::new(true);
    transfers.observe(&FrontierReason::Budget(Limit::Transfers), true, true, false);
    assert!(transfers.trigger.is_none());
    transfers.observe(
        &FrontierReason::Budget(Limit::Transfers),
        false,
        false,
        true,
    );
    assert_eq!(
        transfers.into_trigger(),
        Some(RootTerminalTrigger::Transfers)
    );
}

#[test]
fn only_raw_mandatory_retention_retains_finite_failure_identity() {
    let flattened = retention(None);
    let mut public_reason_only = Boundary::new(true);
    public_reason_only.observe(&flattened.frontiers[0].reason, true, true, true);
    public_reason_only.retention(17, &flattened);
    assert!(public_reason_only.into_trigger().is_none());
    for (raw, kind) in [
        (
            RetentionCause::Expression,
            RootTerminalRetention::Expression,
        ),
        (RetentionCause::State, RootTerminalRetention::State),
    ] {
        let mut boundary = Boundary::new(true);
        boundary.retention(17, &retention(Some(raw)));
        assert_eq!(
            boundary.into_trigger(),
            Some(RootTerminalTrigger::MandatoryRetention {
                state: 17,
                pc: 23,
                kind,
            })
        );
    }
    let mut invalid_index = retention(Some(RetentionCause::State));
    invalid_index.retention_failures[0].0 = 1;
    let mut boundary = Boundary::new(true);
    boundary.retention(17, &invalid_index);
    assert!(boundary.into_trigger().is_none());
}

#[test]
fn model_identity_and_optional_precision_reasons_never_mint_a_trigger() {
    for reason in [
        FrontierReason::UnsupportedOpcode(0xfb),
        FrontierReason::UnknownTarget,
        FrontierReason::MissingCode(fixtures::address(0x999)),
        FrontierReason::CallDepth,
        FrontierReason::RecursiveNoTerminal,
        FrontierReason::Memory,
        FrontierReason::Budget(Limit::CallDepth),
        FrontierReason::Budget(Limit::Memory),
        FrontierReason::Budget(Limit::Model),
        FrontierReason::StateJoin(JoinBoundary::IdentityExhausted),
        FrontierReason::Relations(QueryReason::ExpressionLimit),
        FrontierReason::Relations(QueryReason::ConstraintLimit),
        FrontierReason::Relations(QueryReason::ScalarFactLimit),
        MachineError::Expression(ExprError::Limit {
            nodes: usize::MAX,
            depth: usize::MAX,
        })
        .frontier_reason(),
    ] {
        let mut boundary = Boundary::new(true);
        boundary.observe(&reason, true, true, true);
        assert!(boundary.into_trigger().is_none(), "{reason:?}");
    }
}

#[test]
fn cancellation_and_acquisition_failure_veto_before_or_after_admissible_work() {
    for veto in [
        acquisition_failure(),
        FrontierReason::Relations(QueryReason::Cancelled),
    ] {
        for veto_first in [false, true] {
            let mut boundary = Boundary::new(true);
            if veto_first {
                boundary.observe(&veto, false, false, false);
            }
            boundary.observe(&FrontierReason::Work, true, false, false);
            if !veto_first {
                boundary.observe(&veto, false, false, false);
            }
            assert!(boundary.into_trigger().is_none(), "{veto:?}");
        }
    }
}

#[test]
fn disabled_capture_ignores_work_and_raw_retention() {
    let mut boundary = Boundary::new(false);
    boundary.observe(&FrontierReason::Work, true, true, true);
    boundary.retention(17, &retention(Some(RetentionCause::State)));
    assert!(boundary.into_trigger().is_none());
}

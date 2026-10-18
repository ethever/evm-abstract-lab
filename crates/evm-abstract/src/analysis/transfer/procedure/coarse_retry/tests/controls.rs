use super::*;
use crate::{
    analysis::JoinBoundary,
    domain::{
        relational::QueryReason,
        symbolic::{ExprError, ExprId, StateError},
    },
    world::{RangeError, StoreError},
};

#[test]
fn classifier_requires_a_finite_raw_limit_and_excludes_identity_sentinels() {
    assert_eq!(
        transfer::errors::retention_expression(&ExprError::Limit {
            nodes: 21,
            depth: 4
        }),
        Some(RetentionCause::Expression)
    );
    assert_eq!(
        transfer::errors::retention_bytes(&RangeError::State(StateError::Limit {
            nodes: 21,
            depth: 4
        })),
        Some(RetentionCause::State)
    );
    assert_eq!(
        transfer::errors::retention_store(&StoreError::State(StateError::Limit {
            nodes: 21,
            depth: 4
        })),
        Some(RetentionCause::State)
    );
    for (nodes, depth) in [(usize::MAX, usize::MAX), (usize::MAX, 1), (1, usize::MAX)] {
        assert_eq!(
            transfer::errors::retention_expression(&ExprError::Limit { nodes, depth }),
            None
        );
        assert_eq!(
            transfer::errors::retention_bytes(&RangeError::State(StateError::Limit {
                nodes,
                depth
            })),
            None
        );
    }
    let sentinel: ExprError = StateError::IdentityExhausted.into();
    assert_eq!(transfer::errors::retention_expression(&sentinel), None);
    for error in [
        ExprError::Cancelled,
        ExprError::Unsupported(0xff),
        ExprError::Arity {
            opcode: 1,
            expected: 2,
            actual: 1,
        },
        ExprError::ExportBytes {
            bytes: 21,
            limit: 20,
        },
    ] {
        assert_eq!(transfer::errors::retention_expression(&error), None);
    }
}

#[test]
fn semantic_identity_cancel_and_label_only_frontiers_cannot_start_a_retry() {
    for error in [
        RangeError::UnknownSize,
        RangeError::OffsetTooLarge(U256::MAX),
        RangeError::Overflow {
            offset: usize::MAX,
            size: 1,
        },
        RangeError::TooLarge {
            requested: U256::from(4097),
            limit: 4096,
        },
        RangeError::State(StateError::IdentityExhausted),
        RangeError::State(StateError::Cancelled),
        RangeError::Expression(StateError::IdentityExhausted.into()),
        RangeError::Expression(ExprError::Unsupported(0xff)),
    ] {
        let mut h = Harness::new("525b00");
        let input = instantiate(&h.table.nodes[0].payload, h.config.domain().unwrap()).unwrap();
        let mut execution = transfer::empty_execution(input);
        transfer::boundary_bytes(&mut execution, 0, error);
        assert!(execution.retention_failures.is_empty());
        let used = h.budget.used();
        let execution = h.retry(execution);
        assert_eq!(h.budget.used(), used);
        assert_eq!(h.counters.transfers, 0);
        assert_eq!(execution.frontiers.len(), 1);
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Fresh
        ));
    }
    // This is also the missing-guard/inherited-marker control: a public label
    // without a raw producer cause supplies no retry permission.
    let mut h = Harness::new("525b00");
    let input = instantiate(&h.table.nodes[0].payload, h.config.domain().unwrap()).unwrap();
    let mut execution = transfer::empty_execution(input);
    transfer::boundary(
        &mut execution,
        0,
        FrontierReason::Relations(QueryReason::ExpressionLimit),
    );
    let used = h.budget.used();
    assert_eq!(h.retry(execution).frontiers.len(), 1);
    assert_eq!(h.budget.used(), used);
    assert_eq!(h.counters.transfers, 0);
}

#[test]
fn mixed_obligations_and_unknown_nodes_never_attempt_promotion() {
    for reason in [
        FrontierReason::Work,
        FrontierReason::Relations(QueryReason::Cancelled),
        FrontierReason::StateJoin(JoinBoundary::IdentityExhausted),
        FrontierReason::StateJoin(JoinBoundary::Shape),
        FrontierReason::UnsupportedOpcode(0xff),
        FrontierReason::ByteRange(crate::analysis::ByteRangeBoundary::UnknownSize),
    ] {
        let mut h = Harness::new("525b00");
        let mut first = h.first();
        transfer::boundary(&mut first, 0, reason);
        let used = h.budget.used();
        assert_eq!(h.retry(first).frontiers.len(), 2);
        assert_eq!(h.budget.used(), used);
        assert_eq!(h.counters.transfers, 1);
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Fresh
        ));
    }
    let mut h = Harness::new("525b00");
    let first = h.first();
    h.table.nodes[0].payload.frames.last_mut().unwrap().code = FrameCode::Unknown;
    let used = h.budget.used();
    assert_eq!(h.retry(first).frontiers.len(), 1);
    assert_eq!(h.budget.used(), used);
    assert_eq!(h.counters.transfers, 1);
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Fresh
    ));
}

#[test]
fn failed_preparation_and_cancellation_keep_original_bound_and_same_spent_ledger() {
    let mut h = Harness::new("525b00");
    let first = h.first();
    let original = h.table.nodes[0].payload.clone();
    let remaining = h.config.max_work - h.budget.used();
    let insufficient = original.work_size() * 8 - 1;
    assert!(h.budget.charge(remaining - insufficient));
    let used = h.budget.used();
    let failed = h.retry(first);
    assert_eq!(
        failed.frontiers.last().unwrap().reason,
        FrontierReason::Work
    );
    assert_eq!(h.budget.used(), used);
    assert_eq!(h.counters.transfers, 1);
    assert!(semantic_eq(&original, &h.table.nodes[0].payload));
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Attempted
    ));
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut h = Harness::new("525b00");
        let first = h.first();
        let original = h.table.nodes[0].payload.clone();
        let used = h.budget.used();
        token.cancel();
        let cancelled = h.retry(first);
        assert_eq!(
            cancelled.frontiers.last().unwrap().reason,
            FrontierReason::Relations(QueryReason::Cancelled)
        );
        assert_eq!(h.counters.transfers, 1);
        assert_eq!(h.budget.used(), used);
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
    });
}

#[test]
fn incoming_growth_and_duplicate_inputs_keep_one_fixed_coarse_mode() {
    let mut h = Harness::new("525b00");
    let mut incoming = h.table.nodes[0].payload.clone();
    let first = h.first();
    let retry = h.retry(first);
    assert!(retry.frontiers.is_empty());
    let layout = match &h.table.nodes[0].runtime_retry {
        RuntimeRetry::Coarse(coarse) => coarse.layout.clone(),
        _ => panic!("must have promoted"),
    };
    let shared = h
        .config
        .domain()
        .unwrap()
        .join(&word(7), &word(9))
        .with_expression(ExprId::fresh().unwrap());
    let frame = incoming.frames.last_mut().unwrap();
    frame.stack = vec![shared.clone(), shared];
    frame.stack_aliases =
        super::super::super::stack_alias::StackAliases::form(&frame.stack, true).unwrap();
    incoming.work = incoming.measure_work();
    for _ in 0..3 {
        let id = h
            .solver()
            .enqueue_summary(0, incoming.clone(), None, None)
            .unwrap();
        assert_eq!(id, 0);
        let node = &h.table.nodes[id];
        assert_eq!(node.payload.storage_observations, Default::default());
        assert!(!node.payload.active().stack_aliases.same_class(0, 1));
        let RuntimeRetry::Coarse(coarse) = &node.runtime_retry else {
            panic!("mode reset on growth")
        };
        assert!(semantic_eq(&layout, &coarse.layout));
        let roundtrip = form(
            &instantiate(&node.payload, h.config.domain().unwrap()).unwrap(),
            &h.entry,
            h.config.domain().unwrap(),
        )
        .unwrap();
        let joined = coarse
            .join(
                &node.payload,
                &roundtrip,
                h.config.domain().unwrap(),
                &mut h.budget,
                false,
            )
            .unwrap();
        assert!(semantic_eq(&node.payload, &joined));
    }
    assert_eq!(h.counters.transfers, 2);
    assert_eq!(h.table.nodes.len(), 1);
    assert_eq!(h.table.nodes[0].updates, 1);
}

#[test]
fn an_already_coarse_node_keeps_its_frontier_without_a_second_promotion() {
    let mut h = Harness::new("525b00");
    let first = h.first();
    assert!(h.retry(first).frontiers.is_empty());
    let canonical = h.table.nodes[0].payload.clone();
    // The plain input root still fits five nodes; its next WriteWord does not.
    h.config.analysis.relations.max_nodes = 5;
    let failed = h.first();
    assert_eq!(failed.retention_failures, [(0, RetentionCause::State)]);
    assert_eq!(h.counters.transfers, 3);
    let used = h.budget.used();
    let unchanged = h.retry(failed);
    assert_eq!(h.counters.transfers, 3);
    assert_eq!(h.budget.used(), used);
    assert_eq!(unchanged.frontiers.len(), 1);
    assert!(semantic_eq(&canonical, &h.table.nodes[0].payload));
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Coarse(_)
    ));
}

#[test]
fn the_last_allowed_transfer_can_publish_a_complete_coarse_execution() {
    let mut h = Harness::new("525b00");
    h.config.analysis.max_transfers = 2;
    let first = h.first();
    let retry = h.retry(first);
    assert_eq!(h.counters.transfers, 2);
    assert!(retry.frontiers.is_empty());
    assert_eq!(retry.successors.len(), 1);
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Coarse(_)
    ));
}

#[test]
fn replacing_a_local_attempt_does_not_clear_earlier_table_evidence() {
    let mut h = Harness::new("525b00");
    let input = instantiate(&h.table.nodes[0].payload, h.config.domain().unwrap()).unwrap();
    h.solver()
        .frontier(input.clone(), FrontierReason::UnsupportedOpcode(0xfe), 99);
    h.solver()
        .publish(
            0,
            OutcomeKind::Failure,
            crate::world::ByteArray::empty(),
            input.store,
        )
        .unwrap();
    let generation = h.table.procedures[0].outputs[&OutcomeKind::Failure].generation;
    let first = h.first();
    let retry = h.retry(first);
    assert!(retry.frontiers.is_empty());
    assert_eq!(h.table.frontiers.len(), 1);
    assert_eq!(h.table.frontiers[0].pc, 99);
    assert_eq!(
        h.table.frontiers[0].reason,
        FrontierReason::UnsupportedOpcode(0xfe)
    );
    assert_eq!(
        h.table.procedures[0].outputs[&OutcomeKind::Failure].generation,
        generation
    );
}

#[test]
fn reification_and_retry_interruptions_neither_refund_work_nor_install_partial_bounds() {
    for finish_preparation in [false, true] {
        let mut h = Harness::new("525b00");
        let first = h.first();
        let original = h.table.nodes[0].payload.clone();
        let progress = first.instruction_progress.clone();
        let alternatives = first.outcomes.len();
        let mut quote = WorkBudget::new(h.config.max_work);
        CoarseRuntime::prepare(&original, h.config.domain().unwrap(), &mut quote).unwrap();
        let allowance = quote.used() - usize::from(!finish_preparation);
        let remaining = h.config.max_work - h.budget.used();
        assert!(h.budget.charge(remaining - allowance));
        let used = h.budget.used();
        let stopped = h.retry(first);
        assert_eq!(
            stopped.frontiers.last().unwrap().reason,
            FrontierReason::Work
        );
        assert_eq!(stopped.instruction_progress, progress);
        assert_eq!(stopped.outcomes.len(), alternatives);
        assert!(h.budget.used() > used && h.budget.exhausted());
        assert_eq!(h.counters.transfers, if finish_preparation { 2 } else { 1 });
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
        let spent = h.budget.used();
        let _ = h.retry(stopped);
        assert_eq!(h.budget.used(), spent);
    }
}

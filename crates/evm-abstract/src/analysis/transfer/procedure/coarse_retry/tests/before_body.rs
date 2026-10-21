use crate::domain::ProductAlgebra;
//! Real summary Input binding failures, before any instruction can execute.

#[cfg(test)]
use crate::analysis::testing::{ByteArray, ByteArraySummary};
use crate::domain::EvmNumericAlgebra as _;

use super::*;
use crate::{
    analysis::{Continuation, JoinBoundary, MachineEdgeKind},
    domain::{
        relational::{QueryReason, RelationLimits},
        symbolic::{ByteStateKind, ExprError, ExprId, StateError},
    },
    world::{RangeError, StoreError},
};
use std::collections::BTreeSet;

fn input_summary(
    domain: crate::domain::AlgebraRef<'_, ProductAlgebra>,
    coordinates: usize,
    values: &[u8],
) -> ByteArraySummary {
    // Every operand is an ordinary exact array. The union retains independent
    // non-dense finite cells; no unsupported value or poisoned root is used.
    values
        .iter()
        .map(|value| {
            ByteArray::exact(&vec![*value; coordinates])
                .summary_projection(domain)
                .unwrap()
        })
        .reduce(|old, incoming| old.join(&incoming, domain, false))
        .unwrap()
}

fn harness(code: &str, cap: usize, coordinates: usize, values: &[u8]) -> Harness {
    let (world, entry) = fixtures::fixture(&[(0x101, code)]);
    let mut config = fixtures::config();
    let mut config_numeric = crate::analysis::testing::NumericLimits::machine();
    config_numeric.relations.max_nodes = cap;
    config_numeric.max_constants = 128;
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut seed =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    seed.active_mut().stack = vec![word(7), word(0)];
    seed.active_mut().key.jump_history = vec![3, 7, 11];
    seed.normalize();
    let mut h = Harness {
        world,
        entry,
        budget: WorkBudget::new(config.max_work),
        config,
        table: Table::default(),
        counters: Counters::default(),
    };
    assert_eq!(h.solver().procedure(seed).unwrap(), 0);
    h.table.nodes[0].payload.frames[0].calldata = input_summary(domain, coordinates, values);
    h.table.nodes[0].payload.work = h.table.nodes[0].payload.measure_work();
    h
}

fn raw_failure(h: &Harness) -> MachineError {
    let error = instantiate(&h.table.nodes[0].payload, crate::analysis::testing::product(h.config_numeric.spec()).borrowed()).unwrap_err();
    let MachineError::Bytes(RangeError::State(StateError::Limit { nodes, depth })) = &error else {
        panic!("must fail at the raw Input binding constructor: {error:?}")
    };
    assert!(*nodes > h.config_numeric.relations.max_nodes && *nodes != usize::MAX);
    assert_eq!(*depth, 2, "a shallow carrier, not a deep write history");
    assert_eq!(h.counters.transfers, 0);
    error
}

#[test]
fn real_input_binding_limits_promote_before_body_at_small_library_and_screen13_caps() {
    let library_cap = RelationLimits::default().max_nodes;
    assert_eq!(library_cap, 1024);
    // Screen13 supplies 16K explicitly; it is not the library default.
    let screen13_cap = 16_384;
    for (cap, coordinates, values) in [
        (64, 8, vec![0, 1, 3]),
        (
            library_cap,
            16,
            (0..=127).filter(|value| *value != 63).collect(),
        ),
        (
            screen13_cap,
            128,
            (0..=127).filter(|value| *value != 63).collect(),
        ),
    ] {
        let mut h = harness("525b00", cap, coordinates, &values);
        let original = h.table.nodes[0].payload.clone();
        let error = raw_failure(&h);
        assert!(retention_refusal(&error));
        let domain = crate::analysis::testing::product(h.config_numeric.spec());
let domain = domain.borrowed();
        // Every individual coordinate fits the same cap. Only their complete
        // Input carrier refuses; the selected policy itself remains unchanged.
        input_summary(domain, 1, &values)
            .instantiate(domain)
            .unwrap();
        let mut quote = WorkBudget::new(h.config.max_work);
        let (_, bound, live) = prepare(&original, domain, &mut quote).unwrap();
        assert!(bound.work_size() < original.work_size());
        let ByteStateKind::Input { bindings, .. } = live.active().calldata.state_root().kind()
        else {
            panic!("coarse calldata must remain an independent Input")
        };
        assert!(bindings.is_empty());
        assert!(live.active().calldata.state_root().nodes() < cap);
        assert_eq!(
            live.active().calldata.len().singleton(),
            Some(U256::from(coordinates))
        );
        assert_eq!(live.active().key, original.active().key);
        assert_eq!(live.active().program, original.active().program);
        assert_eq!(
            live.active().environment_calldata,
            original.active().environment_calldata
        );
        let spent = h.budget.used();
        h.solver().run();
        assert!(h.table.frontiers.is_empty(), "{:?}", h.table.frontiers);
        assert_eq!(h.config_numeric.relations.max_nodes, cap);
        assert_eq!(
            h.counters.transfers, 2,
            "one coarse body and its known successor"
        );
        assert!(h.budget.used() >= spent + original.work_size() * 4 + quote.used());
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Coarse(_)
        ));
        assert_eq!(h.table.nodes.len(), 2);
        assert!(
            h.table.procedures[0]
                .outputs
                .contains_key(&OutcomeKind::Return)
        );
        assert_eq!(h.table.procedures[0].covered_blocks, BTreeSet::from([0, 1]));
        assert_eq!(h.table.nodes[0].payload.active().key, original.active().key);
    }
}

#[test]
fn prebody_promotion_keeps_the_whole_original_gamma_and_captured_context() {
    let mut h = harness("525b00", 256, 32, &[0, 1, 3]);
    let domain = crate::analysis::testing::product(h.config_numeric.spec());
let domain = domain.borrowed();
    let owner = address(0x101);
    let mut store =
        crate::analysis::transfer::procedure::testing::initial(&h.world, &h.entry, domain)
            .unwrap()
            .store;
    store
        .write_checked(owner, &word(0), &domain.join(&word(3), &word(5)), domain)
        .unwrap();
    let parent_saved = store.summary_projection(domain).unwrap();
    store
        .write_checked(owner, &word(0), &domain.join(&word(11), &word(13)), domain)
        .unwrap();
    let child_saved = store.summary_projection(domain).unwrap();
    store
        .write_checked(owner, &word(0), &domain.join(&word(17), &word(19)), domain)
        .unwrap();
    let original = &mut h.table.nodes[0].payload;
    original.frames[0].saved_store = parent_saved;
    let mut child = original.frames[0].clone();
    child.saved_store = child_saved;
    child.environment_calldata = false;
    original.frames.push(child);
    original.continuations.push(Continuation {
        return_block: Some(1),
        output_offset: domain.join(&word(1), &word(3)),
        output_size: domain.join(&word(2), &word(4)),
        creation: None,
    });
    original.store = store.summary_projection(domain).unwrap();
    original.location.frames.push(original.active().key.clone());
    original.work = original.measure_work();
    let original = original.clone();
    let error = raw_failure(&h);
    let execution = h.solver().retry_instantiation(0, &original, error).unwrap();
    assert!(execution.frontiers.is_empty());
    // MSTORE's logical-memory envelope preserves a gas-failure sibling in
    // addition to the continuing child. Require both precise kinds, rather
    // than discarding the exceptional completion while selecting the witness.
    let [first, second] = execution.successors.as_slice() else {
        panic!(
            "expected one child continuation and one failure, got {} successors",
            execution.successors.len()
        );
    };
    let (continued, failed) = match (first.kind, second.kind) {
        (MachineEdgeKind::Intraprocedural(_), MachineEdgeKind::Failure) => (first, second),
        (MachineEdgeKind::Failure, MachineEdgeKind::Intraprocedural(_)) => (second, first),
        kinds => panic!("unexpected successor kinds: {kinds:?}"),
    };
    assert!(execution.outcomes.is_empty());
    assert_eq!(execution.completed_calls.len(), 1);
    let completed = &execution.completed_calls[0];
    assert_eq!(completed.kind, OutcomeKind::Failure);
    assert_eq!(completed.output_kind, OutcomeKind::Failure);
    assert_eq!(completed.data, ByteArray::empty());
    assert_eq!(completed.output_data, ByteArray::empty());
    assert_eq!(completed.payload.active().key.basic_block_index, 0);
    assert!(completed.payload.active().stack.is_empty());
    assert_eq!(
        completed.payload.active().memory.len().singleton(),
        Some(U256::ZERO)
    );
    let mut rollback = completed.payload.store.clone();
    rollback.restore(completed.payload.active().saved_store.clone());
    assert_eq!(completed.output_store, rollback);
    assert_eq!(failed.payload.store, rollback);
    assert_eq!(failed.payload.call_stack.depth(), 1);
    assert_eq!(failed.payload.active().returndata, ByteArray::empty());
    assert_eq!(
        failed.payload.active().stack.last().unwrap().singleton(),
        Some(U256::ZERO)
    );
    assert_eq!(
        failed.payload.active().saved_store,
        completed.payload.call_stack.root().state.saved_store
    );
    let live = &continued.payload;
    assert_eq!(live.call_stack.depth(), 2);
    let bound = &h.table.nodes[0].payload;
    assert_eq!(bound.frames.len(), original.frames.len());
    assert_eq!(bound.continuations.len(), 1);
    assert_eq!(bound.continuations[0].return_block, Some(1));
    assert_eq!(bound.continuations[0].creation, None);
    for (before, after) in original.frames.iter().zip(&bound.frames) {
        assert_eq!(before.key, after.key);
        assert_eq!(before.program, after.program);
        assert_eq!(before.code, after.code);
        assert_eq!(before.environment_calldata, after.environment_calldata);
        assert_eq!(
            before.calldata.len().interval().unsigned_bounds(),
            after.calldata.len().interval().unsigned_bounds()
        );
    }
    let values = [
        live.call_stack
            .iter()
            .next()
            .unwrap()
            .saved_store
            .state()
            .read_checked(owner, &word(0), domain)
            .unwrap(),
        live.active()
            .saved_store
            .state()
            .read_checked(owner, &word(0), domain)
            .unwrap(),
        live.store.read_checked(owner, &word(0), domain).unwrap(),
    ];
    for parent in [3u64, 5] {
        for child in [11u64, 13] {
            for current in [17u64, 19] {
                for offset in [1u64, 3] {
                    for size in [2u64, 4] {
                        let mut witness = snapshots::Witness::default();
                        for (value, expected) in values.iter().zip([parent, child, current]) {
                            let expected = U256::from(expected);
                            assert!(value.contains(expected));
                            witness.assign(value.expression().unwrap(), expected);
                        }
                        assert!(
                            bound.continuations[0]
                                .output_offset
                                .contains(U256::from(offset))
                        );
                        assert!(
                            bound.continuations[0]
                                .output_size
                                .contains(U256::from(size))
                        );
                    }
                }
            }
        }
    }
    assert_eq!(h.counters.transfers, 1);
}

#[test]
fn before_body_failed_preparation_or_transfer_keeps_first_frontier_and_metadata() {
    for (code, cap) in [
        ("525b00".to_owned(), 1),
        (
            format!(
                "52{}5b00",
                (1..=64u16)
                    .map(|offset| format!("600761{:04x}52", offset * 32))
                    .collect::<String>()
            ),
            64,
        ),
    ] {
        // The one-node case refuses coarse rehydration itself. The other
        // prepares successfully but fails later in the replacement body.
        let mut h = harness(&code, 64, 8, &[0, 1, 3]);
        h.config_numeric.relations.max_nodes = cap;
        let original = h.table.nodes[0].payload.clone();
        let domain = crate::analysis::testing::product(h.config_numeric.spec());
let domain = domain.borrowed();
        let error = instantiate(&original, domain).unwrap_err();
        let reason = error.clone().frontier_reason();
        let spent = h.budget.used();
        assert!(
            h.solver()
                .retry_instantiation(0, &original, error.clone())
                .is_none()
        );
        assert!(h.budget.used() > spent);
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
        let expected = if cap == 1 {
            // Rehydration of the replacement itself is refused, leaving both
            // the original Input obligation and the rejected retry obligation.
            vec![
                reason,
                FrontierReason::Relations(QueryReason::ExpressionLimit),
            ]
        } else {
            vec![reason]
        };
        assert_eq!(
            h.table
                .frontiers
                .iter()
                .map(|frontier| frontier.reason.clone())
                .collect::<Vec<_>>(),
            expected
        );
        for frontier in &h.table.frontiers {
            assert_eq!(frontier.pc, 0);
            assert_eq!(frontier.target.as_ref(), Some(&original.location));
            assert!(frontier.payload.is_none());
        }
        assert!(h.table.procedures[0].outputs.is_empty());
        assert!(h.table.precision_losses.is_empty());
        assert_eq!(h.counters.transfers, usize::from(cap != 1));
        let transfers = h.counters.transfers;
        let mut fresh_ids = original.clone();
        fresh_ids.frames[0].stack[0] = word(9).with_expression(ExprId::fresh().unwrap());
        fresh_ids.work = fresh_ids.measure_work();
        assert_eq!(
            h.solver()
                .enqueue_summary(0, fresh_ids, None, None)
                .unwrap(),
            0
        );
        let incoming = h.table.nodes[0].payload.clone();
        assert!(
            h.solver()
                .retry_instantiation(0, &incoming, error)
                .is_none()
        );
        assert_eq!(h.counters.transfers, transfers);
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
    }
}

#[test]
fn raw_prebody_classifier_excludes_nonretention_errors_before_flattening() {
    let mut errors = vec![
        MachineError::Work,
        MachineError::Shape,
        MachineError::IdentityExhausted,
        MachineError::States,
        MachineError::Alternatives { limit: 1 },
        MachineError::Relations(QueryReason::ExpressionLimit),
        MachineError::Relations(QueryReason::Cancelled),
        MachineError::Expression(ExprError::Cancelled),
        MachineError::Expression(ExprError::Unsupported(0xff)),
        MachineError::Expression(ExprError::Arity {
            opcode: 1,
            expected: 2,
            actual: 1,
        }),
        MachineError::Expression(ExprError::ExportBytes { bytes: 2, limit: 1 }),
        MachineError::Bytes(RangeError::UnknownSize),
        MachineError::Bytes(RangeError::State(StateError::Cancelled)),
        MachineError::Bytes(RangeError::State(StateError::IdentityExhausted)),
        MachineError::Store(StoreError::State(StateError::IdentityExhausted)),
    ];
    for (nodes, depth) in [(usize::MAX, usize::MAX), (usize::MAX, 1), (1, usize::MAX)] {
        errors.extend([
            MachineError::Expression(ExprError::Limit { nodes, depth }),
            MachineError::Bytes(RangeError::State(StateError::Limit { nodes, depth })),
            MachineError::Store(StoreError::State(StateError::Limit { nodes, depth })),
            MachineError::Store(StoreError::Bytes(RangeError::Expression(
                ExprError::Limit { nodes, depth },
            ))),
        ]);
    }
    for error in errors {
        assert!(!retention_refusal(&error), "{error:?}");
        let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
        let original = h.table.nodes[0].payload.clone();
        assert!(
            h.solver()
                .retry_instantiation(0, &original, error)
                .is_none()
        );
        assert_eq!(h.counters.transfers, 0);
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Fresh
        ));
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(h.table.precision_losses.is_empty());
    }
    assert!(!interruption(&FrontierReason::StateJoin(
        JoinBoundary::Shape
    )));
}

#[test]
fn unknown_native_init_and_missing_captured_program_never_enter_prebody_retry() {
    for code in [
        FrameCode::Unknown,
        FrameCode::InitCode,
        FrameCode::Precompile(address(1)),
        FrameCode::Empty,
        FrameCode::InvalidDelegation,
        FrameCode::Runtime,
    ] {
        let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
        let error = raw_failure(&h);
        h.table.nodes[0].payload.frames[0].code = code;
        if code == FrameCode::Runtime {
            h.table.nodes[0].payload.frames[0].program = None;
        }
        let original = h.table.nodes[0].payload.clone();
        assert!(
            h.solver()
                .retry_instantiation(0, &original, error)
                .is_none()
        );
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Fresh
        ));
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert_eq!(h.counters.transfers, 0);
    }
    let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
    let error = raw_failure(&h);
    let mut parent = h.table.nodes[0].payload.frames[0].clone();
    parent.code = FrameCode::Unknown;
    h.table.nodes[0].payload.frames.insert(0, parent);
    let original = h.table.nodes[0].payload.clone();
    assert!(
        h.solver()
            .retry_instantiation(0, &original, error)
            .is_none()
    );
    assert!(matches!(
        &h.table.nodes[0].runtime_retry,
        RuntimeRetry::Fresh
    ));
}

#[test]
fn prebody_work_cancel_and_transfer_refusals_keep_original_evidence() {
    for finish_preparation in [false, true] {
        let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
        let error = raw_failure(&h);
        let original = h.table.nodes[0].payload.clone();
        let allowance = h.preparation_work() - usize::from(!finish_preparation);
        let remaining = h.config.max_work - h.budget.used();
        assert!(h.budget.charge(remaining - allowance));
        let spent = h.budget.used();
        assert!(
            h.solver()
                .retry_instantiation(0, &original, error)
                .is_none()
        );
        assert!(h.budget.used() >= spent && h.budget.exhausted());
        assert_eq!(h.counters.transfers, usize::from(finish_preparation));
        assert_eq!(h.table.frontiers.len(), 2);
        assert_eq!(
            h.table.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        );
        assert_eq!(h.table.frontiers[1].reason, FrontierReason::Work);
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
        assert!(h.table.procedures[0].outputs.is_empty());
        assert!(h.table.precision_losses.is_empty());
    }
    let token = analysis_control::Cancellation::new();
    analysis_control::with_cancellation(&token, || {
        let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
        let error = raw_failure(&h);
        let original = h.table.nodes[0].payload.clone();
        let spent = h.budget.used();
        token.cancel();
        assert!(
            h.solver()
                .retry_instantiation(0, &original, error)
                .is_none()
        );
        assert_eq!(h.budget.used(), spent);
        assert_eq!(h.counters.transfers, 0);
        assert_eq!(h.table.frontiers.len(), 2);
        assert_eq!(
            h.table.frontiers[0].reason,
            FrontierReason::Relations(QueryReason::ExpressionLimit)
        );
        assert_eq!(
            h.table.frontiers[1].reason,
            FrontierReason::Relations(QueryReason::Cancelled)
        );
        assert!(semantic_eq(&original, &h.table.nodes[0].payload));
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Attempted
        ));
    });
    let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
    let error = raw_failure(&h);
    let original = h.table.nodes[0].payload.clone();
    h.counters.transfers = h.config.analysis.max_transfers;
    assert!(
        h.solver()
            .retry_instantiation(0, &original, error)
            .is_none()
    );
    assert_eq!(h.table.frontiers.len(), 2);
    assert_eq!(
        h.table.frontiers[1].reason,
        FrontierReason::Budget(Limit::Transfers)
    );
    assert!(semantic_eq(&original, &h.table.nodes[0].payload));
}

#[test]
fn prebody_coarse_mode_and_frozen_layout_survive_fresh_incoming_ids() {
    let mut h = harness("525b00", 64, 8, &[0, 1, 3]);
    let error = raw_failure(&h);
    let original = h.table.nodes[0].payload.clone();
    let execution = h
        .solver()
        .retry_instantiation(0, &original, error.clone())
        .unwrap();
    assert_eq!(execution.successors.len(), 1);
    let layout = match &h.table.nodes[0].runtime_retry {
        RuntimeRetry::Coarse(coarse) => coarse.layout().clone(),
        _ => panic!("must have promoted before the body"),
    };
    let domain = crate::analysis::testing::product(h.config_numeric.spec());
let domain = domain.borrowed();
    let mut grown = None;
    for _ in 0..3 {
        let mut incoming = original.clone();
        incoming.frames[0].stack[0] = word(9).with_expression(ExprId::fresh().unwrap());
        incoming.work = incoming.measure_work();
        assert_eq!(
            h.solver().enqueue_summary(0, incoming, None, None).unwrap(),
            0
        );
        let node = &h.table.nodes[0];
        let RuntimeRetry::Coarse(coarse) = &node.runtime_retry else {
            panic!("incoming identities renewed Fresh")
        };
        assert!(semantic_eq(&layout, coarse.layout()));
        let live = instantiate(&node.payload, domain).unwrap();
        let ByteStateKind::Input { bindings, .. } = live.active().calldata.state_root().kind()
        else {
            panic!("coarse join must not restore content bindings")
        };
        assert!(bindings.is_empty());
        assert!(live.active().stack[0].contains(U256::from(7)));
        assert!(live.active().stack[0].contains(U256::from(9)));
        assert_eq!(
            live.active().stack[0].interval().unsigned_bounds(),
            (U256::from(7), U256::from(9))
        );
        assert_eq!(live.active().stack[1].singleton(), Some(U256::ZERO));
        if let Some(previous) = &grown {
            assert!(semantic_eq(previous, &node.payload));
        } else {
            grown = Some(node.payload.clone());
        }
        let bound = node.payload.clone();
        assert!(
            h.solver()
                .retry_instantiation(0, &bound, error.clone())
                .is_none()
        );
        assert_eq!(h.counters.transfers, 1);
        assert!(matches!(
            &h.table.nodes[0].runtime_retry,
            RuntimeRetry::Coarse(_)
        ));
    }
    assert_eq!(h.table.nodes.len(), 1);
}

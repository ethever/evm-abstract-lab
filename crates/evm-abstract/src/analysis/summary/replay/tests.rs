use super::Replay;
use crate::{
    analysis::{
        self, Continuation, FrontierReason, InstructionProgress, MachineError, OutcomeKind,
        RootFrame, Status,
        machine::fixtures,
        summary::{Certificate, Node, SummaryOutput, Terminal},
        transfer::CompletedCall,
    },
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, QueryReason, RelationState},
        symbolic::ExprId,
    },
    resource::WorkBudget,
    world::ByteArray,
};
use revm_bytecode::opcode;
use std::{collections::BTreeSet, sync::Arc};

fn certificate(domain: Domain) -> (Certificate, crate::analysis::MachinePayload, ByteArray) {
    let mut entry = fixtures::payload(domain);
    entry.active_mut().calldata = ByteArray::unknown();
    entry.active_mut().calldata.bind_input(41).unwrap();
    entry.active_mut().stack = vec![AbstractValue::top().with_expression(ExprId::fresh().unwrap())];
    entry.normalize();
    let mut exit = entry.clone();
    let guard = ExprId::fresh().unwrap();
    let data = ByteArray::unknown()
        .join_guarded(
            &ByteArray::unknown(),
            domain,
            guard.clone(),
            domain.spec().relations().expressions(),
        )
        .unwrap();
    let key = AbstractValue::top().with_expression(ExprId::fresh().unwrap());
    exit.active_mut()
        .memory
        .copy_from(
            &fixtures::word(0),
            &data,
            &fixtures::word(0),
            data.len(),
            4096,
            domain,
        )
        .unwrap();
    let historical = exit
        .active()
        .memory
        .read_word_checked(&fixtures::word(0), domain)
        .unwrap();
    exit.store
        .write_checked(fixtures::address(0x101), &key, &historical, domain)
        .unwrap();
    exit.active_mut().saved_store = exit.store.snapshot();
    exit.active_mut().stack = vec![historical.clone()];
    exit.active_mut().returndata = data.clone();
    exit.active_mut().call_value = key.clone();
    assert!(
        exit.relations
            .assume(&guard, true, &domain.spec().relations())
            != crate::domain::relational::CheckResult::ProvenImpossible
    );
    let nested = RootFrame {
        state: exit.active().clone(),
    }
    .into_child(Continuation {
        return_block: Some(1),
        output_offset: key.clone(),
        output_size: data.len().clone(),
        creation: None,
    });
    exit.call_stack.push_child(nested);
    exit.normalize();
    let mut output_store = exit.store.clone();
    output_store
        .write_checked(fixtures::address(0x101), &key, &fixtures::word(9), domain)
        .unwrap();
    let completed = CompletedCall {
        payload: exit.clone(),
        kind: OutcomeKind::Return,
        data: data.clone(),
        output_store: output_store.clone(),
        output_kind: OutcomeKind::Return,
        output_data: data.clone(),
    };
    let node = Node {
        entry: entry.clone(),
        resume_status: Some(true),
        partition: Some(true),
        exit: exit.clone(),
        exit_stack: vec![historical],
        executed_pcs: vec![0],
        instruction_progress: vec![InstructionProgress::Completed],
        instruction_values: Arc::from([]),
        diagnostics: Vec::new(),
        completed_calls: vec![completed],
    };
    let terminal = Terminal {
        from: 0,
        output: SummaryOutput {
            kind: OutcomeKind::Return,
            data: data.clone(),
            store: output_store,
        },
        payload: exit,
        raw_kind: OutcomeKind::Return,
        raw_data: data.clone(),
    };
    let mut caller = fixtures::payload(domain);
    // This old result deliberately shares the original certificate's local
    // source. A suspended caller must not make it a bound input of the replay.
    caller.active_mut().returndata = data.clone();
    caller
        .call_stack
        .push_child(entry.call_stack.root().clone().into_child(Continuation {
            return_block: Some(1),
            output_offset: fixtures::word(0),
            output_size: fixtures::word(32),
            creation: None,
        }));
    caller.normalize();
    (
        Certificate {
            record: 0,
            nodes: vec![node],
            edges: Vec::new(),
            terminals: vec![terminal],
        },
        caller,
        data,
    )
}

#[test]
fn replay_renames_all_hidden_sources_roots_history_and_outputs_with_one_map() {
    let domain = fixtures::config().domain().unwrap();
    let (certificate, caller, original_data) = certificate(domain);
    let caller_before = caller.clone();
    let original_root = certificate.nodes[0]
        .exit
        .active()
        .memory
        .state_root()
        .clone();
    let original_output = certificate.terminals[0].output.store.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let replay = Replay::new(
        &certificate,
        &caller,
        &mut budget,
        domain.spec().relations().expressions(),
    )
    .unwrap();
    let map = replay.renaming.clone();
    let input_tokens = certificate.nodes[0].entry.fresh_leaves();
    assert!(map.keys().all(|token| !input_tokens.contains(token)));
    let old_tokens = original_data.fresh_leaves();
    assert!(!old_tokens.is_empty());
    assert!(old_tokens.iter().all(|token| map.contains_key(token)));
    let expected: BTreeSet<_> = old_tokens.iter().map(|token| map[token].clone()).collect();
    let prepared = replay
        .prepare(
            &mut budget,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
        .unwrap();
    let node = &prepared.nodes[0];
    assert_eq!(node.resume_status, Some(true));
    assert_eq!(
        node.entry.active().calldata.state_root(),
        certificate.nodes[0].entry.active().calldata.state_root()
    );
    assert_eq!(node.entry.call_stack.root().state.returndata, original_data);
    for data in [
        &node.exit.active().returndata,
        &node.completed_calls[0].data,
        &node.completed_calls[0].output_data,
        &prepared.terminals[0].raw_data,
        &prepared.terminals[0].output.data,
    ] {
        assert_eq!(data.fresh_leaves(), expected);
    }
    for payload in [
        &node.exit,
        &node.completed_calls[0].payload,
        &prepared.terminals[0].payload,
    ] {
        let frame = payload.active();
        assert_eq!(frame.stack[0].expression(), node.exit_stack[0].expression());
        let continuation = &payload.call_stack.active_child().unwrap().continuation;
        assert_eq!(
            continuation.output_offset.expression(),
            frame.call_value.expression()
        );
        assert_eq!(
            continuation.output_size.expression(),
            frame.returndata.len().expression()
        );
        let renamed_saved = frame.saved_store.state().fresh_leaves();
        assert!(expected.is_subset(&renamed_saved));
        let roots = frame.memory.fresh_leaves();
        assert!(expected.is_subset(&roots));
        assert!(old_tokens.is_disjoint(&roots));
    }
    assert!(old_tokens.is_disjoint(&node.completed_calls[0].output_store.fresh_leaves()));
    assert!(old_tokens.is_disjoint(&prepared.terminals[0].output.store.fresh_leaves()));
    assert_eq!(caller, caller_before);
    assert_eq!(
        certificate.nodes[0].exit.active().memory.state_root(),
        &original_root
    );
    assert_eq!(certificate.terminals[0].output.store, original_output);

    let replay = Replay::new(
        &certificate,
        &caller,
        &mut budget,
        domain.spec().relations().expressions(),
    )
    .unwrap();
    let second_map = replay.renaming.clone();
    let second = replay
        .prepare(
            &mut budget,
            crate::domain::ValueAlgebra::new(domain).borrowed(),
        )
        .unwrap();
    assert!(
        map.values()
            .all(|token| !second_map.values().any(|other| token == other))
    );
    assert!(
        prepared.terminals[0]
            .raw_data
            .fresh_leaves()
            .is_disjoint(&second.terminals[0].raw_data.fresh_leaves())
    );
    assert_eq!(
        prepared.nodes[0].entry.active().calldata.state_root(),
        second.nodes[0].entry.active().calldata.state_root()
    );
    assert_eq!(
        prepared.nodes[0].exit.active().returndata.fresh_leaves(),
        expected
    );
}

#[test]
fn replay_failure_and_cancellation_do_not_change_input_or_certificate_snapshots() {
    let domain = fixtures::config().domain().unwrap();
    let (certificate, caller, data) = certificate(domain);
    let before = caller.clone();
    let original = certificate.nodes[0].exit.clone();
    let mut budget = WorkBudget::new(100_000_000);
    let limits = crate::domain::symbolic::ExprLimits {
        max_nodes: 1,
        max_depth: 1,
    };
    let replay = Replay::new(&certificate, &caller, &mut budget, limits).unwrap();
    assert!(
        replay
            .prepare(
                &mut budget,
                crate::domain::ValueAlgebra::new(domain).borrowed()
            )
            .is_err()
    );
    assert_eq!(caller, before);
    assert_eq!(certificate.nodes[0].exit, original);
    assert_eq!(certificate.terminals[0].raw_data, data);
    let cancellation = analysis_control::Cancellation::new();
    cancellation.cancel();
    analysis_control::with_cancellation(&cancellation, || {
        let mut budget = WorkBudget::new(100_000_000);
        let error = Replay::new(
            &certificate,
            &caller,
            &mut budget,
            domain.spec().relations().expressions(),
        )
        .err()
        .unwrap();
        assert_eq!(error, MachineError::Relations(QueryReason::Cancelled));
    });
    assert_eq!(caller, before);
    assert_eq!(certificate.nodes[0].exit, original);
}

fn call(output: u8) -> String {
    format!("602060{output:02x}5f5f6102006207a120fa")
}

#[test]
fn cached_calls_keep_independent_fresh_results_and_summary_switch_agrees() {
    let caller = format!("{}50{}5060405ff3", call(0), call(32));
    let (world, entry) = fixtures::fixture(&[(0x101, &caller), (0x200, "5a5f5260205ff3")]);
    let mut enabled = fixtures::config();
    enabled.use_summaries = true;
    let cached = analysis::analyze_world(world.clone(), entry.clone(), enabled.clone()).unwrap();
    enabled.use_summaries = false;
    let direct = analysis::analyze_world(world, entry, enabled).unwrap();
    for result in [&cached, &direct] {
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        crate::ssa::build_world(result)
            .unwrap()
            .verify(result)
            .unwrap();
        let returned = result
            .outcomes()
            .iter()
            .find(|outcome| {
                outcome.kind == OutcomeKind::Return
                    && outcome.data.fresh_leaves().len() >= 2
                    && [0, 32].into_iter().all(|offset| {
                        outcome
                            .data
                            .read_word(&fixtures::word(offset), Domain::default())
                            .contains(alloy_primitives::U256::from(1))
                    })
            })
            .unwrap();
        assert_eq!(
            returned.data.len().singleton(),
            Some(alloy_primitives::U256::from(64))
        );
        let first = returned
            .data
            .read_word_checked(&fixtures::word(0), Domain::default())
            .unwrap();
        let second = returned
            .data
            .read_word_checked(&fixtures::word(32), Domain::default())
            .unwrap();
        assert_ne!(first.expression(), second.expression());
        // Independent leaves may have equal concrete valuations. This test
        // checks identity, not an unsupported inequality assumption.
        assert!(returned.data.fresh_leaves().len() >= 2);
        let policy = fixtures::config().analysis.relations;
        let mut models = Vec::new();
        for outcome in result
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
        {
            let words = [0, 32].map(|offset| {
                outcome
                    .data
                    .read_word_checked(&fixtures::word(offset), Domain::default())
                    .unwrap()
            });
            let cases = [(1u64, 2u64), (1, 1)].map(|(left, right)| {
                let equal = |value: &AbstractValue, constant: u64| {
                    ExprId::operation(
                        opcode::EQ,
                        &[
                            value.symbolic_expression().unwrap(),
                            ExprId::constant(alloy_primitives::U256::from(constant)),
                        ],
                        policy.expressions(),
                    )
                    .unwrap()
                };
                let pair = ExprId::operation(
                    opcode::AND,
                    &[equal(&words[0], left), equal(&words[1], right)],
                    policy.expressions(),
                )
                .unwrap();
                RelationState::default().can_equal(&pair, alloy_primitives::U256::from(1), &policy)
            });
            models.push((outcome.state, cases));
        }
        // A successful two-call path permits both independent and coincident
        // valuations. Other return artifacts can describe a rejected call.
        assert!(
            models.iter().any(
                |(_, cases)| cases == &[CheckResult::MayBeFeasible, CheckResult::MayBeFeasible]
            ),
            "actual output witnesses (hits={}): {models:?}",
            result.summary_stats().hits
        );
    }
    assert!(
        cached.summary_stats().hits > 0,
        "{:?}",
        cached.summary_stats()
    );
    assert_eq!(direct.summary_stats().hits, 0);
    assert_eq!(cached.outcomes().len(), direct.outcomes().len());
    for (cached, direct) in cached.outcomes().iter().zip(direct.outcomes()) {
        assert_eq!(cached.kind, direct.kind);
        assert_eq!(cached.data.len().numeric(), direct.data.len().numeric());
        for offset in [0, 32] {
            assert_eq!(
                cached
                    .data
                    .read_word(&fixtures::word(offset), Domain::default())
                    .numeric(),
                direct
                    .data
                    .read_word(&fixtures::word(offset), Domain::default())
                    .numeric()
            );
        }
    }
}

#[test]
fn budget_interrupted_import_is_atomic_and_keeps_all_input_paths_recoverable() {
    let caller = format!("{}50{}5060205ff3", call(0), call(0));
    let (world, entry) =
        fixtures::fixture(&[(0x101, &caller), (0x200, "6003565b60075f5260205ff3")]);
    let mut config = fixtures::config();
    // Bisect detailed import admission without optional preparation fees.
    config.root_terminal_coverage = false;
    config.use_summaries = true;
    let complete = analysis::analyze_world(world.clone(), entry.clone(), config.clone()).unwrap();
    assert_eq!(
        complete.status(),
        Status::Converged,
        "{:?}",
        complete.frontiers()
    );
    assert!(complete.summary_stats().hits > 0);
    let (mut low, mut high) = (1, complete.work());
    while low < high {
        let midpoint = low + (high - low) / 2;
        config.max_work = midpoint;
        let result = analysis::analyze_world(world.clone(), entry.clone(), config.clone()).unwrap();
        if result.summary_stats().imported_states > 0 {
            high = midpoint;
        } else {
            low = midpoint + 1;
        }
    }
    config.max_work = low - 1;
    let interrupted = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(interrupted.status(), Status::Incomplete);
    assert!(interrupted.summary_stats().hits > 0);
    assert_eq!(interrupted.summary_stats().imported_states, 0);
    assert!(
        interrupted
            .summaries()
            .iter()
            .all(|record| record.reused_at.is_empty())
    );
    let frontier = interrupted
        .frontiers()
        .iter()
        .find(|frontier| {
            frontier.reason == FrontierReason::SummaryWork && frontier.payload.is_some()
        })
        .expect("pending original input for atomic failed import");
    let root = &interrupted.states()[frontier.from.unwrap()];
    assert_eq!(frontier.payload.as_deref(), Some(&root.entry));
    // An entry may have a receipt from an earlier complete ordinary execution.
    // Atomic failure preserves that receipt rather than clearing valid history.
    if root.exit.is_some() {
        assert!(root.execution_evidence.is_some());
        assert!(!root.executed_pcs.is_empty());
    }
    assert!(crate::ssa::build_world(&interrupted).is_err());
    let partial = crate::ssa::build_partial_world(&interrupted).unwrap();
    partial.verify(&interrupted).unwrap();
    assert_eq!(partial.status(), Status::Incomplete);
}

#[test]
fn rejected_world_fingerprint_runs_without_a_summary_or_a_guessed_key() {
    use crate::domain::symbolic::{ByteStateId, ByteView, ExprLimits, GraphLimits};
    let caller = format!("{}50{}5000", call(0), call(0));
    let (_, entry) = fixtures::fixture(&[(0x101, &caller), (0x200, "00")]);
    let mut world = crate::world::World::new(crate::Fork::Osaka, "uncacheable symbolic world");
    let size = GraphLimits::default().max_exact_bytes + 1;
    let input = ByteView::new(
        ByteStateId::exact(Arc::from(vec![0u8; size])),
        ExprId::constant(alloy_primitives::U256::ZERO),
        ExprId::constant(alloy_primitives::U256::from(size)),
    );
    let hash = ExprId::keccak(input, ExprLimits::default()).unwrap();
    let mut account = crate::world::Account::from_hex(&caller, world.fork()).unwrap();
    account.balance = fixtures::word(1_000_000);
    account.storage.insert(
        alloy_primitives::U256::ZERO,
        AbstractValue::top().with_expression(hash),
    );
    world.insert(fixtures::address(0x101), account).unwrap();
    let mut callee = crate::world::Account::from_hex("00", world.fork()).unwrap();
    callee.balance = fixtures::word(1_000_000);
    world.insert(fixtures::address(0x200), callee).unwrap();
    assert!(matches!(
        world.fingerprint(),
        Err(crate::world::FingerprintError::Encoding(_))
    ));
    let mut config = fixtures::config();
    config.max_work = 1_000_000_000;
    config.use_summaries = true;
    let result = analysis::analyze_world(world, entry, config).unwrap();
    assert_eq!(result.status(), Status::Converged);
    assert_eq!(result.summary_stats().hits, 0);
    assert!(result.summaries().is_empty());
    crate::ssa::build_world(&result)
        .unwrap()
        .verify(&result)
        .unwrap();
}

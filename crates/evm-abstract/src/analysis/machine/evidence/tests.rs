use super::{InstructionProgress, InstructionValue};
use crate::domain::ValueAlgebra;
use crate::{
    Address, Fork, U256,
    analysis::{
        self, ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind, Status, WorldAnalysis,
        engine::policy::RecentDistinctJumps,
        transfer::{self, StorageReadPolicy, WorkBudget},
    },
    domain::{AbstractValue, provenance::Origin},
    world::{Account, ByteArray, Entry, EvmEnvironment, World},
};

fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

fn config() -> ExecutionConfig {
    let mut config = ExecutionConfig::default();
    config.analysis.context_depth = 0;
    config.analysis.relations.enabled = false;
    config.use_summaries = false;
    config
}

fn fixture(accounts: &[(u64, &str)]) -> (World, Entry) {
    let mut world = World::new(Fork::Osaka, "instruction value receipt test");
    for (id, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = AbstractValue::constant(U256::from(1_000_000));
        account
            .storage
            .insert(U256::ZERO, AbstractValue::constant(U256::from(7)));
        world.insert(address(*id), account).unwrap();
    }
    let entry = Entry {
        address: address(0x101),
        environment: EvmEnvironment {
            to: address(0x101).into(),
            caller: address(0x900).into(),
            value: AbstractValue::constant(U256::from(37)),
            calldata: ByteArray::empty(),
            ..EvmEnvironment::default()
        },
    };
    (world, entry)
}

fn run(code: &str, config: ExecutionConfig) -> WorldAnalysis {
    let (world, entry) = fixture(&[(0x101, code)]);
    analysis::analyze_world(world, entry, config).unwrap()
}

fn pending_mload() -> WorldAnalysis {
    const CODE: &str = "602a60405100";
    let mut config = config();
    // Bisect the detailed MLOAD receipt without optional preparation fees.
    config.root_terminal_coverage = false;
    config.max_memory_bytes = 32;
    let complete = run(CODE, config.clone());
    assert_eq!(complete.status(), Status::Converged);
    assert_eq!(complete.states()[0].executed_pcs, [0, 2, 4, 5]);
    // Locate the first budget that visits MLOAD in this acyclic prefix. Its
    // visit consumes the offset, then the positive read-scan fee is refused.
    // The upper bound is measured under the unchanged successful policy.
    let (mut minimum, mut maximum) = (1, complete.work());
    while minimum < maximum {
        let midpoint = minimum + (maximum - minimum) / 2;
        config.max_work = midpoint;
        let candidate = run(CODE, config.clone());
        if candidate
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&4))
        {
            maximum = midpoint;
        } else {
            minimum = midpoint + 1;
        }
    }
    config.max_work = minimum;
    run(CODE, config)
}

#[test]
fn popped_intermediate_definitions_retain_numeric_constraints_and_sources() {
    let analysis = run("600260030150345000", config());
    assert_eq!(analysis.status(), Status::Converged);
    let state = &analysis.states()[0];
    assert!(state.exit_stack.is_empty());
    let evidence = state.execution_evidence().unwrap();
    let values = evidence.instruction_values().unwrap();
    assert_eq!(
        values
            .iter()
            .map(|value| (value.instruction_index(), value.numeric().singleton()))
            .collect::<Vec<_>>(),
        vec![
            (0, Some(U256::from(2))),
            (1, Some(U256::from(3))),
            (2, Some(U256::from(5))),
            (4, Some(U256::from(37))),
        ]
    );
    assert!(
        values[2]
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::Arithmetic)
    );
    assert!(
        values[3]
            .provenance()
            .origins()
            .sources()
            .unwrap()
            .contains(&Origin::CallValue)
    );
    for value in values {
        assert_eq!(
            evidence.instructions()[value.instruction_index()],
            InstructionProgress::Completed
        );
    }
}

#[test]
fn stack_aliases_do_not_allocate_definition_observations() {
    let analysis = run("60028090505000", config());
    let evidence = analysis.states()[0].execution_evidence().unwrap();
    let values = evidence.instruction_values().unwrap();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].instruction_index(), 0);
    assert_eq!(values[0].numeric().singleton(), Some(U256::from(2)));
}

#[test]
fn pending_memory_and_faulted_instructions_have_no_normal_result() {
    let pending = pending_mload();
    assert_eq!(pending.status(), Status::Incomplete);
    assert_eq!(pending.states()[0].executed_pcs, [0, 2, 4]);
    assert_eq!(pending.states()[0].exit_stack.len(), 1);
    assert_eq!(
        pending.states()[0].exit_stack[0].singleton(),
        Some(U256::from(42))
    );
    let [frontier] = pending.frontiers() else {
        panic!("expected exactly the refused MLOAD read scan")
    };
    assert_eq!(frontier.reason, FrontierReason::Work);
    assert_eq!((frontier.from, frontier.pc), (Some(0), Some(4)));
    assert_eq!(pending.outcomes().len(), 1);
    assert!(
        pending
            .outcomes()
            .iter()
            .all(|o| o.kind == OutcomeKind::Failure)
    );
    for (analysis, expected_phase, expected_indices) in [
        (pending, InstructionProgress::OperandsConsumed, vec![0, 1]),
        (
            run("5f01600100", config()),
            InstructionProgress::Faulted,
            vec![0],
        ),
        (run("fe", config()), InstructionProgress::Faulted, vec![]),
    ] {
        let evidence = analysis.states()[0].execution_evidence().unwrap();
        assert_eq!(evidence.instructions().last(), Some(&expected_phase));
        assert_eq!(
            evidence
                .instruction_values()
                .unwrap()
                .iter()
                .map(InstructionValue::instruction_index)
                .collect::<Vec<_>>(),
            expected_indices
        );
    }
}

#[test]
fn changed_joined_entry_hides_the_old_receipt_values() {
    let mut config = config();
    config.analysis.max_transfers = 2;
    let analysis = run("5f5b600101600156", config);
    let stale = analysis
        .states()
        .iter()
        .find_map(|state| {
            state
                .execution_evidence()
                .filter(|evidence| !evidence.is_current())
        })
        .expect("the loop grows its joined entry before the next transfer");
    assert!(!stale.instruction_values.is_empty());
    assert!(stale.instruction_values().is_none());
}

#[test]
fn result_observations_preserve_small_work_budget_receipts() {
    let config = config();
    let domain = config.domain().unwrap();
    let admission_work = |world: &World, entry: &Entry, input: &analysis::MachinePayload| {
        assert_eq!(input.store.touched_in_transaction(address(3)), Some(false));
        let probe = |maximum| {
            let mut budget = WorkBudget::new(maximum);
            let execution = transfer::execute(
                world,
                entry,
                input.clone(),
                &config,
                ValueAlgebra::new(domain).borrowed(),
                &RecentDistinctJumps,
                &mut budget,
                StorageReadPolicy::Abstract,
            );
            (execution, budget)
        };
        // The first PUSH visit costs exactly one semantic unit. Locate its
        // admission boundary under the original policy, without copying the
        // private entry reservation formula into this receipt regression.
        let (full, _) = probe(config.max_work);
        assert_eq!(full.executed_pcs.first(), Some(&0));
        let (mut minimum, mut maximum) = (1, config.max_work);
        while minimum < maximum {
            let midpoint = minimum + (maximum - minimum) / 2;
            if probe(midpoint).0.executed_pcs.is_empty() {
                minimum = midpoint + 1;
            } else {
                maximum = midpoint;
            }
        }
        let admission = minimum.checked_sub(1).unwrap();
        assert!(admission > 0);
        for maximum in [0, admission - 1, admission] {
            let (execution, budget) = probe(maximum);
            let admitted = maximum == admission;
            assert!(budget.exhausted());
            assert_eq!(budget.used(), if admitted { admission } else { 0 });
            assert_eq!(execution.payload, *input);
            assert!(execution.executed_pcs.is_empty());
            assert!(execution.instruction_progress.is_empty());
            assert!(execution.instruction_values.is_empty());
            assert!(execution.completed_calls.is_empty());
            assert!(execution.successors.is_empty());
            assert!(execution.outcomes.is_empty());
            assert!(execution.diagnostics.is_empty());
            assert_eq!(
                execution.frontiers,
                vec![transfer::ExecutionFrontier {
                    pc: 0,
                    reason: FrontierReason::Work,
                    target: admitted.then(|| input.key()),
                    range: None,
                    payload: None,
                }]
            );
        }
        admission
    };

    let (world, entry) = fixture(&[(0x101, "600200")]);
    let initial = transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
    let admission = admission_work(&world, &entry, &initial);
    let pushed = U256::from(2);
    let push_setup_work = ValueAlgebra::new(domain)
        .literal_quote_work(pushed)
        .saturating_add(AbstractValue::constant(pushed).clone_work());
    // Keep the declared PUSH factory/receipt setup separate from entry
    // admission and semantic visits. STOP costs one visit plus its operation,
    // then needs checked finalization before it can install an outcome.
    for (semantic_work, expected_pcs, expected_progress) in [
        (0, vec![], vec![]),
        (1, vec![0], vec![InstructionProgress::Completed]),
        (
            2,
            vec![0, 2],
            vec![
                InstructionProgress::Completed,
                InstructionProgress::OperandsConsumed,
            ],
        ),
        (
            3,
            vec![0, 2],
            vec![
                InstructionProgress::Completed,
                InstructionProgress::Dispatched,
            ],
        ),
    ] {
        let setup_work = if semantic_work == 0 {
            0
        } else {
            push_setup_work
        };
        let maximum = admission + setup_work + semantic_work;
        let mut budget = WorkBudget::new(maximum);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            ValueAlgebra::new(domain).borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert!(budget.exhausted());
        assert_eq!(budget.used(), maximum);
        assert_eq!(budget.used() - admission - setup_work, semantic_work);
        assert_eq!(execution.executed_pcs, expected_pcs);
        assert_eq!(execution.instruction_progress, expected_progress);
        assert!(execution.successors.is_empty());
        assert!(execution.completed_calls.is_empty());
        assert!(execution.diagnostics.is_empty());
        assert!(execution.outcomes.is_empty());
        assert_eq!(
            execution.frontiers,
            vec![transfer::ExecutionFrontier {
                pc: if semantic_work == 0 { 0 } else { 2 },
                reason: FrontierReason::Work,
                target: Some(execution.payload.key()),
                range: None,
                payload: None,
            }]
        );
        assert_eq!(
            execution
                .instruction_values
                .iter()
                .map(|value| (value.instruction_index(), value.numeric().singleton()))
                .collect::<Vec<_>>(),
            if semantic_work == 0 {
                vec![]
            } else {
                vec![(0, Some(U256::from(2)))]
            }
        );
        assert_eq!(
            execution
                .payload
                .active()
                .stack
                .iter()
                .map(AbstractValue::singleton)
                .collect::<Vec<_>>(),
            if semantic_work == 0 {
                vec![]
            } else {
                vec![Some(U256::from(2))]
            }
        );
        assert_eq!(execution.payload.store, initial.store);
    }

    // Derive the cleanup fee from the checked lifecycle path used by STOP,
    // independently of any successful transfer budget. Test both sides of its
    // final reservation as well as the zero-cleanup dispatch refusal above.
    let mut finalized_store = initial.store.clone();
    let mut finalization_budget = WorkBudget::new(config.max_work);
    finalized_store
        .finalize_transaction_checked(domain, None, &mut finalization_budget)
        .unwrap();
    assert!(!finalization_budget.exhausted());
    let finalization_work = finalization_budget.used();
    assert!(finalization_work > 0);
    for remaining in [finalization_work - 1, finalization_work] {
        let mut expected_store = initial.store.clone();
        let mut cleanup_budget = WorkBudget::new(remaining);
        let completed = expected_store
            .finalize_transaction_checked(domain, None, &mut cleanup_budget)
            .is_ok();
        assert_eq!(completed, remaining == finalization_work);
        let mut budget = WorkBudget::new(admission + push_setup_work + 3 + remaining);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            ValueAlgebra::new(domain).borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(
            budget.used(),
            admission + push_setup_work + 3 + cleanup_budget.used()
        );
        assert_eq!(budget.exhausted(), !completed);
        assert_eq!(execution.executed_pcs, vec![0, 2]);
        assert_eq!(
            execution.instruction_progress,
            vec![
                InstructionProgress::Completed,
                InstructionProgress::Dispatched
            ]
        );
        assert_eq!(execution.instruction_values.len(), 1);
        assert_eq!(execution.instruction_values[0].instruction_index(), 0);
        assert_eq!(
            execution.instruction_values[0].numeric().singleton(),
            Some(U256::from(2))
        );
        assert_eq!(execution.payload.active().stack.len(), 1);
        assert_eq!(
            execution.payload.active().stack[0].singleton(),
            Some(U256::from(2))
        );
        assert_eq!(execution.payload.store, initial.store);
        assert!(execution.successors.is_empty());
        assert!(execution.completed_calls.is_empty());
        assert!(execution.diagnostics.is_empty());
        if completed {
            assert!(execution.frontiers.is_empty());
            assert_eq!(execution.outcomes.len(), 1);
            assert_eq!(execution.outcomes[0].kind, OutcomeKind::Return);
            assert_eq!(execution.outcomes[0].data.exact_bytes(), Some(Vec::new()));
            assert_eq!(execution.outcomes[0].store, finalized_store);
        } else {
            assert_eq!(expected_store, initial.store);
            assert!(execution.outcomes.is_empty());
            assert_eq!(
                execution.frontiers,
                vec![transfer::ExecutionFrontier {
                    pc: 2,
                    reason: FrontierReason::Work,
                    target: Some(execution.payload.key()),
                    range: None,
                    payload: None,
                }]
            );
        }
    }

    let (world, entry) = fixture(&[(0x101, "6003565b00")]);
    let initial = transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
    let admission = admission_work(&world, &entry, &initial);
    let pushed = U256::from(3);
    let push_setup_work = ValueAlgebra::new(domain)
        .literal_quote_work(pushed)
        .saturating_add(AbstractValue::constant(pushed).clone_work());
    // JUMP pays its history-navigation work before dispatch, then reserves a
    // separate successor visit. Neither refusal may invent a completed edge.
    for semantic_work in [3, 4, 5] {
        let maximum = admission + push_setup_work + semantic_work;
        let mut budget = WorkBudget::new(maximum);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            ValueAlgebra::new(domain).borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(budget.used(), maximum);
        assert_eq!(budget.used() - admission - push_setup_work, semantic_work);
        assert_eq!(budget.exhausted(), semantic_work < 5);
        assert_eq!(execution.executed_pcs, vec![0, 2]);
        assert_eq!(
            execution.instruction_progress,
            vec![
                InstructionProgress::Completed,
                if semantic_work == 3 {
                    InstructionProgress::OperandsConsumed
                } else {
                    InstructionProgress::Dispatched
                },
            ]
        );
        assert!(execution.outcomes.is_empty());
        assert!(execution.completed_calls.is_empty());
        assert!(execution.diagnostics.is_empty());
        assert_eq!(execution.instruction_values.len(), 1);
        assert_eq!(execution.instruction_values[0].instruction_index(), 0);
        assert_eq!(
            execution.instruction_values[0].numeric().singleton(),
            Some(U256::from(3))
        );
        assert!(execution.payload.active().stack.is_empty());
        if semantic_work < 5 {
            assert!(execution.successors.is_empty());
            assert_eq!(
                execution.frontiers,
                vec![crate::analysis::transfer::ExecutionFrontier {
                    pc: 2,
                    reason: FrontierReason::Work,
                    target: Some(execution.payload.key()),
                    range: None,
                    payload: None,
                }]
            );
        } else {
            assert!(execution.frontiers.is_empty());
            assert_eq!(execution.successors.len(), 1);
            assert_eq!(
                execution.successors[0].kind,
                MachineEdgeKind::Intraprocedural(analysis::EdgeKind::Jump)
            );
            assert_eq!(
                execution.successors[0]
                    .payload
                    .active()
                    .key
                    .basic_block_index,
                1
            );
        }
    }

    let (world, entry) = fixture(&[(0x101, "5f3500")]);
    let initial = transfer::initial(&world, &entry, ValueAlgebra::new(domain).borrowed()).unwrap();
    let admission = admission_work(&world, &entry, &initial);
    let pushed = U256::from(0);
    let push_setup_work = ValueAlgebra::new(domain)
        .literal_quote_work(pushed)
        .saturating_add(AbstractValue::constant(pushed).clone_work());
    let initial_snapshots = serde_json::to_value((
        &initial.store,
        &initial.active().calldata,
        &initial.active().memory,
    ))
    .unwrap();
    // One semantic unit stops before the load's visit and preserves its operand.
    // The original 10,000-unit allowance pays the scalar scan and byte operation
    // but cannot pay the complete word plan. Retain its popped-operand phase,
    // without an output value or any installed state change.
    for semantic_work in [1, 10_000] {
        let maximum = admission + push_setup_work + semantic_work;
        let mut budget = WorkBudget::new(maximum);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            ValueAlgebra::new(domain).borrowed(),
            &RecentDistinctJumps,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        let before_load = semantic_work == 1;
        assert!(budget.exhausted());
        if before_load {
            assert_eq!(budget.used() - admission - push_setup_work, 1);
        } else {
            let scalar_scan = 32 * (domain.capacity() + 32);
            assert!(budget.used() - admission - push_setup_work > 2 + scalar_scan);
            assert!(budget.used() - admission - push_setup_work <= semantic_work);
        }
        assert_eq!(
            execution.executed_pcs,
            if before_load { vec![0] } else { vec![0, 1] }
        );
        assert_eq!(
            execution.instruction_progress,
            if before_load {
                vec![InstructionProgress::Completed]
            } else {
                vec![
                    InstructionProgress::Completed,
                    InstructionProgress::OperandsConsumed,
                ]
            }
        );
        assert_eq!(execution.instruction_values.len(), 1);
        assert_eq!(execution.instruction_values[0].instruction_index(), 0);
        assert_eq!(
            execution.instruction_values[0].numeric().singleton(),
            Some(U256::ZERO)
        );
        assert_eq!(
            execution.payload.active().stack.len(),
            usize::from(before_load)
        );
        if before_load {
            assert_eq!(
                execution.payload.active().stack[0].singleton(),
                Some(U256::ZERO)
            );
        }
        assert!(execution.successors.is_empty());
        assert!(execution.completed_calls.is_empty());
        assert!(execution.outcomes.is_empty());
        assert!(execution.diagnostics.is_empty());
        assert_eq!(
            execution.frontiers,
            vec![crate::analysis::transfer::ExecutionFrontier {
                pc: 1,
                reason: FrontierReason::Work,
                target: Some(execution.payload.key()),
                range: None,
                payload: None,
            }]
        );
        assert_eq!(
            serde_json::to_value((
                &execution.payload.store,
                &execution.payload.active().calldata,
                &execution.payload.active().memory,
            ))
            .unwrap(),
            initial_snapshots
        );
    }
}

#[test]
fn summary_replay_shares_numeric_and_source_receipts_without_dispatch_results() {
    let call = "5f5f5f5f5f6102006207a120f1";
    let caller = format!("{call}50{call}5000");
    let (world, entry) = fixture(&[(0x101, &caller), (0x200, "5f545f5260205ff3")]);
    let mut enabled_config = config();
    enabled_config.use_summaries = true;
    let enabled = analysis::analyze_world(world.clone(), entry.clone(), enabled_config).unwrap();
    let disabled = analysis::analyze_world(world, entry, config()).unwrap();
    assert_eq!(enabled.status(), Status::Converged);
    assert_eq!(disabled.status(), Status::Converged);
    assert!(enabled.summary_stats().hits > 0);
    let record = enabled
        .summaries()
        .iter()
        .find(|record| !record.reused_at.is_empty())
        .unwrap();
    let replayed = enabled.states()[record.reused_at[0]]
        .execution_evidence()
        .unwrap();
    let values = replayed.instruction_values().unwrap();
    let original = disabled
        .states()
        .iter()
        .find(|state| state.active().code_address == address(0x200))
        .unwrap()
        .execution_evidence()
        .unwrap();
    assert_eq!(values, original.instruction_values().unwrap());
    assert_eq!(values[1].instruction_index(), 1);
    assert_eq!(values[1].numeric().singleton(), Some(U256::from(7)));
    assert_eq!(
        values[1].provenance().origins().sources().unwrap(),
        &std::collections::BTreeSet::from([Origin::Storage])
    );
    for state in enabled.states() {
        let evidence = state.execution_evidence().unwrap();
        assert!(evidence.instruction_values().unwrap().iter().all(|value| {
            evidence.instructions()[value.instruction_index()] == InstructionProgress::Completed
        }));
    }
}

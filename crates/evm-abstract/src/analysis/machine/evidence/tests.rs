use super::{InstructionProgress, InstructionValue};
use crate::{
    Address, Fork, U256,
    analysis::{
        self, ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind, Status, WorldAnalysis,
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
    let mut memory_config = config();
    memory_config.max_memory_bytes = 32;
    for (code, config, expected_phase, expected_indices) in [
        (
            "602a60405100",
            memory_config,
            InstructionProgress::OperandsConsumed,
            vec![0, 1],
        ),
        (
            "5f01600100",
            config(),
            InstructionProgress::Faulted,
            vec![0],
        ),
        ("fe", config(), InstructionProgress::Faulted, vec![]),
    ] {
        let analysis = run(code, config);
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
    let (world, entry) = fixture(&[(0x101, "600200")]);
    let initial = transfer::initial(&world, &entry, domain).unwrap();
    // These are the original transfer boundaries: PUSH costs one visit, STOP
    // costs one visit plus its operation. Recording the result consumes no
    // semantic work and must not move any of those boundaries.
    for (maximum, expected_pcs, expected_progress) in [
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
        let mut budget = WorkBudget::new(maximum);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(budget.used(), maximum);
        assert_eq!(execution.executed_pcs, expected_pcs);
        assert_eq!(execution.instruction_progress, expected_progress);
        assert!(execution.successors.is_empty());
        assert!(execution.diagnostics.is_empty());
        if maximum < 3 {
            assert!(execution.outcomes.is_empty());
            assert_eq!(
                execution.frontiers,
                vec![crate::analysis::transfer::ExecutionFrontier {
                    pc: if maximum == 0 { 0 } else { 2 },
                    reason: FrontierReason::Work,
                    target: Some(execution.payload.key()),
                    range: None,
                    payload: None,
                }]
            );
        } else {
            assert!(execution.frontiers.is_empty());
            assert_eq!(execution.outcomes.len(), 1);
            assert_eq!(execution.outcomes[0].kind, OutcomeKind::Return);
        }
        assert_eq!(execution.instruction_values.len(), usize::from(maximum > 0));
        assert_eq!(
            execution.payload.active().stack.len(),
            usize::from(maximum > 0)
        );
    }

    let (world, entry) = fixture(&[(0x101, "6003565b00")]);
    let initial = transfer::initial(&world, &entry, domain).unwrap();
    for maximum in [3, 4] {
        let mut budget = WorkBudget::new(maximum);
        let execution = transfer::execute(
            &world,
            &entry,
            initial.clone(),
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
        );
        assert_eq!(budget.used(), maximum);
        assert_eq!(execution.executed_pcs, vec![0, 2]);
        assert_eq!(
            execution.instruction_progress,
            vec![
                InstructionProgress::Completed,
                InstructionProgress::Dispatched,
            ]
        );
        assert!(execution.outcomes.is_empty());
        assert_eq!(execution.instruction_values.len(), 1);
        if maximum == 3 {
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

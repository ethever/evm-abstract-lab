//! Known bytecode retains its semantics when a repeated entry becomes relative.

use crate::{
    analysis::{
        ExecutionConfig, FrameCode, FrontierReason, OutcomeKind, ProcedureSummaryKind, Status,
        analyze_world,
        machine::fixtures::{address, fixture, word},
    },
    domain::Domain,
    world::{Account, Entry, World},
};
use alloy_primitives::U256;

fn observed_fixture(accounts: &[(u64, &str)], owner: u64, initial: u64) -> (World, Entry) {
    let (empty, entry) = fixture(&[]);
    let mut world = World::new(empty.fork(), "known recursion regression");
    for (number, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = word(1_000_000);
        if *number == owner {
            account.storage.insert(U256::ZERO, word(initial));
        }
        world.insert(address(*number), account).unwrap();
    }
    (world, entry)
}

fn check_entry_gas_failure(
    result: &crate::analysis::WorldAnalysis,
    outcome: &crate::analysis::MachineOutcome,
) {
    assert_eq!(outcome.kind, OutcomeKind::Failure);
    assert_eq!(
        outcome.state, 0,
        "only the existing entry gas-failure seed is allowed"
    );
    assert_eq!(outcome.data.len().singleton(), Some(U256::ZERO));
    assert_eq!(outcome.store, result.states()[0].entry.store);
}

mod admission;

#[test]
fn direct_known_recursion_closes_with_default_depth_and_without_unknown_effects() {
    // Every entered frame writes 11, then CALLs itself and returns 42 even when
    // admission fails. The unchanged other account is outside this code's effects.
    let code = "600b5f555f5f5f5f5f6101016207a120f150602a5f5260205ff3";
    for use_summaries in [false, true] {
        let (world, entry) = observed_fixture(&[(0x101, code), (0x102, "00")], 0x102, 5);
        let config = ExecutionConfig {
            use_summaries,
            ..ExecutionConfig::default()
        };
        let result = analyze_world(world, entry, config).unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(
            result
                .states()
                .iter()
                .all(|state| state.entry.call_stack.depth() <= 3)
        );
        assert!(
            result
                .states()
                .iter()
                .any(|state| state.entry.call_stack.depth() == 3),
            "a direct edge must repeat before the relative solver takes over"
        );
        let records = result.procedure_summaries();
        assert!(!records.is_empty());
        assert!(records.iter().all(|record| {
            record.kind == ProcedureSummaryKind::KnownRecursion
                && record.closed
                && record.root_procedure.is_some()
                && record
                    .procedures
                    .iter()
                    .all(|procedure| procedure.frame.mode == FrameCode::Runtime)
        }));
        assert!(records.iter().any(|record| record.reused > 0));
        assert!(
            result
                .outcomes()
                .iter()
                .any(|outcome| outcome.kind == OutcomeKind::Return)
        );
        for outcome in result.outcomes() {
            if outcome.kind == OutcomeKind::Failure {
                check_entry_gas_failure(&result, outcome);
                continue;
            }
            assert_eq!(
                outcome.kind,
                OutcomeKind::Return,
                "state {} diagnostics {:?}",
                outcome.state,
                result.diagnostics()
            );
            assert_eq!(
                outcome
                    .data
                    .read_word(&word(0), Domain::default())
                    .singleton(),
                Some(U256::from(42))
            );
            assert_eq!(
                outcome
                    .store
                    .read(address(0x101), &word(0), Domain::default())
                    .singleton(),
                Some(U256::from(11))
            );
            assert_eq!(
                outcome
                    .store
                    .read(address(0x102), &word(0), Domain::default())
                    .singleton(),
                Some(U256::from(5))
            );
        }
    }
}

#[test]
fn no_base_return_recursion_derives_revert_and_rollback_from_call_rejection() {
    let code = "600b5f555f5f5f5f5f6101016207a120f15060635f5260205ffd";
    let (world, entry) = observed_fixture(&[(0x101, code)], 0x101, 7);
    let result = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(!result.procedure_summaries().is_empty());
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| record.closed)
    );
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Revert),
        "maximum-depth rejection must resume real parents"
    );
    for outcome in result.outcomes() {
        if outcome.kind == OutcomeKind::Failure {
            check_entry_gas_failure(&result, outcome);
            continue;
        }
        assert_eq!(
            outcome.kind,
            OutcomeKind::Revert,
            "state {} diagnostics {:?}",
            outcome.state,
            result.diagnostics()
        );
        assert_eq!(
            outcome
                .data
                .read_word(&word(0), Domain::default())
                .singleton(),
            Some(U256::from(99))
        );
        assert_eq!(
            outcome
                .store
                .read(address(0x101), &word(0), Domain::default())
                .singleton(),
            Some(U256::from(7))
        );
    }
}

#[test]
fn recursive_component_without_any_terminal_retains_an_explicit_frontier() {
    // A CALL followed by an unconditional loop has no modeled terminal even
    // after maximum-depth admission fails. Gas exhaustion is not silently lost.
    let code = "5f5f5f5f5f6101016207a120f15b600d56";
    let (world, entry) = fixture(&[(0x101, code)]);
    let result = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(result.status(), Status::Incomplete);
    assert!(
        result
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::RecursiveNoTerminal)
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .all(|record| !record.closed)
    );
    assert!(
        result
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind != OutcomeKind::Return)
    );
    for outcome in result.outcomes() {
        check_entry_gas_failure(&result, outcome);
    }
}

#[test]
fn repeated_outer_calls_keep_procedure_provenance_out_of_exact_graph_replay() {
    let a = "5f5f5f5f5f6101026207a120f1505f5f5f5f5f6101026207a120f150602a5f5260205ff3";
    let b = "5f5f5f5f5f6101026207a120f150602a5f5260205ff3";
    let (world, entry) = fixture(&[(0x101, a), (0x102, b)]);
    let result = analyze_world(world, entry, ExecutionConfig::default()).unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert_eq!(
        result.summary_stats().hits,
        0,
        "a Summary edge cannot lose its procedure record on replay"
    );
    assert!(result.summary_stats().rejected_incomplete > 0);
    let records = result.procedure_summaries();
    assert!(records.len() >= 2);
    assert!(
        records
            .iter()
            .all(|record| record.closed && record.kind == ProcedureSummaryKind::KnownRecursion)
    );
    for edge in result
        .edges()
        .iter()
        .filter(|edge| matches!(edge.kind, crate::analysis::MachineEdgeKind::Summary(_)))
    {
        assert!(
            records
                .iter()
                .any(|record| record.source_state == edge.from && record.root_procedure.is_some()),
            "missing procedure source {}",
            edge.from
        );
    }
    assert!(
        result
            .outcomes()
            .iter()
            .any(|outcome| outcome.kind == OutcomeKind::Return)
    );
}

#[test]
fn recursive_procedure_keeps_private_return_pcs_finite_at_an_acyclic_helper() {
    // Four callers push different private return PCs and enter one helper.
    // The marker at 336 returns 99 but is unreachable from every real input.
    // Widening this acyclic join used to reach it and publish {42,99}.
    let mut code = "60205f5f3760005f60205f60006101015af1505f3560031680600014610034578060011461003d57806002146100465761004f565b50610100610140565b50610110610140565b50610120610140565b5061013061014056".to_owned();
    for return_pc in [256, 272, 288, 304] {
        code.push_str(&"00".repeat(return_pc - code.len() / 2));
        code.push_str("5b602a5f5260205ff3");
    }
    code.push_str(&"00".repeat(320 - code.len() / 2));
    code.push_str("5b56");
    code.push_str(&"00".repeat(336 - code.len() / 2));
    code.push_str("5b60635f5260205ff3");
    let (world, mut entry) = fixture(&[(0x101, &code)]);
    entry.environment.calldata = crate::world::ByteArray::unknown();
    let program = crate::bytecode::Program::from_hex_with_fork(&code, world.fork()).unwrap();
    let marker = program.jumpdest_blocks()[&336];
    for use_summaries in [true, false] {
        let mut config = ExecutionConfig {
            max_work: 1_000_000_000,
            use_summaries,
            ..ExecutionConfig::default()
        };
        config.analysis.max_constants = 512;
        config.analysis.relations.enabled = false;
        let domain = config.domain().unwrap();
        let result = analyze_world(world.clone(), entry.clone(), config).unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(result.frontiers().is_empty());
        assert!(!result.procedure_summaries().is_empty());
        for record in result.procedure_summaries() {
            assert!(record.closed);
            for procedure in &record.procedures {
                assert!(!procedure.covered_blocks.contains(&marker));
                let returns = procedure
                    .outputs
                    .iter()
                    .filter(|output| output.kind == OutcomeKind::Return)
                    .collect::<Vec<_>>();
                assert!(!returns.is_empty());
                for output in returns {
                    assert_eq!(output.data.len().singleton(), Some(U256::from(32)));
                    assert_eq!(
                        output.data.read_word(&word(0), domain).singleton(),
                        Some(U256::from(42))
                    );
                }
            }
        }
        for outcome in result.outcomes() {
            if outcome.kind == OutcomeKind::Failure {
                check_entry_gas_failure(&result, outcome);
            } else {
                assert_eq!(outcome.kind, OutcomeKind::Return);
                assert_eq!(
                    outcome.data.read_word(&word(0), domain).singleton(),
                    Some(U256::from(42))
                );
            }
        }
    }
}

fn sequential_helpers(nested: bool) -> String {
    // After the self CALL, visit the same private helper four times in order.
    // Its distinct return PCs are real stack values, rather than numeric growth.
    let mut code = "60205f5f3760005f60205f60006101015af150".to_owned();
    let helper = if nested { 400 } else { 320 };
    code.push_str(&format!("61010061{helper:04x}56"));
    for (pc, next) in [(256, 272), (272, 288), (288, 304)] {
        code.push_str(&"00".repeat(pc - code.len() / 2));
        code.push_str(&format!("5b61{next:04x}61{helper:04x}56"));
    }
    code.push_str(&"00".repeat(304 - code.len() / 2));
    code.push_str("5b602a5f5260205ff3");
    if !nested {
        code.push_str(&"00".repeat(320 - code.len() / 2));
        code.push_str("5b56");
    }
    code.push_str(&"00".repeat(336 - code.len() / 2));
    code.push_str("5b60635f5260205ff3");
    if nested {
        code.push_str(&"00".repeat(400 - code.len() / 2));
        code.push_str("5b6101a06101c056");
        code.push_str(&"00".repeat(416 - code.len() / 2));
        code.push_str("5b56");
        code.push_str(&"00".repeat(448 - code.len() / 2));
        code.push_str("5b56");
    }
    code
}

fn check_sequential_helpers(nested: bool) {
    let code = sequential_helpers(nested);
    let (world, mut entry) = fixture(&[(0x101, &code)]);
    entry.environment.calldata = crate::world::ByteArray::unknown();
    let program = crate::bytecode::Program::from_hex_with_fork(&code, world.fork()).unwrap();
    let marker = program.jumpdest_blocks()[&336];
    for use_summaries in [true, false] {
        let mut config = ExecutionConfig {
            max_work: 1_000_000_000,
            use_summaries,
            ..ExecutionConfig::default()
        };
        config.analysis.context_depth = 128;
        config.analysis.max_constants = 512;
        config.analysis.relations.enabled = false;
        let domain = config.domain().unwrap();
        let result = analyze_world(world.clone(), entry.clone(), config).unwrap();
        assert_eq!(
            result.status(),
            Status::Converged,
            "{:?}",
            result.frontiers()
        );
        assert!(result.frontiers().is_empty());
        assert!(!result.procedure_summaries().is_empty());
        for record in result.procedure_summaries() {
            assert!(record.closed);
            for procedure in &record.procedures {
                assert!(!procedure.covered_blocks.contains(&marker));
                let returns = procedure
                    .outputs
                    .iter()
                    .filter(|output| output.kind == OutcomeKind::Return)
                    .collect::<Vec<_>>();
                assert!(!returns.is_empty());
                for output in returns {
                    assert_eq!(output.data.len().singleton(), Some(U256::from(32)));
                    assert_eq!(
                        output.data.read_word(&word(0), domain).singleton(),
                        Some(U256::from(42))
                    );
                }
            }
        }
        assert!(
            result
                .states()
                .iter()
                .all(|state| !state.executed_pcs.contains(&336))
        );
        let returns = result
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .collect::<Vec<_>>();
        assert!(!returns.is_empty());
        for output in returns {
            assert_eq!(
                output.data.read_word(&word(0), domain).singleton(),
                Some(U256::from(42))
            );
        }
    }
}

#[test]
fn sequential_private_helpers_keep_each_real_return_pc_without_false_feedback() {
    check_sequential_helpers(false);
}

#[test]
fn nested_private_helpers_keep_the_recent_outer_caller_and_exact_outputs() {
    check_sequential_helpers(true);
}

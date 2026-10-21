//! Evidence regressions for partial numerical procedure closure.

use crate::domain::ProductAlgebra;

use super::*;
use crate::analysis::machine::fixtures::{address, fixture, word};
use crate::analysis::testing::AbstractLog;
use crate::world::LogKey;
use alloy_primitives::U256;
use std::collections::VecDeque;

mod context;
mod policy;
mod projection;
mod relation_boundary_probe;
mod resume_status;
mod stack_alias;
mod storage_observations;
mod worklist;

#[test]
fn recursive_call_success_keeps_the_copied_output_separate_from_failure_memory() {
    let code = "610000351561005f57610001610000350361000052306101011461003e576100206100006100206100006100006101015af11561006f57610020610000f35b6100206100006100206100006100006102015af11561006f57610020610000f35b6100005461000052610020610000f35b610000610000fd";
    let (empty, mut entry) = fixture(&[]);
    entry.environment.calldata = ByteArray::unknown();
    let mut world = World::new(empty.fork(), "recursive output partition regression");
    for (owner, value) in [(0x101, 5), (0x201, 9)] {
        let mut account = crate::analysis::testing::Account::from_hex(code, world.fork()).unwrap();
        account.balance = word(1_000_000);
        account.storage.insert(U256::ZERO, word(value));
        world.insert(address(owner), account).unwrap();
    }
    let config = crate::analysis::ExecutionConfig {
        max_work: 200_000_000,
        ..Default::default()
    };
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut queue = VecDeque::from([crate::analysis::transfer::procedure::testing::initial(
        &world, &entry, domain,
    )
    .unwrap()]);
    let recursive = loop {
        let payload = queue.pop_front().expect("known cycle must be reachable");
        if is_recursive_entry(&payload) {
            break payload;
        }
        assert!(payload.call_stack.depth() <= 3);
        let execution = execute_inner(
            &world,
            &entry,
            payload,
            &config,
            domain,
            &mut budget,
            StorageReadPolicy::Abstract,
            false,
        );
        queue.extend(
            execution
                .successors
                .into_iter()
                .map(|successor| successor.payload),
        );
    };
    let mut table = Table::default();
    let (_, record) = table.execute(
        0,
        &world,
        &entry,
        recursive,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(record.closed);
    for procedure in record.procedures {
        for output in procedure
            .outputs
            .into_iter()
            .filter(|output| output.kind == OutcomeKind::Return)
        {
            let word = output.data.read_word(&word(0), domain);
            assert!(
                word.constants()
                    .is_some_and(|values| values.iter().all(|value| [
                        U256::from(5),
                        U256::from(9)
                    ]
                    .contains(value))),
                "{word}"
            );
        }
    }
    for use_summaries in [true, false] {
        let result = crate::analysis::testing::execute(
            world.clone(),
            entry.clone(),
            crate::analysis::ExecutionConfig {
                // This is a semantic copy/rollback regression. Work-frontier
                // cuts are checked separately against the selected policies.
                max_work: 100_000_000,
                use_summaries,
                root_terminal_coverage: false,
                ..Default::default()
            },
            crate::domain::DomainSpec::default(),
        )
        .unwrap();
        assert_eq!(
            result.status,
            crate::analysis::Status::Converged,
            "summaries={use_summaries} work={} states={} transfers={} frontiers={}",
            result.work,
            result.states.len(),
            result.transfers,
            result.frontiers.len()
        );
        assert!(result.frontiers.is_empty());
        assert!(!result.procedure_summaries().is_empty());
        assert!(
            result
                .procedure_summaries()
                .iter()
                .all(|record| record.closed)
        );
        let returns = result
            .outcomes
            .iter()
            .filter(|output| output.kind == OutcomeKind::Return)
            .collect::<Vec<_>>();
        assert!(!returns.is_empty());
        for output in returns {
            let value = output.data.read_word(&word(0), domain);
            assert!(
                value
                    .constants()
                    .is_some_and(|values| values.iter().all(|value| [
                        U256::from(5),
                        U256::from(9),
                    ]
                    .contains(value))),
                "outer state{} {value}",
                output.state,
            );
            for (owner, expected) in [(0x101, 5), (0x201, 9)] {
                assert_eq!(
                    output
                        .store
                        .read(address(owner), &word(0), domain)
                        .singleton(),
                    Some(U256::from(expected)),
                );
            }
        }
    }
}

#[test]
fn parked_joined_summary_contains_the_current_bound() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let mut input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    input
        .store
        .write(address(0x101), &word(0), &word(0), domain);
    let original = form(&input, &entry, domain).unwrap();
    input
        .store
        .write(address(0x101), &word(0), &word(1), domain);
    let incoming = form(&input, &entry, domain).unwrap();
    let joined = join(&original, &incoming, domain, false).unwrap();
    let mut table = Table::default();
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    solver.frontier_summary(joined, FrontierReason::Budget(Limit::Transfers), 0);
    let frontier = &table.frontiers[0];
    assert_eq!(frontier.reason, FrontierReason::Budget(Limit::Transfers));
    let parked = frontier
        .payload
        .as_ref()
        .expect("current bound fits construction budget");
    let value = parked.store.read(address(0x101), &word(0), domain);
    assert!(value.contains(U256::ZERO));
    assert!(value.contains(U256::ONE));
    assert!(!value.contains(U256::from(2)));
}

#[test]
fn exhausted_ledger_does_not_replace_current_summary_with_an_old_input() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let (world, entry) = fixture(&[(0x101, "00")]);
    let input =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let summary = form(&input, &entry, domain).unwrap();
    let mut table = Table::default();
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(0);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    solver.frontier_summary(summary, FrontierReason::Work, 0);
    assert!(table.frontiers[0].payload.is_none());
    assert_eq!(table.frontiers[0].target.as_ref(), Some(&input.key()));
    assert_eq!(table.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(budget.used(), 0);
}

#[test]
fn symbolic_owner_namespace_is_never_used_for_storage_discovery() {
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::default());
    let domain = domain.borrowed();
    let (world, entry) = fixture(&[(0x101, "5f5400")]);
    let mut payload =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let namespace = owner_namespace(&world, &payload.store);
    let frame = payload.active_mut();
    frame.key.address = namespace;
    frame.key.address_value = AddressInput::Concrete(namespace);
    frame.address_operand = Some(TestValue::unknown_address());
    let config = crate::analysis::ExecutionConfig::default();
    let mut budget = WorkBudget::new(config.max_work);
    let execution = execute_inner(
        &world,
        &entry,
        payload,
        &config,
        domain,
        &mut budget,
        StorageReadPolicy::Discover,
        true,
    );
    assert!(
        !execution
            .frontiers
            .iter()
            .any(|frontier| matches!(frontier.reason, FrontierReason::MissingStorage { .. })),
        "synthetic namespace entered RPC discovery: {:?}",
        execution.frontiers
    );
    assert!(!execution.outcomes.is_empty());
}

#[test]
fn equal_exit_buffers_share_one_invocation_source_but_later_calls_are_fresh() {
    let (world, entry) = fixture(&[]);
    let config = crate::analysis::ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let payload =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    assert_eq!(payload.active().code, FrameCode::Unknown);
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let mut call = || {
        table.execute(
            0,
            &world,
            &entry,
            payload.clone(),
            &config,
            domain,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
    };
    let (first, evidence) = call();
    assert!(evidence.closed);
    let returned = first
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Return)
        .unwrap();
    let reverted = first
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Revert)
        .unwrap();
    assert_eq!(
        returned.data.len().expression(),
        reverted.data.len().expression()
    );
    let first_length = returned.data.len().expression().unwrap().clone();
    let (second, evidence) = call();
    assert!(evidence.closed);
    let returned = second
        .outcomes
        .iter()
        .find(|output| output.kind == OutcomeKind::Return)
        .unwrap();
    assert_ne!(
        returned.data.len().expression(),
        Some(&first_length),
        "separate unknown calls must retain independent length choices"
    );
}

#[test]
fn static_environment_closes_without_analyzing_or_reporting_known_callbacks() {
    // This protected owner loops forever. Static callbacks preserve its values,
    // while touch metadata remains possible. Every caller-visible outcome is in the
    // unknown process's envelope irrespective of this code's closure.
    let (world, mut entry) = fixture(&[(0x102, "5b5f56")]);
    entry.environment.is_static = true;
    let config = crate::analysis::ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let mut payload =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    payload
        .store
        .write(address(0x102), &word(0), &word(42), domain);
    payload
        .store
        .write_transient(address(0x102), &word(0), &word(7), domain);
    payload.store.write_balance(address(0x102), word(19));
    payload.store.write_nonce(address(0x102), word(3));
    payload
        .store
        .emit_log(
            LogKey {
                address: address(0x102),
                code_address: address(0x102),
                code_hash: alloy_primitives::B256::ZERO,
                source_kind: crate::world::LogSourceKind::Runtime,
                pc: 0,
            },
            AbstractLog {
                topics: vec![word(9)],
                data: ByteArray::exact(&[1, 2]),
            },
            domain,
        )
        .unwrap();
    payload.active_mut().saved_store = payload.store.snapshot();
    let mut touched_payload = payload.clone();
    touched_payload.store.may_touch_all();
    let mut rolled_back = touched_payload.store.clone();
    rolled_back.restore(payload.active().saved_store.clone());
    let mut table = Table::default();
    let mut budget = WorkBudget::new(config.max_work);
    let mut counters = Counters::default();
    let result = table
        .solver(
            &world,
            &entry,
            &config,
            &mut budget,
            &mut counters,
            StorageReadPolicy::Abstract,
        )
        .environment(payload.clone());
    assert!(result.frontiers.is_empty());
    assert!(result.successors.is_empty());
    assert_eq!(result.payload, touched_payload);
    assert_eq!(result.outcomes.len(), 3);
    for kind in [
        OutcomeKind::Return,
        OutcomeKind::Revert,
        OutcomeKind::Failure,
    ] {
        let output = result
            .outcomes
            .iter()
            .find(|output| output.kind == kind)
            .unwrap();
        assert_eq!(
            &output.store,
            if kind == OutcomeKind::Return {
                &touched_payload.store
            } else {
                &rolled_back
            }
        );
        if kind == OutcomeKind::Failure {
            assert_eq!(output.data.len().singleton(), Some(U256::ZERO));
        } else {
            assert!(output.data.len().contains(U256::ZERO));
            assert!(output.data.len().contains(U256::from(96)));
            assert!(output.data.byte_at(0, domain).contains(U256::from(255)));
        }
    }
    let (result, evidence) = table.execute(
        0,
        &world,
        &entry,
        payload,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(result.frontiers.is_empty());
    assert!(evidence.closed);
    assert_eq!(evidence.procedures.len(), 1);
    assert_eq!(evidence.procedures[0].frame.mode, FrameCode::Unknown);
    assert!(evidence.procedures[0].covered_blocks.is_empty());
}

#[test]
fn static_environment_keeps_a_typed_frontier_when_envelope_work_is_unavailable() {
    let (world, mut entry) = fixture(&[(0x102, "00")]);
    entry.environment.is_static = true;
    let config = crate::analysis::ExecutionConfig::default();
    let config_numeric = crate::analysis::testing::NumericLimits::default();
    let domain = crate::domain::ValueAlgebra::new(ProductAlgebra::from_spec(config_numeric.spec()));
    let domain = domain.borrowed();
    let payload =
        crate::analysis::transfer::procedure::testing::initial(&world, &entry, domain).unwrap();
    let mut table = Table::default();
    let mut budget = WorkBudget::new(0);
    let mut counters = Counters::default();
    let mut solver = table.solver(
        &world,
        &entry,
        &config,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    let result = solver.environment(payload.clone());
    assert_eq!(result.payload, payload);
    assert!(result.outcomes.is_empty());
    assert!(result.successors.is_empty());
    assert_eq!(result.frontiers.len(), 1);
    assert_eq!(result.frontiers[0].reason, FrontierReason::Work);
    assert_eq!(result.frontiers[0].target.as_ref(), Some(&payload.key()));
    assert_eq!(budget.used(), 0);
}

mod cooperation;

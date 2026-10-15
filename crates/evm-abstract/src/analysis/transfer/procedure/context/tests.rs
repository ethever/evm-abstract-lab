//! Recent sources preserve private callers while repeated loop keys stabilize.

use super::{canonical, work_size};
use crate::{
    analysis::{machine::fixtures::fixture, transfer},
    domain::Domain,
};
use std::collections::BTreeSet;

#[test]
fn older_duplicates_move_to_their_most_recent_position() {
    assert_eq!(canonical(&[1, 2, 3, 2]).unwrap(), [1, 3, 2]);
    assert_eq!(canonical(&[1, 2, 1, 2, 3, 2]).unwrap(), [1, 3, 2]);
    assert!(canonical(&[]).unwrap().is_empty());
}

#[test]
fn nested_helpers_keep_the_recent_outer_callsite() {
    let first = [18, 400];
    let second = [18, 400, 448, 416, 256, 400];
    assert_ne!(canonical(&first).unwrap(), canonical(&second).unwrap());
    assert_eq!(canonical(&second).unwrap(), [18, 448, 416, 256, 400]);
}

#[test]
fn repeated_loop_sources_reach_periodic_keys_without_128_iterations() {
    let mut current = Vec::new();
    let mut contexts = BTreeSet::new();
    for source in [12, 13].into_iter().cycle().take(256) {
        current.push(source);
        current = canonical(&current).unwrap();
        contexts.insert(current.clone());
    }
    assert_eq!(
        contexts,
        BTreeSet::from([vec![12], vec![12, 13], vec![13, 12]])
    );
}

#[test]
fn empty_and_declared_bounded_histories_do_not_grow_and_work_saturates() {
    assert!(canonical(&[]).unwrap().is_empty());
    assert_eq!(work_size(0), 0);
    assert_eq!(work_size(usize::MAX), usize::MAX);
    let history = (0..128).collect::<Vec<_>>();
    assert_eq!(canonical(&history).unwrap(), history);
    assert!(work_size(128) >= 128);
}

#[test]
fn known_forms_keep_and_charge_context_while_unknown_forms_erase_it() {
    let domain = Domain::default();
    for known in [true, false] {
        let (world, entry) = if known {
            fixture(&[(0x101, "00")])
        } else {
            fixture(&[])
        };
        let mut payload = transfer::initial(&world, &entry, domain).unwrap();
        let baseline = super::super::form(&payload, &entry, domain).unwrap();
        payload.active_mut().key.jump_history = vec![1, 2, 3, 2];
        let formed = super::super::form(&payload, &entry, domain).unwrap();
        if known {
            assert_eq!(formed.active().key.jump_history, [1, 3, 2]);
            assert_eq!(formed.work_size(), baseline.work_size() + 3);
        } else {
            assert!(formed.active().key.jump_history.is_empty());
            assert_eq!(formed.work_size(), baseline.work_size());
        }
    }
}

#[test]
fn context_depth_zero_stays_empty_during_actual_loop_execution_and_replay() {
    use super::super::{Counters, StorageReadPolicy, Table, WorkBudget};
    use crate::analysis::{ExecutionConfig, OutcomeKind};
    use alloy_primitives::U256;
    let (world, entry) = fixture(&[(0x101, "5f5b60010180601190106100015750602a5f5260205ff3")]);
    let mut config = ExecutionConfig {
        max_work: 1_000_000_000,
        ..ExecutionConfig::default()
    };
    config.analysis.context_depth = 0;
    config.analysis.max_constants = 512;
    config.analysis.relations.enabled = false;
    let domain = config.domain().unwrap();
    let input = transfer::initial(&world, &entry, domain).unwrap();
    let mut table = Table::default();
    let mut counters = Counters::default();
    let mut budget = WorkBudget::new(config.max_work);
    let (execution, record) = table.execute(
        0,
        &world,
        &entry,
        input,
        &config,
        domain,
        &mut budget,
        &mut counters,
        StorageReadPolicy::Abstract,
    );
    assert!(execution.frontiers.is_empty());
    assert!(record.closed);
    assert!(
        table
            .nodes
            .iter()
            .all(|node| node.payload.active().key.jump_history.is_empty())
    );
    assert!(table.nodes.len() <= 4);
    let returns = execution
        .outcomes
        .iter()
        .filter(|outcome| outcome.kind == OutcomeKind::Return)
        .collect::<Vec<_>>();
    assert!(!returns.is_empty());
    for output in returns {
        assert_eq!(
            output
                .data
                .read_word(&crate::analysis::machine::fixtures::word(0), domain)
                .singleton(),
            Some(U256::from(42))
        );
    }
}

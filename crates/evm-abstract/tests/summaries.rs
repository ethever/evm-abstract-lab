//! Whole-callee relation reuse, exact guards, graph preservation and shared budgets.
#[path = "summaries/oracle.rs"]
mod oracle;

use alloy_primitives::U256;
use evm_abstract::{
    Fork,
    analysis::{
        Config, ExecutionConfig, FrontierReason, MachineEdgeKind, OutcomeKind, Status,
        WorldAnalysis, analyze_world,
    },
    domain::AbstractValue,
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use oracle::address;
use std::collections::BTreeSet;

fn call(op: u8, target: u16, value: u8, output: u8) -> String {
    let value = if matches!(op, 0xf1 | 0xf2) {
        format!("60{value:02x}")
    } else {
        String::new()
    };
    format!("60{output:02x}5f5f5f{value}61{target:04x}6207a120{op:02x}")
}

fn fixture(accounts: &[(u64, &str)]) -> (World, Entry) {
    let mut world = World::new(Fork::Osaka, "test:summary:fixed-snapshot");
    for (number, code) in accounts {
        let mut account = Account::from_hex(code, world.fork()).unwrap();
        account.balance = AbstractValue::constant(U256::from(1_000_000));
        account
            .storage
            .insert(U256::ZERO, AbstractValue::constant(U256::from(7)));
        world.insert(address(*number), account).unwrap();
    }
    let entry = Entry {
        address: address(0x101),
        environment: evm_abstract::world::EvmEnvironment {
            to: (address(0x101)).into(),
            caller: (address(0x1000)).into(),
            value: AbstractValue::constant(U256::ZERO),
            calldata: ByteArray::empty(),
            is_static: false,
            ..evm_abstract::world::EvmEnvironment::default()
        },
    };
    (world, entry)
}

fn relations(analysis: &WorldAnalysis) -> BTreeSet<String> {
    analysis
        .outcomes()
        .iter()
        .map(|outcome| {
            serde_json::to_string(&(outcome.kind, &outcome.data, &outcome.store)).unwrap()
        })
        .collect()
}

fn compare(accounts: &[(u64, &str)]) -> (WorldAnalysis, WorldAnalysis) {
    let (world, entry) = fixture(accounts);
    let enabled = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    let disabled = analyze_world(
        world.clone(),
        entry.clone(),
        ExecutionConfig {
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    for analysis in [&enabled, &disabled] {
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "frontiers={:?}",
            analysis.frontiers()
        );
        ssa::build_world(analysis)
            .unwrap()
            .verify(analysis)
            .unwrap();
        assert_eq!(
            oracle::compare(&world, &entry, analysis),
            OutcomeKind::Return
        );
    }
    assert_eq!(relations(&enabled), relations(&disabled));
    assert!(disabled.summaries().is_empty());
    assert_eq!(disabled.summary_stats().hits, 0);
    (enabled, disabled)
}

#[test]
fn complete_read_only_relation_reuses_across_call_sites_and_output_ranges() {
    let caller = format!(
        "{}50{}5060205ff3",
        call(0xf1, 0x200, 0, 0),
        call(0xf1, 0x200, 0, 32)
    );
    let (enabled, disabled) = compare(&[(0x101, &caller), (0x200, "5f545f5260205ff3")]);
    assert!(
        enabled.summary_stats().hits > 0,
        "{:?}",
        enabled.summary_stats()
    );
    assert!(enabled.transfers() < disabled.transfers());
    let record = enabled
        .summaries()
        .iter()
        .find(|record| !record.reused_at.is_empty())
        .unwrap();
    assert!(
        record
            .outputs
            .iter()
            .any(|output| output.kind == OutcomeKind::Return
                && output.data.len().contains(U256::from(32)))
    );
    assert!(
        record.reused_at.iter().all(|id| enabled.states()[*id]
            .entry
            .active()
            .key
            .basic_block_index
            == 0)
    );
    assert!(
        enabled
            .edges()
            .iter()
            .filter(|edge| edge.kind == MachineEdgeKind::Call)
            .count()
            >= 2
    );
    assert!(
        enabled
            .edges()
            .iter()
            .filter(|edge| edge.kind == MachineEdgeKind::Return)
            .count()
            >= 2
    );
    assert!(
        record
            .reused_at
            .iter()
            .all(|id| !enabled.states()[*id].executed_pcs.is_empty())
    );
    // Replayed receipts must use the new caller's native state IDs, not the
    // original certificate's numbering, and must retain instruction phases.
    let partial = ssa::build_partial_world(&enabled).unwrap();
    partial.verify(&enabled).unwrap();
    assert!(partial.deferred_edges().is_empty());
    assert_eq!(partial.transitions().len(), enabled.edges().len());
    for state in enabled.states() {
        let evidence = state.execution_evidence().unwrap();
        assert!(evidence.is_current());
        assert_eq!(evidence.instructions().len(), state.executed_pcs.len());
        for (to, kind) in evidence.successors() {
            assert!(
                enabled
                    .edges()
                    .iter()
                    .any(|edge| { edge.from == state.id && edge.to == *to && edge.kind == *kind })
            );
        }
    }
}

#[test]
fn full_store_value_caller_and_mode_preconditions_produce_misses() {
    for between in ["60095f55".to_owned(), String::new()] {
        let second = if between.is_empty() {
            call(0xf1, 0x200, 1, 32)
        } else {
            call(0xf1, 0x200, 0, 32)
        };
        let caller = format!("{}50{between}{second}5060205ff3", call(0xf1, 0x200, 0, 32));
        let (enabled, _) = compare(&[(0x101, &caller), (0x200, "345f5260205ff3")]);
        let distinct: BTreeSet<_> = enabled
            .summaries()
            .iter()
            .filter(|record| record.input.frame.state.key.code_address == address(0x200))
            .map(|record| serde_json::to_string(&record.input).unwrap())
            .collect();
        assert!(
            distinct.len() >= 2,
            "changed input incorrectly conflated: {:?}",
            enabled.summaries()
        );
        assert!(enabled.summary_stats().misses >= 2);
    }
    let caller = format!(
        "{}50{}5060205ff3",
        call(0xf1, 0x200, 0, 32),
        call(0xf4, 0x200, 0, 32)
    );
    let (enabled, _) = compare(&[(0x101, &caller), (0x200, "335f5260205ff3")]);
    let callers: BTreeSet<_> = enabled
        .summaries()
        .iter()
        .map(|record| record.input.frame.state.key.caller)
        .collect();
    assert!(callers.contains(&address(0x101).into()) && callers.contains(&address(0x1000).into()));
}

#[test]
fn nested_revert_certificate_preserves_graph_and_rollback_effects() {
    let caller = format!(
        "{}50{}{}5060205ff3",
        call(0xf1, 0x200, 0, 32),
        "5b".repeat(12),
        call(0xf1, 0x200, 0, 32)
    );
    let callee = format!(
        "{}5060085f5d5f5fa0602a5f5260205ffd",
        call(0xf1, 0x201, 0, 0)
    );
    let (enabled, _) = compare(&[(0x101, &caller), (0x200, &callee), (0x201, "60095f5500")]);
    assert!(enabled.summary_stats().hits > 0);
    assert!(enabled.summaries().iter().any(|record| {
        record.input.frame.state.key.code_address == address(0x200)
            && !record.reused_at.is_empty()
            && record
                .outputs
                .iter()
                .any(|output| output.kind == OutcomeKind::Revert)
    }));
    assert!(
        enabled
            .states()
            .iter()
            .filter(|state| state.key.frames.len() == 3)
            .count()
            >= 2
    );
    assert!(
        enabled
            .outcomes()
            .iter()
            .filter(|outcome| outcome.kind == OutcomeKind::Return)
            .all(|outcome| outcome.store.possible_logs().is_empty())
    );
}

#[test]
fn incomplete_recursive_children_never_publish_a_complete_relation() {
    let code = format!("{}00", call(0xf1, 0x101, 0, 0));
    let (world, entry) = fixture(&[(0x101, &code)]);
    let analysis = analyze_world(
        world,
        entry,
        ExecutionConfig {
            max_call_depth: 3,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::CallDepth)
    );
    assert!(analysis.summaries().is_empty());
    assert!(analysis.summary_stats().rejected_incomplete > 0);
}

#[test]
fn interrupted_summary_import_retains_an_explicit_shared_budget_frontier() {
    let caller = format!(
        "{}50{}{}5000",
        call(0xf1, 0x200, 0, 32),
        "5b".repeat(12),
        call(0xf1, 0x200, 0, 32)
    );
    let (world, entry) = fixture(&[(0x101, &caller), (0x200, "6003565b5f545f5260205ff3")]);
    let complete = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert!(complete.summary_stats().hits > 0);
    let certified_states = complete
        .summaries()
        .iter()
        .map(|record| record.state_count)
        .max()
        .unwrap();
    assert!(certified_states > 1);
    let limit = complete.work();
    let mut interrupted = None;
    // Find an actual interruption inside graph import, without depending on
    // incidental work-unit constants from unrelated opcode implementation.
    // Work reservations grew with product domains. Locate the first completed
    // imported node; a coarse numeric stride can skip the entire partial-import window.
    let (mut lo, mut hi) = (1, limit);
    while lo < hi {
        let budget = lo + (hi - lo) / 2;
        let analysis = analyze_world(
            world.clone(),
            entry.clone(),
            ExecutionConfig {
                max_work: budget,
                ..ExecutionConfig::default()
            },
        )
        .unwrap();
        if analysis.summary_stats().hits > 0 && analysis.summary_stats().imported_states > 0 {
            hi = budget;
        } else {
            lo = budget + 1;
        }
    }
    let analysis = analyze_world(
        world,
        entry,
        ExecutionConfig {
            max_work: lo,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    if analysis.summary_stats().hits > 0
        && analysis.summary_stats().imported_states > 0
        && analysis.summary_stats().imported_states < certified_states
        && analysis
            .frontiers()
            .iter()
            .any(|f| f.reason == FrontierReason::SummaryWork)
    {
        interrupted = Some((lo, analysis));
    }
    let (budget, analysis) =
        interrupted.expect("no budget interruption during selected summary reuse");
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(analysis.work() <= budget);
    assert!(ssa::build_world(&analysis).is_err());
    let partial = ssa::build_partial_world(&analysis).unwrap();
    partial.verify(&analysis).unwrap();
    assert_eq!(partial.status(), Status::Incomplete);
    assert!(
        partial
            .frontiers()
            .iter()
            .any(|frontier| { frontier.reason == FrontierReason::SummaryWork })
    );
}

#[test]
fn creation_changes_nonce_and_code_overlay_preconditions_before_later_calls() {
    fn push2(bytes: &mut Vec<u8>, value: usize) {
        bytes.extend([0x61, (value >> 8) as u8, value as u8]);
    }
    // Deploy one STOP byte between two reads of an unchanged callee. The full
    // transaction precondition changes even though that callee's slots do not.
    let init = revm::primitives::hex::decode("5f5f526001601ff3").unwrap();
    let mut caller = revm::primitives::hex::decode(call(0xf1, 0x200, 0, 0)).unwrap();
    caller.push(0x50);
    push2(&mut caller, init.len());
    let offset_patch = caller.len();
    push2(&mut caller, 0);
    caller.extend([0x5f, 0x39]);
    push2(&mut caller, init.len());
    caller.extend([0x5f, 0x5f, 0xf0, 0x50]);
    caller.extend(revm::primitives::hex::decode(call(0xf1, 0x200, 0, 0)).unwrap());
    caller.extend([0x50, 0x00]);
    let offset = caller.len();
    caller[offset_patch + 1] = (offset >> 8) as u8;
    caller[offset_patch + 2] = offset as u8;
    caller.extend(init);
    let hex = revm::primitives::hex::encode(caller);
    let (mut world, entry) = fixture(&[(0x101, &hex), (0x200, "5f545f5260205ff3")]);
    let created = address(0x101).create(0);
    world.insert(created, Account::absent()).unwrap();
    let enabled = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    let disabled = analyze_world(
        world.clone(),
        entry.clone(),
        ExecutionConfig {
            use_summaries: false,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    for analysis in [&enabled, &disabled] {
        assert_eq!(
            analysis.status(),
            Status::Converged,
            "{:?}",
            analysis.frontiers()
        );
        assert_eq!(
            oracle::compare(&world, &entry, analysis),
            OutcomeKind::Return
        );
        ssa::build_world(analysis)
            .unwrap()
            .verify(analysis)
            .unwrap();
    }
    assert_eq!(relations(&enabled), relations(&disabled));
    let inputs: Vec<_> = enabled
        .summaries()
        .iter()
        .filter(|record| record.input.frame.state.key.code_address == address(0x200))
        .map(|record| &record.input)
        .collect();
    assert!(
        inputs
            .iter()
            .any(|input| input.store.nonce(address(0x101)).contains(U256::ZERO))
    );
    assert!(
        inputs
            .iter()
            .any(|input| input.store.nonce(address(0x101)).contains(U256::from(1)))
    );
    assert!(
        inputs
            .iter()
            .any(|input| input.store.created_in_transaction(created) == Some(true))
    );
    assert!(
        inputs
            .iter()
            .any(|input| input.store.raw_account_code(created) == Some(vec![0x00]))
    );
}

#[test]
fn selfdestruct_balance_effects_invalidate_later_exact_inputs() {
    let caller = format!(
        "{}50{}50{}5000",
        call(0xf1, 0x200, 0, 0),
        call(0xf1, 0x201, 0, 0),
        call(0xf1, 0x200, 0, 0)
    );
    let (enabled, _) = compare(&[
        (0x101, &caller),
        (0x200, "5f545f5260205ff3"),
        (0x201, "610101ff"),
    ]);
    let inputs: Vec<_> = enabled
        .summaries()
        .iter()
        .filter(|record| record.input.frame.state.key.code_address == address(0x200))
        .map(|record| &record.input)
        .collect();
    assert!(
        inputs
            .iter()
            .any(|input| input.store.read_balance(address(0x201))
                == AbstractValue::constant(U256::from(1_000_000)))
    );
    assert!(inputs.iter().any(|input| {
        input
            .store
            .read_balance(address(0x201))
            .contains(U256::ZERO)
    }));
}

#[test]
fn source_joins_do_not_widen_an_already_published_exact_relation() {
    let caller = format!("5b{}5060095f555f56", call(0xf4, 0x200, 0, 0));
    let (world, entry) = fixture(&[(0x101, &caller), (0x200, "5f545f5260205ff3")]);
    let analysis = analyze_world(
        world,
        entry,
        ExecutionConfig {
            // 本测试需要循环后的调用汇合到同一源节点，再核对已发表证书保持不变。
            analysis: Config {
                context_depth: 0,
                ..Config::default()
            },
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    assert_eq!(
        analysis.status(),
        Status::Converged,
        "{:?}",
        analysis.frontiers()
    );
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();
    let slot = AbstractValue::constant(U256::ZERO);
    let original = analysis
        .summaries()
        .iter()
        .find(|record| {
            record.input.store.read(
                address(0x101),
                &slot,
                evm_abstract::domain::Domain::default(),
            ) == AbstractValue::constant(U256::from(7))
        })
        .expect("initial exact relation was never published");
    assert!(
        analysis.states()[original.source_state]
            .entry
            .store
            .read(
                address(0x101),
                &slot,
                evm_abstract::domain::Domain::default()
            )
            .contains(U256::from(9))
    );
    assert!(original.outputs.iter().all(|output| {
        output.store.read(
            address(0x101),
            &slot,
            evm_abstract::domain::Domain::default(),
        ) == AbstractValue::constant(U256::from(7))
    }));
    assert!(analysis.summaries().iter().any(|record| {
        record
            .input
            .store
            .read(
                address(0x101),
                &slot,
                evm_abstract::domain::Domain::default(),
            )
            .contains(U256::from(9))
    }));
}

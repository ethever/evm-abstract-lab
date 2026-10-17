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
    domain::{
        AbstractValue, Domain,
        relational::{CheckResult, RelationState},
    },
    ssa,
    world::{Account, ByteArray, Entry, World},
};
use oracle::address;
use std::collections::{BTreeMap, BTreeSet};

/// Compare observable numeric/provenance unions independently of immutable
/// execution-history graph layout. Snapshot roots and actual read semantics
/// are checked separately below; this is not complete native-graph equality.
fn observable(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(items) => items.iter_mut().for_each(observable),
        serde_json::Value::Object(fields) => {
            for key in ["expression", "root", "roots", "base", "operands"] {
                fields.remove(key);
            }
            fields.values_mut().for_each(observable);
        }
        _ => {}
    }
}

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
            let mut row =
                serde_json::to_value((outcome.kind, &outcome.data, &outcome.store)).unwrap();
            observable(&mut row);
            serde_json::to_string(&row).unwrap()
        })
        .collect()
}

// Only outcome membership is unordered. Replay may rename runtime leaves, but
// one injective map must preserve aliases across every row and conditional graph.
fn same_observable_relations(
    left: &BTreeSet<String>,
    right: &BTreeSet<String>,
    captured_leaves: &BTreeSet<u64>,
) -> bool {
    fn match_value(
        left: &serde_json::Value,
        right: &serde_json::Value,
        forward: &mut BTreeMap<u64, u64>,
        reverse: &mut BTreeMap<u64, u64>,
    ) -> bool {
        match (left, right) {
            (serde_json::Value::Object(left), serde_json::Value::Object(right)) => {
                if left.len() == 1 && left.contains_key("Fresh") {
                    if right.len() != 1 {
                        return false;
                    }
                    let (Some(left), Some(right)) = (
                        left["Fresh"].as_u64(),
                        right.get("Fresh").and_then(serde_json::Value::as_u64),
                    ) else {
                        return false;
                    };
                    return match (forward.get(&left), reverse.get(&right)) {
                        (Some(mapped), Some(original)) => *mapped == right && *original == left,
                        (None, None) => {
                            forward.insert(left, right);
                            reverse.insert(right, left);
                            true
                        }
                        _ => false,
                    };
                }
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right
                            .get(key)
                            .is_some_and(|other| match_value(value, other, forward, reverse))
                    })
            }
            (serde_json::Value::Array(left), serde_json::Value::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(value, other)| match_value(value, other, forward, reverse))
            }
            _ => left == right,
        }
    }
    fn match_rows(
        left: &[serde_json::Value],
        right: &[serde_json::Value],
        used: &mut [bool],
        forward: &BTreeMap<u64, u64>,
        reverse: &BTreeMap<u64, u64>,
    ) -> bool {
        let Some((row, remaining)) = left.split_first() else {
            return true;
        };
        for (index, candidate) in right.iter().enumerate() {
            if used[index] {
                continue;
            }
            // A rejected pairing must not leak its tentative leaf assignments.
            let mut next_forward = forward.clone();
            let mut next_reverse = reverse.clone();
            if match_value(row, candidate, &mut next_forward, &mut next_reverse) {
                used[index] = true;
                if match_rows(remaining, right, used, &next_forward, &next_reverse) {
                    return true;
                }
                used[index] = false;
            }
        }
        false
    }
    if left.len() != right.len() {
        return false;
    }
    let parse = |rows: &BTreeSet<String>| -> Vec<serde_json::Value> {
        rows.iter()
            .map(|row| serde_json::from_str(row).unwrap())
            .collect()
    };
    let left = parse(left);
    let right = parse(right);
    let anchors = captured_leaves.iter().map(|id| (*id, *id)).collect();
    match_rows(
        &left,
        &right,
        &mut vec![false; right.len()],
        &anchors,
        &anchors,
    )
}

fn checked_analyses(accounts: &[(u64, &str)]) -> (WorldAnalysis, WorldAnalysis) {
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
    assert!(disabled.summaries().is_empty());
    assert_eq!(disabled.summary_stats().hits, 0);
    (enabled, disabled)
}

fn compare(accounts: &[(u64, &str)]) -> (WorldAnalysis, WorldAnalysis) {
    let (enabled, disabled) = checked_analyses(accounts);
    assert_eq!(relations(&enabled), relations(&disabled));
    (enabled, disabled)
}

fn compare_full_store(accounts: &[(u64, &str)]) -> (WorldAnalysis, WorldAnalysis) {
    let (enabled, disabled) = checked_analyses(accounts);
    // The fixture clones one World and concrete Entry. Its saved input sources
    // stay fixed; only later private runtime leaves may be renamed.
    let captured = |analysis: &WorldAnalysis| -> BTreeSet<u64> {
        analysis.states()[0]
            .entry
            .active()
            .saved_store
            .state()
            .fresh_leaves()
            .into_iter()
            .map(|leaf| {
                let evm_abstract::domain::symbolic::ExprKind::Fresh(id) = leaf.kind() else {
                    panic!("fresh_leaves must contain only Fresh source leaves")
                };
                *id
            })
            .collect()
    };
    let captured_leaves = captured(&enabled);
    assert_eq!(captured_leaves, captured(&disabled));
    assert!(same_observable_relations(
        &relations(&enabled),
        &relations(&disabled),
        &captured_leaves,
    ));
    (enabled, disabled)
}

#[test]
fn observable_relation_bijection_preserves_conditional_aliases_and_inputs() {
    fn row(first: u64, second: u64, length: u64) -> serde_json::Value {
        let condition = |leaf| {
            serde_json::json!({
                "identity_scope": "producer",
                "nodes": [
                    {"Fresh": leaf},
                    {"Input": {"scope": 7, "symbol": "CallValue"}},
                    {"SlotsInput": {"source": {"token": 1, "namespace": "0x101"}, "bindings": []}},
                    {"Constant": "0x7"},
                    {"SlotRead": {"state": 2, "key": 3}},
                    {"Operation": {"opcode": 0x14, "args": [0, 4]}},
                    {"Fresh": 99},
                    {"SlotsInput": {"source": {"token": 6, "namespace": "0x999"}, "bindings": []}},
                    {"SlotRead": {"state": 7, "key": 3}},
                    {"Operation": {"opcode": 0x14, "args": [5, 8]}}
                ]
            })
        };
        let scope = |number| {
            serde_json::json!({
                "facts": [{"value": AbstractValue::constant(U256::from(number))}],
                "conditional": []
            })
        };
        let repeated = serde_json::json!({
            "condition": condition(second),
            "then_facts": scope(7_u64),
            "else_facts": scope(9_u64)
        });
        let mut then_facts = scope(7_u64);
        then_facts["conditional"] = serde_json::json!([repeated.clone()]);
        let mut else_facts = scope(9_u64);
        else_facts["conditional"] = serde_json::json!([repeated.clone()]);
        serde_json::json!(["Return", {
            "length": AbstractValue::constant(U256::from(length)),
            "conditional": [{
                "condition": condition(first), "then_facts": then_facts, "else_facts": else_facts
            }, repeated]
        }, {}])
    }
    let set = |rows: &[serde_json::Value]| -> BTreeSet<String> {
        rows.iter()
            .map(|row| {
                let mut row = row.clone();
                observable(&mut row);
                serde_json::to_string(&row).unwrap()
            })
            .collect()
    };
    let failure = serde_json::json!(["Failure", {"conditional": []}, {}]);
    let left = set(&[row(1, 2, 32), row(2, 1, 64), failure.clone()]);
    // Renaming reverses the raw set order of the Return rows.
    let right = [row(20, 10, 32), row(10, 20, 64), failure];
    let captured = BTreeSet::from([99]);
    let matches =
        |rows: &[serde_json::Value]| same_observable_relations(&left, &set(rows), &captured);
    assert!(matches(&right));
    assert!(same_observable_relations(&set(&right), &left, &captured));
    fn rename_source(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(fields) => {
                if fields.len() == 1
                    && fields.get("Fresh").and_then(serde_json::Value::as_u64) == Some(99)
                {
                    fields.insert("Fresh".to_owned(), serde_json::json!(199));
                } else {
                    fields.values_mut().for_each(rename_source);
                }
            }
            serde_json::Value::Array(values) => values.iter_mut().for_each(rename_source),
            _ => {}
        }
    }
    let mut changed_source = right.clone();
    changed_source.iter_mut().for_each(rename_source);
    assert!(same_observable_relations(
        &left,
        &set(&changed_source),
        &BTreeSet::new()
    ));
    assert!(
        !matches(&changed_source),
        "a captured source is not a private runtime leaf"
    );
    // Consistent many-to-one renaming must fail even when one leaf is unchanged.
    assert!(!matches(&[row(1, 1, 32), row(1, 1, 64), right[2].clone()]));

    for (index, pointer) in [
        (
            0,
            "/1/conditional/0/else_facts/conditional/0/condition/nodes/0/Fresh",
        ),
        (1, "/1/conditional/0/condition/nodes/0/Fresh"),
    ] {
        let mut split = right.clone();
        *split[index].pointer_mut(pointer).unwrap() = serde_json::json!(30);
        assert!(!matches(&split), "split alias at {index}:{pointer}");
    }
    for (pointer, replacement) in [
        ("/nodes/5/Operation/opcode", serde_json::json!(0x18)),
        ("/nodes/1/Input/scope", serde_json::json!(8)),
        ("/nodes/1/Input/symbol", serde_json::json!("Caller")),
        (
            "/nodes/2/SlotsInput/source/namespace",
            serde_json::json!("0x202"),
        ),
        ("/nodes/2/SlotsInput/source/token", serde_json::json!(0)),
        ("/identity_scope", serde_json::json!("other-producer")),
    ] {
        let mut changed = right.clone();
        *changed[0][1]["conditional"][0]["condition"]
            .pointer_mut(pointer)
            .unwrap() = replacement;
        assert!(!matches(&changed), "condition changed at {pointer}");
    }
    let mut numeric = right.clone();
    numeric[0][1]["length"] =
        serde_json::to_value(AbstractValue::constant(U256::from(33))).unwrap();
    assert!(!matches(&numeric));
    let mut polarity = right.clone();
    let branch = &mut polarity[0][1]["conditional"][0];
    let then_facts = branch["then_facts"].clone();
    branch["then_facts"] = branch["else_facts"].clone();
    branch["else_facts"] = then_facts;
    assert!(!matches(&polarity));
    let mut order = right.clone();
    order[0][1]["conditional"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(!matches(&order));
    let mut operands = right.clone();
    operands[0][1]["conditional"][0]["condition"]["nodes"][5]["Operation"]["args"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);
    assert!(!matches(&operands));
    let mut failure_kind = right.clone();
    failure_kind[2][0] = serde_json::json!("Return");
    assert!(!matches(&failure_kind));
    assert!(!matches(&right[..2]));
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
        let (enabled, _) = compare_full_store(&[(0x101, &caller), (0x200, "345f5260205ff3")]);
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
    let (enabled, _) = compare_full_store(&[(0x101, &caller), (0x200, "335f5260205ff3")]);
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
    let (enabled, disabled) = compare(&[(0x101, &caller), (0x200, &callee), (0x201, "60095f5500")]);
    for analysis in [&enabled, &disabled] {
        let initial = &analysis.states()[0].entry.store;
        let domain = Domain::from_spec(analysis.domain_spec());
        // This additional inspection query has its own bounded encoding work.
        // The analysis above still runs its unchanged default execution policy.
        let inspection = evm_abstract::domain::relational::RelationLimits {
            max_nodes: 8192,
            ..domain.spec().relations()
        };
        let zero = AbstractValue::constant(U256::ZERO);
        for outcome in analysis.outcomes() {
            // These compare the actual immutable roots, including source IDs
            // and initial binding graphs. Rollback cannot replace input arrays
            // by independent fresh sources even when their numeric facts match.
            for account in [0x101, 0x200, 0x201] {
                for transient in [false, true] {
                    assert_eq!(
                        outcome
                            .store
                            .slot_root(address(account), transient)
                            .unwrap(),
                        initial.slot_root(address(account), transient).unwrap()
                    );
                }
            }
            if outcome.kind == OutcomeKind::Return {
                let word = outcome.data.read_word_checked(&zero, domain).unwrap();
                let expression = word.symbolic_expression().unwrap();
                for candidate in word
                    .constants()
                    .expect("literal revert result or pre-entry failure")
                {
                    assert_eq!(
                        RelationState::default().can_equal(&expression, *candidate, &inspection),
                        CheckResult::MayBeFeasible
                    );
                }
                assert_eq!(
                    RelationState::default().can_equal(
                        &expression,
                        U256::from(0xffff),
                        &inspection
                    ),
                    CheckResult::ProvenImpossible
                );
            }
        }
    }
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
fn incomplete_child_chain_never_publishes_a_complete_relation() {
    // Exhaust the unfolding policy before reaching a repeated procedure. A
    // completed failure alternative cannot certify the unentered child branch.
    let first = format!("{}00", call(0xf1, 0x200, 0, 0));
    let second = format!("{}00", call(0xf1, 0x300, 0, 0));
    let third = format!("{}00", call(0xf1, 0x400, 0, 0));
    let (world, entry) = fixture(&[
        (0x101, &first),
        (0x200, &second),
        (0x300, &third),
        (0x400, "00"),
    ]);
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

// Reconstruct the full guard from an actual call entry, including its rollback
// checkpoint. Both publications and reuse sites must bind to this callee.
fn read_only_summary_input_at(
    analysis: &WorldAnalysis,
    state_id: usize,
) -> evm_abstract::analysis::SummaryInput {
    let source = &analysis.states()[state_id];
    assert_eq!(source.id, state_id);
    assert!(source.execution_evidence().unwrap().is_current());
    assert!(source.exit.is_some());
    assert!(
        analysis
            .edges()
            .iter()
            .any(|edge| edge.to == state_id && edge.kind == MachineEdgeKind::Call)
    );
    let payload = &source.entry;
    let (frame, _) = payload
        .call_stack
        .active_child()
        .unwrap()
        .clone()
        .into_root();
    assert_eq!(frame.state.key.code_address, address(0x200));
    assert_eq!(frame.state.key.basic_block_index, 0);
    assert_eq!(&frame.state.key, source.active());
    let evm_abstract::world::Code::Runtime(program) =
        &analysis.world().account(address(0x200)).unwrap().code
    else {
        panic!("the read-only callee must have runtime bytecode")
    };
    assert_eq!(frame.state.program.as_ref(), Some(program));
    let config = analysis.config();
    evm_abstract::analysis::SummaryInput {
        fork: analysis.world().fork(),
        snapshot: analysis.world().identity().clone(),
        world_fingerprint: analysis.world().fingerprint().unwrap(),
        code_hash: payload
            .store
            .raw_account_code(frame.state.key.code_address)
            .map(alloy_primitives::keccak256),
        frame,
        store: payload.store.clone(),
        environment: analysis.entry().environment.clone(),
        relations: payload.relations.clone(),
        remaining_call_depth: config
            .max_call_depth
            .saturating_sub(payload.call_stack.depth()),
        max_call_depth: config.max_call_depth,
        context_depth: config.analysis.context_depth,
        max_constants: config.analysis.max_constants,
        domain_spec: analysis.domain_spec(),
        max_memory_bytes: config.max_memory_bytes,
    }
}

#[test]
fn rollback_touch_preconditions_keep_the_original_two_call_sequence_as_misses() {
    let caller = format!(
        "{}50{}{}5000",
        call(0xf1, 0x200, 0, 32),
        "5b".repeat(12),
        call(0xf1, 0x200, 0, 32)
    );
    let (world, entry) = fixture(&[(0x101, &caller), (0x200, "6003565b5f545f5260205ff3")]);
    let analysis = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(analysis.status(), Status::Converged);
    assert!(analysis.frontiers().is_empty());
    assert_eq!(analysis.summary_stats().hits, 0);
    assert_eq!(analysis.summary_stats().misses, 2);
    assert_eq!(analysis.summary_stats().published, 2);
    assert_eq!(analysis.summary_stats().rejected_incomplete, 0);
    assert_eq!(analysis.summary_stats().imported_states, 0);
    assert_eq!(
        oracle::compare(&world, &entry, &analysis),
        OutcomeKind::Return
    );
    ssa::build_world(&analysis)
        .unwrap()
        .verify(&analysis)
        .unwrap();

    let mut records: Vec<_> = analysis.summaries().iter().collect();
    records.sort_unstable_by_key(|record| record.source_state);
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(
            record.input,
            read_only_summary_input_at(&analysis, record.source_state)
        );
        assert!(record.state_count > 1);
        assert!(record.edge_count > 0);
        assert!(
            record
                .outputs
                .iter()
                .any(|output| output.kind == OutcomeKind::Return)
        );
        assert!(record.reused_at.is_empty());
        assert_eq!(
            record.input.store.touched_in_transaction(address(0x200)),
            Some(true)
        );
        assert_eq!(
            record
                .input
                .frame
                .state
                .saved_store
                .state()
                .touched_default(),
            Some(false)
        );
    }
    let first = &records[0].input;
    let second = &records[1].input;
    assert_ne!(records[0].source_state, records[1].source_state);
    assert_ne!(first, second);
    assert_eq!(
        analysis.states()[0]
            .entry
            .active()
            .saved_store
            .state()
            .touched_in_transaction(address(0x200)),
        Some(false)
    );
    assert_eq!(
        first
            .frame
            .state
            .saved_store
            .state()
            .touched_in_transaction(address(0x200)),
        Some(false)
    );
    assert_eq!(
        second
            .frame
            .state
            .saved_store
            .state()
            .touched_in_transaction(address(0x200)),
        None
    );

    // A missing key inherits false; an explicit null retains may-touch. Check
    // membership separately because JSON indexing maps both cases to null.
    let first_json = serde_json::to_value(first).unwrap();
    let second_json = serde_json::to_value(second).unwrap();
    let touch_key = address(0x200).to_string();
    let first_touched = first_json
        .pointer("/frame/state/saved_store/touched")
        .unwrap()
        .as_object()
        .unwrap();
    let second_touched = second_json
        .pointer("/frame/state/saved_store/touched")
        .unwrap()
        .as_object()
        .unwrap();
    assert!(!first_touched.contains_key(&touch_key));
    assert!(second_touched.contains_key(&touch_key));
    assert_eq!(
        second_touched.get(&touch_key),
        Some(&serde_json::Value::Null)
    );

    // CALL checkpoints before touching the target. After rejection and success
    // join, only that saved touch changes; every other input field stays exact.
    let mut expected = (*first).clone();
    let mut saved_store = expected.frame.state.saved_store.state().clone();
    saved_store.may_touch_account(address(0x200));
    expected.frame.state.saved_store = saved_store.snapshot();
    assert_eq!(&expected, second);
}

#[test]
fn interrupted_summary_import_retains_an_explicit_shared_budget_frontier() {
    // Warm the rollback touch guard, then retain the same POP/JUMPDEST
    // separation between the two calls that can share a complete certificate.
    let caller = format!(
        "{}50{}{}50{}{}5000",
        call(0xf1, 0x200, 0, 32),
        "5b".repeat(12),
        call(0xf1, 0x200, 0, 32),
        "5b".repeat(12),
        call(0xf1, 0x200, 0, 32)
    );
    let (world, entry) = fixture(&[(0x101, &caller), (0x200, "6003565b5f545f5260205ff3")]);
    let complete = analyze_world(world.clone(), entry.clone(), ExecutionConfig::default()).unwrap();
    assert_eq!(complete.status(), Status::Converged);
    assert!(complete.frontiers().is_empty());
    assert_eq!(complete.summary_stats().misses, 2);
    assert_eq!(complete.summary_stats().published, 2);
    let reused = complete
        .summaries()
        .iter()
        .find(|record| !record.reused_at.is_empty())
        .unwrap();
    assert!(reused.state_count > 1);
    assert_eq!(
        reused.input,
        read_only_summary_input_at(&complete, reused.source_state)
    );
    assert_eq!(
        reused.input.store.touched_in_transaction(address(0x200)),
        Some(true)
    );
    assert_eq!(
        reused
            .input
            .frame
            .state
            .saved_store
            .state()
            .touched_in_transaction(address(0x200)),
        None
    );
    for &state_id in &reused.reused_at {
        assert_ne!(state_id, reused.source_state);
        assert_eq!(
            reused.input,
            read_only_summary_input_at(&complete, state_id)
        );
    }
    assert!(complete.summary_stats().hits > 0);
    let certified_states = complete
        .summaries()
        .iter()
        .map(|record| record.state_count)
        .max()
        .unwrap();
    assert!(certified_states > 1);
    let limit = complete.work();
    // Find an actual interruption inside graph import, without depending on
    // incidental work-unit constants from unrelated opcode implementation.
    // Import is atomic: the last budget before any imported state becomes visible
    // must preserve the complete source input without committing a partial graph.
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
            max_work: lo - 1,
            ..ExecutionConfig::default()
        },
    )
    .unwrap();
    let budget = lo - 1;
    assert!(analysis.summary_stats().hits > 0);
    assert_eq!(analysis.summary_stats().imported_states, 0);
    assert!(
        analysis
            .summaries()
            .iter()
            .all(|record| record.reused_at.is_empty())
    );
    let frontier = analysis
        .frontiers()
        .iter()
        .find(|frontier| {
            frontier.reason == FrontierReason::SummaryWork && frontier.payload.is_some()
        })
        .expect("atomic import retains its complete source input");
    let source = &analysis.states()[frontier.from.unwrap()];
    assert_eq!(frontier.payload.as_deref(), Some(&source.entry));
    assert!(source.exit.is_none());
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

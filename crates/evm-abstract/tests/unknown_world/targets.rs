use super::support::*;
use alloy_primitives::U256;
use evm_abstract::{
    analysis::{Config, ExecutionConfig, FrontierReason, MachineEdgeKind, Status},
    domain::{
        Domain,
        relational::{CheckResult, RelationLimits},
    },
    world::{Account, ByteArray},
};
use revm_bytecode::opcode;

fn wide_symbolic_policy() -> ExecutionConfig {
    // Full address-space partitioning also evaluates every native candidate.
    // These functional guards use one explicit ample policy; the default
    // resource-boundary test below keeps the same complete address space.
    ExecutionConfig {
        max_work: 1_000_000_000,
        analysis: Config {
            relations: RelationLimits {
                max_constraints: 2048,
                max_nodes: 65_536,
                max_depth: 512,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    }
}

fn complete_symbolic(analysis: &evm_abstract::analysis::WorldAnalysis) {
    complete(analysis);
    assert!(!analysis.unknown_summaries().is_empty());
    assert!(
        analysis
            .unknown_summaries()
            .iter()
            .all(|summary| summary.closed)
    );
}

fn symbolic_with_native_input_gaps(analysis: &evm_abstract::analysis::WorldAnalysis) {
    assert!(!analysis.unknown_summaries().is_empty());
    if analysis.frontiers().is_empty() {
        complete_symbolic(analysis);
    } else {
        // This full address space includes USER as an unknown-code address.
        // Its callback into A may enter the root branch and call a native
        // precompile with arbitrary data. That real native-input gap also
        // leaves the dependent U relation open, rather than proving closure.
        assert_eq!(analysis.status(), Status::Incomplete);
        assert!(
            analysis
                .frontiers()
                .iter()
                .all(|frontier| matches!(frontier.reason, FrontierReason::PrecompileInput(_))),
            "{:?}",
            frontier_causes(analysis)
        );
        let native_gaps: std::collections::BTreeSet<_> = analysis
            .frontiers()
            .iter()
            .map(|frontier| match frontier.reason {
                FrontierReason::PrecompileInput(owner) => owner,
                _ => unreachable!(),
            })
            .collect();
        let expected: std::collections::BTreeSet<_> = (1..=17).chain([256]).map(address).collect();
        assert_eq!(native_gaps, expected);
        let open: Vec<_> = analysis
            .unknown_summaries()
            .iter()
            .filter(|summary| !summary.closed)
            .collect();
        assert!(!open.is_empty());
        for summary in &open {
            assert!(
                analysis
                    .frontiers()
                    .iter()
                    .any(|frontier| frontier.from == Some(summary.source_state)
                        && matches!(frontier.reason, FrontierReason::PrecompileInput(_))),
                "open relation at {} has no corresponding native-input gap",
                summary.source_state
            );
        }
        eprintln!(
            "issue124 alias native boundary: open_sources={:?} native_targets={native_gaps:?}",
            open.iter()
                .map(|summary| summary.source_state)
                .collect::<Vec<_>>()
        );
    }
}

fn symbolic_caller(exclude_c: bool) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("self_callback", true);
    if exclude_c {
        a.input(0)
            .push_word(U256::MAX >> 96)
            .op(opcode::AND)
            .push(C as u16)
            .op(opcode::EQ)
            .jump("excluded", true);
    }
    a.call_arguments(opcode::CALL, 0, (0, 0), (0, 32))
        .input(0)
        .push_word(U256::from(1_000_000))
        .label("call")
        .op(opcode::CALL)
        .label("resumed")
        .op(opcode::ISZERO)
        .jump("call_failed", true)
        .input(0)
        .mstore(32)
        .return_range(0, 64);
    a.label("self_callback").push(17).return_top();
    a.label("call_failed").push(0).push(0).op(opcode::REVERT);
    if exclude_c {
        a.label("excluded").push(0).push(0).op(opcode::REVERT);
    }
    a.finish()
}

fn known_c() -> Fixture {
    let mut c = Assembly::default();
    c.push(23).return_top();
    c.finish()
}

#[test]
fn symbolic_target_covers_current_owner_other_known_and_remaining_addresses_with_guards() {
    let a = symbolic_caller(false);
    let c = known_c();
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    insert(&mut world, C, &c, &[]);
    let analysis = run_config(world, entry(ByteArray::unknown()), wide_symbolic_policy());
    complete_symbolic(&analysis);
    let call_pc = a.labels["call"] + 1;
    for candidate in [A, C] {
        let entries: Vec<_> = analysis
            .edges()
            .iter()
            .filter(|edge| edge.kind == MachineEdgeKind::Call)
            .filter(|edge| {
                let from = &analysis.states()[edge.from];
                let to = &analysis.states()[edge.to];
                from.key.frames.len() == 1
                    && from.executed_pcs.contains(&call_pc)
                    && to.active().code_address == address(candidate)
            })
            .map(|edge| &analysis.states()[edge.to])
            .collect();
        assert!(
            !entries.is_empty(),
            "symbolic target omitted known {candidate:#x}"
        );
        for child in entries {
            let target = child
                .entry
                .call_stack
                .root()
                .state
                .calldata
                .read_word(&word(0), Domain::default());
            let expression = target
                .expression()
                .expect("same symbolic target survives caller suspension");
            for other in [A, C, U].into_iter().filter(|other| *other != candidate) {
                assert_eq!(
                    child.entry.relations.can_equal(
                        expression,
                        U256::from(other),
                        &analysis.config().analysis.relations
                    ),
                    CheckResult::Unsat,
                    "known {candidate:#x} branch lacks the target equality guard excluding {other:#x}"
                );
            }
        }
    }
    assert!(reachable(&analysis, A, a.labels["resumed"]));
    let outputs = returns(&analysis);
    assert!(
        outputs
            .iter()
            .any(|o| output_word(o, 32).contains(U256::from(U))),
        "remaining target addresses were discarded"
    );
}

#[test]
fn existing_target_path_condition_excludes_known_c_candidate() {
    let a = symbolic_caller(true);
    let c = known_c();
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    insert(&mut world, C, &c, &[]);
    let analysis = run_config(world, entry(ByteArray::unknown()), wide_symbolic_policy());
    complete_symbolic(&analysis);
    let call_pc = a.labels["call"] + 1;
    assert!(
        !analysis
            .edges()
            .iter()
            .any(|edge| edge.kind == MachineEdgeKind::Call
                && analysis.states()[edge.from].key.frames.len() == 1
                && analysis.states()[edge.from].executed_pcs.contains(&call_pc)
                && analysis.states()[edge.to].active().code_address == address(C)),
        "target != C path entered known C anyway"
    );
    assert!(!returns(&analysis).is_empty());
}

#[test]
fn repeated_calls_keep_the_same_symbolic_target_identity_and_known_branch_guards() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true);
    for (label, output) in [("first_call", 0), ("second_call", 32)] {
        a.call_arguments(opcode::CALL, 0, (0, 0), (output, 32))
            .input(0)
            .push_word(U256::from(1_000_000))
            .label(label)
            .op(opcode::CALL)
            .op(opcode::ISZERO)
            .jump("failed", true);
    }
    a.return_range(0, 64);
    a.label("callback").push(17).return_top();
    a.label("failed").push(0).push(0).op(opcode::REVERT);
    let code = a.finish();
    let mut world = new_world();
    insert(&mut world, A, &code, &[]);
    insert(&mut world, C, &known_c(), &[]);
    let analysis = run_config(world, entry(ByteArray::unknown()), wide_symbolic_policy());
    complete_symbolic(&analysis);
    let original = analysis.states()[0]
        .entry
        .call_stack
        .root()
        .state
        .calldata
        .read_word(&word(0), Domain::default())
        .expression()
        .unwrap()
        .clone();
    for label in ["first_call", "second_call"] {
        let call_pc = code.labels[label] + 1;
        for candidate in [A, C] {
            let children: Vec<_> = analysis
                .edges()
                .iter()
                .filter(|edge| {
                    edge.kind == MachineEdgeKind::Call
                        && analysis.states()[edge.from].key.frames.len() == 1
                        && analysis.states()[edge.from].executed_pcs.contains(&call_pc)
                        && analysis.states()[edge.to].active().code_address == address(candidate)
                })
                .map(|edge| &analysis.states()[edge.to])
                .collect();
            assert!(
                !children.is_empty(),
                "{label} omitted known target {candidate:#x}"
            );
            for child in children {
                let target = child
                    .entry
                    .call_stack
                    .root()
                    .state
                    .calldata
                    .read_word(&word(0), Domain::default());
                assert_eq!(
                    target.expression(),
                    Some(&original),
                    "the repeated target was reconstructed as a fresh address"
                );
                for other in [A, C, U].into_iter().filter(|other| *other != candidate) {
                    assert_eq!(
                        child.entry.relations.can_equal(
                            target.expression().unwrap(),
                            U256::from(other),
                            &analysis.config().analysis.relations,
                        ),
                        CheckResult::Unsat,
                        "{label} lacks its target guard"
                    );
                }
            }
        }
    }
    complete(&analysis);
    assert!(!returns(&analysis).is_empty());
}

#[test]
fn different_symbolic_targets_can_alias_the_same_known_storage_owner() {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true)
        .push(1)
        .mstore(0)
        .call_arguments(opcode::CALL, 0, (0, 32), (32, 32))
        .input(0)
        .push_word(U256::from(1_000_000))
        .op(opcode::CALL)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .push(0)
        .mstore(0)
        .call_arguments(opcode::CALL, 0, (0, 32), (64, 32))
        .input(32)
        .push_word(U256::from(1_000_000))
        .op(opcode::CALL)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .return_range(32, 64);
    a.label("callback").op(opcode::STOP);
    a.label("failed").push(0).push(0).op(opcode::REVERT);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    insert(
        &mut world,
        C,
        &super::fixtures::external_cell(),
        &[(0, 5), (1, 1)],
    );
    let mut concrete_input = Vec::new();
    concrete_input.extend(U256::from(C).to_be_bytes::<32>());
    concrete_input.extend(U256::from(C).to_be_bytes::<32>());
    let observed = super::oracle::execute(&world, &entry(ByteArray::exact(&concrete_input)));
    assert_eq!(observed.output_word(0), U256::from(9));
    assert_eq!(observed.output_word(32), U256::from(9));
    let analysis = run_config(world, entry(ByteArray::unknown()), wide_symbolic_policy());
    symbolic_with_native_input_gaps(&analysis);
    let root = &analysis.states()[0].entry.call_stack.root().state;
    let first = root.calldata.read_word(&word(0), Domain::default());
    let second = root.calldata.read_word(&word(32), Domain::default());
    assert_ne!(first.expression(), second.expression());
    observed.covers(&analysis);
}

#[test]
fn full_symbolic_target_default_budget_retains_explicit_typed_frontiers() {
    let mut world = new_world();
    insert(&mut world, A, &symbolic_caller(false), &[]);
    insert(&mut world, C, &known_c(), &[]);
    let analysis = run(world, entry(ByteArray::unknown()));
    assert_eq!(analysis.config().max_work, 20_000_000);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis.frontiers().iter().any(|frontier| matches!(
            frontier.reason,
            FrontierReason::Work | FrontierReason::SummaryWork | FrontierReason::Relations(_)
        )),
        "{:?}",
        frontier_causes(&analysis)
    );
    assert!(analysis.work() <= analysis.config().max_work);
}

#[test]
fn evm_address_projection_keeps_high_word_bits_in_parent_and_calls_known_c() {
    let a = symbolic_caller(false);
    let c = known_c();
    let target: U256 = (U256::ONE << 200) | U256::from(C);
    let mut world = new_world();
    insert(&mut world, A, &a, &[]);
    insert(&mut world, C, &c, &[]);
    let analysis = run(world, entry(ByteArray::exact(&target.to_be_bytes::<32>())));
    complete(&analysis);
    for outcome in returns(&analysis) {
        assert_eq!(output_word(outcome, 0).singleton(), Some(U256::from(23)));
        assert_eq!(output_word(outcome, 32).singleton(), Some(target));
    }
}

#[test]
fn explicitly_empty_code_preserves_storage_and_returns_success_with_empty_data() {
    let mut a = Assembly::default();
    a.store(0, 42)
        .call(opcode::CALL, U as u16, 0, (0, 0), (0, 32))
        .op(opcode::DUP1)
        .op(opcode::ISZERO)
        .jump("failed", true)
        .mstore(0)
        .op(opcode::RETURNDATASIZE)
        .mstore(32)
        .return_range(0, 64);
    a.label("failed").push(0).push(0).op(opcode::REVERT);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    world.insert(address(U), Account::empty()).unwrap();
    let analysis = run(world, entry(ByteArray::empty()));
    complete(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[1]);
        only(&output_word(outcome, 32), &[0]);
        only(&read(&outcome.store, A, 0), &[42]);
    }
}

#[test]
fn no_external_execution_retains_original_concrete_result() {
    let mut a = Assembly::default();
    a.store(0, 7).push(0).op(opcode::SLOAD).return_top();
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    let analysis = run(world, entry(ByteArray::empty()));
    complete(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[7]);
        only(&read(&outcome.store, A, 0), &[7]);
    }
}

#[test]
fn native_identity_precompile_remains_a_concrete_call() {
    let mut a = Assembly::default();
    a.push(42)
        .mstore(0)
        .call(opcode::CALL, 4, 0, (0, 32), (32, 32))
        .op(opcode::ISZERO)
        .jump("failed", true)
        .return_range(32, 32);
    a.label("failed").push(0).push(0).op(opcode::REVERT);
    let mut world = new_world();
    insert(&mut world, A, &a.finish(), &[]);
    let analysis = run(world, entry(ByteArray::empty()));
    complete(&analysis);
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 0), &[42]);
    }
}

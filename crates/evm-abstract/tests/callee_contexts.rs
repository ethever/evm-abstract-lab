//! Relative bodies share callee work while concrete executions check control and
//! transaction effects. The analyzed root keeps Unknown caller/value/calldata.
#![allow(dead_code)]

#[path = "callee_contexts/admission.rs"]
mod admission;
#[path = "support/native.rs"]
mod native;
#[path = "unknown_world/oracle.rs"]
mod oracle;
#[path = "callee_contexts/semantics.rs"]
mod semantics;
#[path = "unknown_world/support.rs"]
mod support;

use std::{collections::BTreeSet, time::Instant};

use alloy_primitives::U256;
use evm_abstract::{
    analysis::{
        AnalysisEngine, AnalysisInput, CorrelatedJoin, EdgeKind, EnginePolicies, EvmWorldAnalysis,
        ExecutionConfig, FeedbackAcceleration, HybridConfig, MachineEdgeKind,
        ProcedureSummaryPolicy, RegionalSummaries, SemanticPolicies, Status, StructuralPartition,
        control::Control, cooperation::NoExtension,
    },
    domain::{EvmValue, NumericValue, ProductAlgebra},
    world::{AddressInput, EvmByteArray, EvmEntry, EvmWorld},
};
use revm_bytecode::opcode;
use support::{A, Assembly, U, address, insert, new_world};

fn unknown() -> EvmEntry<NumericValue> {
    EvmEntry {
        address: address(A),
        environment: evm_abstract::world::EvmEnvironmentValues {
            to: address(A).into(),
            caller: AddressInput::unknown_caller(),
            value: EvmValue::top(),
            calldata: EvmByteArray::unknown_checked().unwrap(),
            ..Default::default()
        },
    }
}

fn config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 100_000_000,
        root_terminal_coverage: false,
        analysis: evm_abstract::analysis::Config {
            context_depth: 2,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn run(world: EvmWorld<NumericValue>, early: bool) -> EvmWorldAnalysis<NumericValue> {
    let summary = if early {
        RegionalSummaries::with_early_known_admission()
    } else {
        RegionalSummaries::new()
    };
    run_policy(world, summary, if early { "early" } else { "late" })
}

fn run_policy(
    world: EvmWorld<NumericValue>,
    summary: impl ProcedureSummaryPolicy<ProductAlgebra>,
    label: &str,
) -> EvmWorldAnalysis<NumericValue> {
    run_config(world, summary, label, config())
}

fn run_config(
    world: EvmWorld<NumericValue>,
    summary: impl ProcedureSummaryPolicy<ProductAlgebra>,
    label: &str,
    config: ExecutionConfig,
) -> EvmWorldAnalysis<NumericValue> {
    let policies = EnginePolicies::default().with_state(SemanticPolicies::new(
        StructuralPartition,
        CorrelatedJoin,
        FeedbackAcceleration,
        summary,
    ));
    let mut extension = NoExtension;
    let entry = unknown();
    let started = Instant::now();
    let report = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run(
            AnalysisInput::new(world, entry.clone()).unwrap(),
            config,
            &Control::default(),
        )
        .unwrap();
    eprintln!(
        "callee_contexts policy={label} states={} transfers={} work={} elapsed_us={}",
        report.states().len(),
        report.transfers(),
        report.work(),
        started.elapsed().as_micros()
    );
    complete(&report);
    assert_eq!(report.entry(), &entry);
    report
}

fn complete(report: &EvmWorldAnalysis<NumericValue>) {
    assert_eq!(
        report.status(),
        Status::Converged,
        "{:?}",
        report.frontiers()
    );
    assert!(report.frontiers().is_empty());
    let mut sources = BTreeSet::new();
    for record in report.procedure_summaries().iter().rev() {
        // Earlier records describe the source before input growth. Closure
        // requires the latest record for every source, as in Hybrid coverage.
        if sources.insert(record.source_state) {
            assert!(record.closed && record.source_current);
        }
    }
}

fn two_callers() -> (EvmWorld<NumericValue>, support::Fixture) {
    let mut a = Assembly::default();
    a.input(0).jump("right", true);
    a.push_label("left_return").jump("shared_call", false);
    a.label("right")
        .push_label("right_return")
        .jump("shared_call", false);
    a.label("shared_call")
        .call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 0))
        .op(opcode::POP)
        .op(opcode::JUMP);
    a.label("left_return").push(11).return_top();
    a.label("right_return").push(22).return_top();
    let caller = a.finish();
    let mut b = Assembly::default();
    for _ in 0..24 {
        b.op(opcode::JUMPDEST)
            .push(17)
            .push(19)
            .op(opcode::ADD)
            .op(opcode::POP);
    }
    b.push(42).return_top();
    let mut world = new_world();
    insert(&mut world, A, &caller, &[(0, 5)]);
    insert(&mut world, U, &b.finish(), &[(0, 9)]);
    (world, caller)
}

fn assert_return_jumps(report: &EvmWorldAnalysis<NumericValue>, caller: &support::Fixture) {
    let expected = BTreeSet::from([caller.labels["left_return"], caller.labels["right_return"]]);
    let mut actual = BTreeSet::new();
    let mut histories = BTreeSet::new();
    for state in report.states().iter().filter(|state| {
        state.active().code_address == address(A)
            && state.executed_pcs.last().is_some_and(|pc| {
                caller.bytes[*pc] == opcode::JUMP && *pc > caller.labels["shared_call"]
            })
    }) {
        let targets = report
            .edges()
            .iter()
            .filter(|edge| {
                edge.from == state.id
                    && edge.kind == MachineEdgeKind::Intraprocedural(EdgeKind::Jump)
            })
            .map(|edge| {
                let to = &report.states()[edge.to];
                to.program().unwrap().blocks()[to.active().basic_block_index].instructions[0].pc
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(
            targets.len(),
            1,
            "caller return target was joined: {targets:?}"
        );
        histories.insert(state.active().jump_history.clone());
        actual.extend(targets);
    }
    assert_eq!(actual, expected);
    assert_eq!(histories.len(), 2);
}

#[test]
fn two_caller_histories_share_one_callee_body_and_keep_return_jump_correlation() {
    let (world, caller) = two_callers();
    let late = run(world.clone(), false);
    let early = run(world.clone(), true);
    let isolated = run_config(
        world.clone(),
        RegionalSummaries::with_early_known_admission(),
        "early-without-cross-caller-reuse",
        ExecutionConfig {
            use_summaries: false,
            ..config()
        },
    );
    for report in [&late, &early, &isolated] {
        assert_return_jumps(report, &caller);
        for selector in [0, 1] {
            let observed = oracle::execute(&world, &support::entry(support::calldata(selector)));
            assert_eq!(
                observed.output_word(0),
                U256::from(if selector == 0 { 11 } else { 22 })
            );
            observed.covers(report);
        }
    }
    let mut roots = BTreeSet::new();
    let mut caller_histories = BTreeSet::new();
    let mut entries = BTreeSet::new();
    for record in early.procedure_summaries() {
        let root = record.root_procedure.unwrap();
        assert_eq!(record.procedures[root].frame.code_address, address(U));
        roots.insert(root);
        let source = &early.states()[record.source_state];
        assert_eq!(source.entry.call_stack.depth(), 2);
        caller_histories.insert(source.key.frames[0].jump_history.clone());
        for node in &record.regional_cfg.nodes {
            assert_eq!(node.frame.code_address, address(U));
            assert_eq!(node.frame.address, address(U));
            assert!(node.frame.is_static);
            entries.insert(node.id);
        }
    }
    assert_eq!(roots.len(), 1);
    assert_eq!(caller_histories.len(), 2);
    assert!(!entries.is_empty());
    // Configured reuse gives all World callers the same table namespace.
    // Prove actual reuse with a zero-transfer lookup, then disable that reuse
    // on the same input and observe distinct body admissions and more work.
    assert!(early.config().use_summaries);
    assert!(
        early
            .procedure_summaries()
            .iter()
            .all(|record| record.procedures.len() == 1)
    );
    assert!(
        early
            .procedure_summaries()
            .iter()
            .any(|record| record.reused > 0 && record.transfers == 0)
    );
    assert_eq!(
        isolated
            .procedure_summaries()
            .last()
            .unwrap()
            .procedures
            .len(),
        2
    );
    assert!(early.transfers() < isolated.transfers());
    assert!(early.transfers() < late.transfers());
}

#[test]
fn hybrid_unselected_callee_executes_in_relative_body_with_selected_callers_preserved() {
    let (world, caller) = two_callers();
    let policies = EnginePolicies::default();
    let mut extension = NoExtension;
    let report = AnalysisEngine::new(&native::product(), &policies, &mut extension)
        .run_hybrid(
            AnalysisInput::new(world.clone(), unknown()).unwrap(),
            config(),
            HybridConfig {
                max_detail_work: 50_000_000,
                max_detail_regions: 1,
            },
            &Control::default(),
        )
        .unwrap();
    assert!(report.coverage().baseline_current && report.coverage().detail_current);
    let detail = report.detail().unwrap();
    assert_return_jumps(detail, &caller);
    let mut roots = BTreeSet::new();
    for record in detail.procedure_summaries() {
        let root = record.root_procedure.unwrap();
        roots.insert(root);
        assert_eq!(record.procedures[root].frame.code_address, address(U));
        assert!(
            record
                .regional_cfg
                .nodes
                .iter()
                .all(|node| node.frame.jump_history.is_empty())
        );
    }
    assert_eq!(roots.len(), 1);
    assert!(detail.config().use_summaries);
    assert!(
        detail
            .procedure_summaries()
            .iter()
            .all(|record| record.procedures.len() == 1)
    );
    assert!(
        detail
            .procedure_summaries()
            .iter()
            .any(|record| record.reused > 0 && record.transfers == 0)
    );
    for selector in [0, 1] {
        oracle::execute(&world, &support::entry(support::calldata(selector))).covers(detail);
    }
}

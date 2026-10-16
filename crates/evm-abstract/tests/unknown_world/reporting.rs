use super::{contexts, support::*};
use evm_abstract::{
    analysis::{
        ExecutionConfig, FrameCode, FrontierReason, MachineEdgeKind, Status, control::Control,
        progress::Observer,
    },
    domain::relational::QueryReason,
    world::ByteArray,
};
use revm_bytecode::opcode;

#[test]
fn resource_budgets_leave_real_frontiers_and_do_not_claim_convergence() {
    for policy in ["work", "states", "transfers", "depth"] {
        let a = contexts::readonly_parent(opcode::STATICCALL);
        let mut world = new_world();
        insert(&mut world, A, &a, &[(0, 42)]);
        let mut config = ExecutionConfig::default();
        match policy {
            "work" => config.max_work = 8,
            "states" => config.analysis.max_states = 1,
            "transfers" => config.analysis.max_transfers = 1,
            "depth" => config.max_call_depth = 1,
            _ => unreachable!(),
        }
        let analysis = run_config(world, entry(ByteArray::empty()), config);
        assert_eq!(
            analysis.status(),
            Status::Incomplete,
            "{policy} budget was silently treated as closure"
        );
        assert!(!analysis.frontiers().is_empty());
        assert!(
            analysis.frontiers().iter().any(|f| match policy {
                "work" => matches!(f.reason, FrontierReason::Work | FrontierReason::SummaryWork),
                "states" | "transfers" => matches!(f.reason, FrontierReason::Budget(_)),
                "depth" => f.reason == FrontierReason::CallDepth,
                _ => false,
            }),
            "{policy}: {:?}",
            analysis.frontiers()
        );
        assert!(analysis.work() <= analysis.config().max_work);
        assert!(analysis.states().len() <= analysis.config().analysis.max_states);
        assert!(analysis.transfers() <= analysis.config().analysis.max_transfers);
    }
}

#[test]
fn closed_static_unknown_summary_has_evidence_and_feeds_later_storage_reads() {
    let a = contexts::readonly_parent(opcode::STATICCALL);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 42)]);
    let analysis = run(world, entry(ByteArray::empty()));
    complete(&analysis);
    assert!(
        !analysis.procedure_summaries().is_empty(),
        "closed unknown relation has no public coverage record"
    );
    for summary in analysis.procedure_summaries() {
        assert!(
            summary.closed,
            "Converged included a partial unknown relation"
        );
        assert!(!summary.procedures.is_empty());
        assert!(
            summary
                .procedures
                .iter()
                .any(|p| p.frame.mode == FrameCode::Unknown && !p.outputs.is_empty())
        );
        for procedure in &summary.procedures {
            assert!(
                procedure
                    .dependencies
                    .iter()
                    .all(|index| *index < summary.procedures.len()),
                "coverage record references a missing procedure"
            );
        }
    }
    assert!(
        analysis
            .states()
            .iter()
            .any(|s| s.active().mode == FrameCode::Unknown),
        "unknown execution is not visible in the public graph"
    );
    assert!(
        analysis
            .edges()
            .iter()
            .any(|e| matches!(e.kind, MachineEdgeKind::Summary(_))),
        "summary application has no public execution evidence"
    );
    for outcome in returns(&analysis) {
        only(&output_word(outcome, 32), &[42]);
        only(&read(&outcome.store, A, 0), &[42]);
    }
    let ir = evm_abstract::ssa::build_world(&analysis).unwrap();
    ir.verify(&analysis).unwrap();
    assert!(
        ir.transitions()
            .iter()
            .any(|t| matches!(t.kind, MachineEdgeKind::Summary(_)))
    );
}

#[test]
fn missing_known_callback_effect_coverage_remains_incomplete() {
    let a = contexts::readonly_parent(opcode::CALL);
    let mut c = Assembly::default();
    c.op(opcode::CALLDATASIZE)
        .push(0)
        .push(0)
        .op(opcode::CREATE)
        .op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 42)]);
    insert(&mut world, C, &c.finish(), &[]);
    let analysis = run(world, entry(ByteArray::empty()));
    assert_eq!(
        analysis.status(),
        Status::Incomplete,
        "unknown C initcode had no established effect coverage"
    );
    assert!(
        analysis.frontiers().iter().any(|f| matches!(
            f.reason,
            FrontierReason::Creation(_)
                | FrontierReason::Memory
                | FrontierReason::ByteRange(_)
                | FrontierReason::Relations(_)
        )),
        "missing callback effect coverage was lost: {:?}",
        analysis.frontiers()
    );
    // A reliable zero-interaction U return remains usable in the partial graph.
    assert!(
        returns(&analysis)
            .iter()
            .any(|o| output_word(o, 32).contains(evm_abstract::U256::from(42)))
    );
}

#[test]
fn local_cancellation_is_reported_without_poisoning_possible_unknown_interactions() {
    let a = contexts::readonly_parent(opcode::CALL);
    let mut c = Assembly::default();
    c.input(0)
        .push(7)
        .op(opcode::EQ)
        .jump("write", true)
        .op(opcode::STOP);
    c.label("write").store(0, 9).op(opcode::STOP);
    let mut world = new_world();
    insert(&mut world, A, &a, &[(0, 42)]);
    insert(&mut world, C, &c.finish(), &[(0, 5)]);
    // The removed solver-fuel fixture now exercises real scoped cancellation.
    // The fresh retry uses the same world, input and complete default policy.
    let input = entry(ByteArray::empty());
    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    let cancelled =
        control.scope(|| run_config(world.clone(), input.clone(), ExecutionConfig::default()));
    assert_eq!(cancelled.status(), Status::Incomplete);
    assert!(
        cancelled
            .frontiers()
            .iter()
            .any(|frontier| frontier.reason == FrontierReason::Relations(QueryReason::Cancelled)),
        "local cancellation cause was not retained"
    );
    assert!(
        cancelled
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind != evm_abstract::analysis::OutcomeKind::Return)
    );
    let analysis = run_config(world, input, ExecutionConfig::default());
    let outcomes = returns(&analysis);
    for expected in [5, 9] {
        assert!(
            outcomes
                .iter()
                .any(|o| read(&o.store, C, 0).contains(evm_abstract::U256::from(expected))),
            "fresh local analysis omitted C={expected} after leaving the cancelled scope"
        );
    }
    // Ordinary local imprecision may coexist with convergence only when the
    // complete conservative relation was closed for all retained interactions.
    if analysis.status() == Status::Converged {
        assert!(analysis.frontiers().is_empty());
        assert!(!analysis.procedure_summaries().is_empty());
        assert!(
            analysis
                .procedure_summaries()
                .iter()
                .all(|summary| summary.closed)
        );
        assert!(
            analysis
                .edges()
                .iter()
                .any(|e| matches!(e.kind, MachineEdgeKind::Summary(_)))
        );
    } else {
        assert!(!analysis.frontiers().is_empty());
    }
}

#[test]
fn repeated_static_unknown_calls_scale_under_shared_counters_without_time_thresholds() {
    check_repeated_static_calls(true);
}

#[test]
fn disabling_summary_cache_preserves_the_same_repeated_static_unknown_coverage() {
    check_repeated_static_calls(false);
}

fn repeated_static_fixture(calls: usize) -> Fixture {
    let mut a = Assembly::default();
    a.op(opcode::CALLER)
        .push(USER as u16)
        .op(opcode::EQ)
        .op(opcode::ISZERO)
        .jump("callback", true);
    for _ in 0..calls {
        a.call(opcode::STATICCALL, U as u16, 0, (0, 0), (0, 0))
            .op(opcode::POP);
    }
    a.push(0).op(opcode::SLOAD).return_top();
    a.label("callback").op(opcode::STOP);
    a.finish()
}

#[test]
fn default_work_budget_retains_an_explicit_frontier_for_large_static_workload() {
    let mut world = new_world();
    insert(&mut world, A, &repeated_static_fixture(256), &[(0, 42)]);
    let analysis = run(world, entry(ByteArray::empty()));
    assert_eq!(analysis.config().max_work, 20_000_000);
    assert_eq!(analysis.status(), Status::Incomplete);
    assert!(
        analysis.frontiers().iter().any(|frontier| matches!(
            frontier.reason,
            FrontierReason::Work | FrontierReason::SummaryWork
        )),
        "{:?}",
        frontier_causes(&analysis)
    );
    assert!(analysis.work() <= analysis.config().max_work);
    // Exhaustion can precede procedure admission. The frontier is required
    // whether or not a partial procedure record has been constructed yet.
    assert!(
        analysis
            .frontiers()
            .iter()
            .all(|frontier| { frontier.from.is_some() || frontier.target.is_some() })
    );
}

fn check_repeated_static_calls(use_summaries: bool) {
    for calls in [1, 4, 16] {
        let mut world = new_world();
        insert(&mut world, A, &repeated_static_fixture(calls), &[(0, 42)]);
        // Performance comparisons use the same sufficient ledger on both
        // sides. Functional busy/x, nested, C and rollback tests keep defaults.
        let config = ExecutionConfig {
            use_summaries,
            max_work: 1_000_000_000,
            ..Default::default()
        };
        let started = std::time::Instant::now();
        let analysis = run_config(world, entry(ByteArray::empty()), config);
        complete(&analysis);
        assert!(!analysis.procedure_summaries().is_empty());
        assert!(
            analysis
                .procedure_summaries()
                .iter()
                .all(|summary| summary.closed)
        );
        let outcomes = returns(&analysis);
        assert!(!outcomes.is_empty());
        for outcome in outcomes {
            only(&output_word(outcome, 0), &[42]);
        }
        assert!(analysis.work() <= analysis.config().max_work);
        assert!(analysis.states().len() <= analysis.config().analysis.max_states);
        assert!(analysis.transfers() <= analysis.config().analysis.max_transfers);
        eprintln!(
            "issue124 scale: calls={calls} use_summaries={use_summaries} states={} transfers={} work={} known_summary_hits={} elapsed_us={}",
            analysis.states().len(),
            analysis.transfers(),
            analysis.work(),
            analysis.summary_stats().hits,
            started.elapsed().as_micros()
        );
        let procedure_count: usize = analysis
            .procedure_summaries()
            .iter()
            .map(|s| s.procedures.len())
            .sum();
        let reused: usize = analysis
            .procedure_summaries()
            .iter()
            .map(|s| s.reused)
            .sum();
        eprintln!(
            "issue124 scale relation evidence: calls={calls} use_summaries={use_summaries} requests={} procedures={procedure_count} reused={reused}",
            analysis.procedure_summaries().len()
        );
    }
}

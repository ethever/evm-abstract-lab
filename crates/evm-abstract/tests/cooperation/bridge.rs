//! Public EVM -> external analyzer -> EVM -> analyzer production feedback.
#[path = "bridge/extra.rs"]
mod extra;
#[path = "bridge/fixture.rs"]
pub(super) mod fixture;
#[path = "bridge/rule.rs"]
pub(super) mod rule;

use super::{
    fixtures,
    policy::{Calls, Policy},
};
use alloy_primitives::U256;
use analysis_kernel::{Change, Registered};
use evm_abstract::{
    analysis::{
        EnginePolicies, ExecutionConfig, FrontierReason, OutcomeKind, Status,
        control::Control,
        cooperation::{
            CooperationError, Extension, Point, PointKind, analyze_world_cooperating,
            analyze_world_cooperating_with_control,
        },
        progress::Observer,
    },
    domain::ValueAlgebra,
};
use std::collections::{BTreeMap, BTreeSet};

fn config() -> ExecutionConfig {
    ExecutionConfig {
        max_work: 5_000_000_000,
        root_terminal_coverage: false,
        ..ExecutionConfig::default()
    }
}

#[test]
fn an_external_registered_rule_changes_real_cfg_and_receives_later_evm_inputs() {
    let fixture = fixture::guarded(false);
    let calls = Calls::default();
    let mut policy = Policy::new(&calls);
    policy.exclusion_queries = false; // Require program-level external reasoning.
    let algebra = ValueAlgebra::new(policy);
    let mut external = Extension::new(Registered::new(rule::Rule {
        enabled: true,
        ..rule::Rule::default()
    }));
    let result = analyze_world_cooperating(
        fixtures::world(&fixture.code),
        fixtures::entry(),
        config(),
        &algebra,
        &EnginePolicies::default(),
        &mut external,
    )
    .unwrap();
    assert_eq!(
        result.status(),
        Status::Converged,
        "{:?}",
        result.frontiers()
    );
    assert!(
        !result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.impossible))
    );
    let trace = external.analyzer.rule();
    let strengthened: BTreeSet<_> = trace
        .feedback
        .iter()
        .filter_map(|(subject, change)| (*change == Change::Changed).then_some(subject.point))
        .collect();
    assert!(
        strengthened.len() >= 2,
        "EVM must advance to another input after external feedback"
    );
    assert!(
        trace
            .feedback
            .iter()
            .any(|(_, change)| *change == Change::Unchanged)
    );
    assert!(
        trace.subjects.len() < 10_000,
        "equal facts must quiesce rather than endlessly rerun"
    );
    assert!(result.outcomes().iter().any(|outcome| {
        outcome.kind == OutcomeKind::Return
            && outcome
                .data
                .read_word_with(&fixtures::word(0), algebra.borrowed())
                .singleton()
                == Some(U256::ONE)
    }));

    let mut removed = Extension::new(Registered::new(rule::Rule::default()));
    let control = analyze_world_cooperating(
        fixtures::world(&fixture.code),
        fixtures::entry(),
        config(),
        &algebra,
        &EnginePolicies::default(),
        &mut removed,
    )
    .unwrap();
    assert_eq!(
        control.status(),
        Status::Converged,
        "{:?}",
        control.frontiers()
    );
    assert!(
        control
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.impossible)),
        "feedback removal must lose actual CFG precision"
    );
}

#[test]
fn recursive_summary_growth_reseeds_external_subjects_and_preserves_the_joint_fixed_point() {
    let fixture = fixture::guarded(true);
    let calls = Calls::default();
    let mut policy = Policy::new(&calls);
    policy.exclusion_queries = false;
    let algebra = ValueAlgebra::new(policy);
    let mut external = Extension::new(Registered::new(rule::Rule {
        enabled: true,
        ..rule::Rule::default()
    }));
    let result = analyze_world_cooperating(
        fixtures::world(&fixture.code),
        fixtures::entry(),
        config(),
        &algebra,
        &EnginePolicies::default(),
        &mut external,
    )
    .unwrap();
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
            .all(|summary| summary.closed)
    );
    assert!(
        result
            .procedure_summaries()
            .iter()
            .flat_map(|summary| &summary.procedures)
            .any(|procedure| procedure.output_updates > 1),
        "late output growth must actually occur"
    );
    let trace = external.analyzer.rule();
    assert!(
        trace
            .subjects
            .iter()
            .any(|subject| subject.point.kind == PointKind::Body)
    );
    let mut generations: BTreeMap<Point, BTreeSet<_>> = BTreeMap::new();
    for subject in &trace.subjects {
        generations
            .entry(subject.point)
            .or_default()
            .insert(subject.scope);
    }
    assert!(
        generations
            .iter()
            .any(|(point, scopes)| point.kind == PointKind::Body && scopes.len() > 1),
        "summary/input growth must create fresh external universes, not reuse old narrowing"
    );
    assert!(
        !result
            .states()
            .iter()
            .any(|state| state.executed_pcs.contains(&fixture.impossible))
    );
    assert!(trace.feedback.iter().any(
        |(subject, change)| subject.point.kind == PointKind::Body && *change == Change::Changed
    ));
}

#[test]
fn external_work_refusal_retains_incomplete_pending_effects_and_differs_from_cancellation() {
    let fixture = fixture::guarded(false);
    let calls = Calls::default();
    let mut policy = Policy::new(&calls);
    policy.exclusion_queries = false;
    let algebra = ValueAlgebra::new(policy);
    let mut external = Extension::new(Registered::new(rule::Rule {
        enabled: true,
        exhaust_after_feedback: true,
        ..rule::Rule::default()
    }));
    let result = analyze_world_cooperating(
        fixtures::world(&fixture.code),
        fixtures::entry(),
        ExecutionConfig {
            root_terminal_coverage: true,
            ..config()
        },
        &algebra,
        &EnginePolicies::default(),
        &mut external,
    )
    .expect("ordinary shared Work retains the partial native report");
    assert_eq!(result.status(), Status::Incomplete);
    assert!(result.work() <= config().max_work);
    assert!(!result.frontiers().is_empty());
    assert!(
        result
            .frontiers()
            .iter()
            .all(|f| f.reason == FrontierReason::Work)
    );
    assert!(
        result
            .frontiers()
            .iter()
            .any(|f| f.payload.is_some() || f.target.is_some() || f.from.is_some()),
        "the unexpanded EVM continuation must remain attributable"
    );
    assert!(
        result
            .outcomes()
            .iter()
            .all(|outcome| outcome.kind == OutcomeKind::Failure),
        "earlier EVM gas failures are valid; unfinished feedback cannot prove a later Return/Revert"
    );
    for failure in result.outcomes() {
        assert_eq!(failure.data.len().singleton(), Some(U256::ZERO));
        assert!(failure.store.possible_logs().is_empty());
        assert!(
            !failure
                .store
                .read_with(
                    fixtures::address(fixtures::OWNER),
                    &fixtures::word(0),
                    algebra.borrowed()
                )
                .contains(U256::from(42))
        );
    }
    // v5 ad-hoc cooperating entry has no AnalysisSession snapshot/revision.
    // terminal::prepare refuses MissingIdentity before preparing coverage even
    // when enabled. Keep that absence explicit; never invent AllInputStates
    // evidence from the partial graph. Session coverage has separate regressions.
    assert!(result.root_terminal_coverage().is_none());
    assert!(
        external
            .analyzer
            .rule()
            .feedback
            .iter()
            .any(|(_, change)| *change == Change::Changed),
        "interruption must occur after useful feedback, not just fail initialization"
    );

    let control = Control::new(Observer::default());
    control.cancellation().cancel();
    let mut cancelled = Extension::new(Registered::new(rule::Rule::default()));
    assert!(matches!(
        analyze_world_cooperating_with_control(
            fixtures::world(&fixture.code),
            fixtures::entry(),
            config(),
            &algebra,
            &EnginePolicies::default(),
            &mut cancelled,
            &control,
        ),
        Err(CooperationError::Cancelled)
    ));
    assert!(cancelled.analyzer.rule().subjects.is_empty());
}

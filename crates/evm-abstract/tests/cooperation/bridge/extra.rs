//! A downstream-owned query vocabulary; only the final adapter knows StackFacts.
use evm_abstract::analysis::control::Control;
use evm_abstract::analysis::{AnalysisEngine, AnalysisInput};
#[path = "extra/consumer.rs"]
mod consumer;
#[path = "extra/contracts.rs"]
mod contracts;
#[path = "extra/provider.rs"]
mod provider;

use super::{config, fixture};
use crate::{fixtures, oracle, policy::Calls};
use analysis_kernel::{Change, Pair, Registered};
use evm_abstract::analysis::{
    EnginePolicies, OutcomeKind,
    cooperation::{Extension, PairedTasks, PerInput, PointKind},
};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn client_owned_capability_connects_independent_rules_to_real_evm_feedback() {
    let fixture = fixture::guarded(false);
    let calls = Calls::default();
    let algebra = fixtures::feedback_algebra(&calls);
    for removed in [None, Some("provider"), Some("consumer")] {
        // Consumer starts before the provider: only its automatic Pending-read
        // dependency can bring it back after the client-owned answer arrives.
        let mut extension = Extension::with_tasks(
            Pair::new(
                Registered::new(consumer::Consumer::new(removed != Some("consumer"))),
                Registered::new(provider::Provider::new(removed != Some("provider"))),
            ),
            PairedTasks::new(PerInput, PerInput),
        )
        .with_stores(contracts::Stores::default());
        let result = AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension)
            .run(
                AnalysisInput::new(fixtures::world(&fixture.code), fixtures::entry()).unwrap(),
                config(),
                &Control::default(),
            )
            .unwrap();
        fixtures::complete(&result);
        let impossible = result
            .states()
            .iter()
            .any(|s| s.executed_pcs.contains(&fixture.impossible));
        assert_eq!(
            impossible,
            removed.is_some(),
            "each independent component is necessary: {removed:?}"
        );
        // One joint data/storage/balance/log tuple covers an actual revm run,
        // including under both precision-removal controls.
        oracle::assert_guard_coverage(&fixture.code, &result, &algebra);
        assert!(
            result
                .outcomes()
                .iter()
                .any(|o| o.kind == OutcomeKind::Return)
        );
        let consumer = extension.analyzer.left.rule();
        let provider = extension.analyzer.right.rule();
        assert!(
            consumer.pending > 0,
            "late client-owned publication must really be exercised"
        );
        assert!(
            consumer
                .subjects
                .iter()
                .all(|s| Some(s.scope.epoch()) == extension.stores.epoch.get())
        );
        assert!(
            extension.stores.created.get()
                >= provider.subjects.iter().collect::<BTreeSet<_>>().len()
        );
        if removed.is_none() {
            let changed: BTreeSet<_> = consumer
                .feedback
                .iter()
                .filter_map(|(s, c)| (*c == Change::Changed).then_some(s.point))
                .collect();
            assert!(
                changed.len() >= 2,
                "feedback must reach another production input, then return through both analyzers"
            );
            assert!(
                provider.feedback_observed > 0,
                "EVM feedback must wake its automatically subscribed independent provider"
            );
            assert!(
                consumer
                    .feedback
                    .iter()
                    .any(|(_, c)| *c == Change::Unchanged)
            );
            assert!(
                provider
                    .publications
                    .iter()
                    .any(|(_, c)| *c == Change::Unchanged)
            );
            assert!(
                consumer.subjects.len() + provider.subjects.len() < 10_000,
                "equal publications must terminate"
            );
        }
    }
}

#[test]
fn client_owned_channels_are_recreated_when_recursive_summary_inputs_grow() {
    let fixture = fixture::guarded(true);
    let calls = Calls::default();
    let algebra = fixtures::feedback_algebra(&calls);
    let mut extension = Extension::with_tasks(
        Pair::new(
            Registered::new(consumer::Consumer::new(true)),
            Registered::new(provider::Provider::new(true)),
        ),
        PairedTasks::new(PerInput, PerInput),
    )
    .with_stores(contracts::Stores::default());
    let result = AnalysisEngine::new(&algebra, &EnginePolicies::default(), &mut extension)
        .run(
            AnalysisInput::new(fixtures::world(&fixture.code), fixtures::entry()).unwrap(),
            config(),
            &Control::default(),
        )
        .unwrap();
    fixtures::complete(&result);
    assert!(!result.procedure_summaries().is_empty());
    assert!(result.procedure_summaries().iter().all(|s| s.closed));
    assert!(
        result
            .procedure_summaries()
            .iter()
            .flat_map(|s| &s.procedures)
            .any(|p| p.output_updates > 1)
    );
    assert!(
        !result
            .states()
            .iter()
            .any(|s| s.executed_pcs.contains(&fixture.impossible))
    );
    let consumer = extension.analyzer.left.rule();
    let provider = extension.analyzer.right.rule();
    let mut generations = BTreeMap::<_, BTreeSet<_>>::new();
    for s in &provider.subjects {
        generations.entry(s.point).or_default().insert(s.scope);
        assert_eq!(Some(s.scope.epoch()), extension.stores.epoch.get());
    }
    assert!(
        generations
            .iter()
            .any(|(p, g)| p.kind == PointKind::Body && g.len() > 1)
    );
    assert!(
        consumer
            .feedback
            .iter()
            .any(|(s, c)| s.point.kind == PointKind::Body && *c == Change::Changed)
    );
    // Fresh per-Subject stores must again be unresolved when the consumer is
    // first registered; a copied old answer would bypass these Pending reads.
    assert!(consumer.pending >= generations.values().map(BTreeSet::len).sum::<usize>());
    assert!(
        consumer
            .feedback
            .iter()
            .any(|(_, c)| *c == Change::Unchanged)
    );
    assert!(provider.feedback_observed > 0);
    assert!(consumer.subjects.len() + provider.subjects.len() < 10_000);
}

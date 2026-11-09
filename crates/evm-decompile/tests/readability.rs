//! Reading choices are tested against source execution, not their cost formulas.
#[path = "lifted/oracle/expressions.rs"]
mod expressions;
#[path = "readability/fixtures.rs"]
mod fixtures;
#[path = "shared_procedures/oracle.rs"]
mod oracle;
#[path = "shared_procedures/support.rs"]
mod support;

use evm_decompile::{
    Artifact, ArtifactId, Completion, Config, DiagnosticCode, EntityId, ExecutionLimits,
    ProcedurePreference, ReadabilityProfile, ReadabilityReason, RegionAnchor, RegionPreference,
    Severity, StageKind, Strategy, StructureForm, StructurePreference, StructuredKind,
    ValidationStatus, decompile, readability_source,
};

const PROFILES: [ReadabilityProfile; 3] = [
    ReadabilityProfile::Balanced,
    ReadabilityProfile::Linear,
    ReadabilityProfile::Compact,
];

fn checked(input: &evm_decompile::Input, config: &Config) -> Artifact {
    let artifact = decompile(input, config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    assert!(
        artifact
            .stages
            .iter()
            .all(|s| s.validation.status == ValidationStatus::Passed)
    );
    artifact
}

fn root_anchor(input: &evm_decompile::Input, config: &Config, artifact: &Artifact) -> RegionAnchor {
    RegionAnchor {
        source: readability_source(input, config),
        entry: artifact.current().unwrap().ir.entry.unwrap(),
        continuation: None,
        loop_header: None,
    }
}

fn choose(config: &mut Config, region: RegionAnchor, structure: StructurePreference) {
    config.readability.regions = vec![RegionPreference { region, structure }];
}

#[test]
fn reading_profiles_preserve_failed_guards_loop_updates_and_normal_continuations() {
    for profile in PROFILES {
        let mut config = Config::default();
        config.readability.profile = profile;
        config.readability.procedures = ProcedurePreference::Inline;
        let input = fixtures::conditional_tail();
        let artifact = checked(&input, &config);
        for first in [0, 1] {
            for second in [0, 1] {
                let host = fixtures::host(&input, &[first, second]);
                let report = oracle::certify(&input, &artifact, host, &ExecutionLimits::default());
                assert!(report.calls.is_empty());
                assert_eq!(
                    report
                        .host
                        .state
                        .storage
                        .get(&evm_decompile::U256::ZERO)
                        .copied(),
                    if first != 0 && second != 0 {
                        None
                    } else {
                        Some(evm_decompile::U256::from(if first == 0 { 2 } else { 1 }))
                    }
                );
            }
        }
        let input = fixtures::countdown();
        let artifact = checked(&input, &config);
        for count in [0, 1, 3] {
            let report = oracle::certify(
                &input,
                &artifact,
                fixtures::host(&input, &[count]),
                &ExecutionLimits::default(),
            );
            assert!(report.calls.is_empty());
            assert_eq!(
                report.host.state.storage[&evm_decompile::U256::ZERO],
                evm_decompile::U256::ZERO
            );
        }
    }
}

#[test]
fn explicit_root_forms_change_the_retained_representation_without_changing_effects() {
    let input = fixtures::conditional_tail();
    let baseline = checked(&input, &Config::default());
    let anchor = root_anchor(&input, &Config::default(), &baseline);
    for profile in PROFILES {
        for (preference, selected) in [
            (StructurePreference::Natural, StructureForm::Natural),
            (StructurePreference::Dispatcher, StructureForm::Dispatcher),
        ] {
            let mut config = Config::default();
            config.readability.profile = profile;
            config.readability.procedures = ProcedurePreference::Inline;
            assert_eq!(readability_source(&input, &config), anchor.source);
            choose(&mut config, anchor.clone(), preference);
            let artifact = checked(&input, &config);
            let decision = artifact
                .readability
                .iter()
                .find(|d| d.region == anchor)
                .expect("retained root choice");
            assert_eq!(decision.preference, preference);
            assert_eq!(decision.selected, selected);
            assert_eq!(decision.reason, ReadabilityReason::UserPreference);
            let root = &artifact
                .current()
                .unwrap()
                .ir
                .structured
                .as_ref()
                .unwrap()
                .root;
            assert_eq!(
                matches!(root.kind, StructuredKind::Dispatcher { .. }),
                selected == StructureForm::Dispatcher
            );
            for words in [[0, 0], [0, 1], [1, 0], [1, 1]] {
                let (outcome, _, _) = support::source_execution(
                    &input,
                    &artifact,
                    fixtures::host(&input, &words),
                    &ExecutionLimits::default(),
                );
                assert!(outcome.is_ok());
            }
        }
    }
}

#[test]
fn stale_source_and_unmatched_boundaries_cannot_redirect_a_different_region() {
    let input = fixtures::conditional_tail();
    let config = Config::default();
    let baseline = checked(&input, &config);
    let anchor = root_anchor(&input, &config, &baseline);
    let mut stale = anchor.clone();
    stale.source = ArtifactId("stale-source".into());
    let mut wrong_entry = anchor.clone();
    wrong_entry.entry = EntityId(u64::MAX);
    let mut wrong_continuation = anchor.clone();
    wrong_continuation.continuation = Some(anchor.entry);
    let mut wrong_loop = anchor.clone();
    wrong_loop.loop_header = Some(anchor.entry);
    for invalid in [stale, wrong_entry, wrong_continuation, wrong_loop] {
        let mut config = config.clone();
        choose(&mut config, invalid, StructurePreference::Dispatcher);
        let artifact = checked(&input, &config);
        assert!(
            artifact
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::Unsupported && d.severity == Severity::Info)
        );
        assert!(
            !artifact
                .readability
                .iter()
                .any(|d| d.reason == ReadabilityReason::UserPreference)
        );
        assert_eq!(
            artifact.current().unwrap().ir.structured,
            baseline.current().unwrap().ir.structured
        );
        oracle::certify(
            &input,
            &artifact,
            fixtures::host(&input, &[1, 0]),
            &ExecutionLimits::default(),
        );
    }
    let mut changed_recovery = config;
    changed_recovery.limits.max_constant_set = 16;
    assert_ne!(readability_source(&input, &changed_recovery), anchor.source);
    choose(
        &mut changed_recovery,
        anchor,
        StructurePreference::Dispatcher,
    );
    let artifact = checked(&input, &changed_recovery);
    assert!(
        artifact
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::Unsupported && d.severity == Severity::Info)
    );
    assert!(
        !artifact
            .readability
            .iter()
            .any(|d| d.reason == ReadabilityReason::UserPreference)
    );
}

#[test]
fn dispatcher_preference_reports_a_strategy_conflict_without_silently_changing_policy() {
    let input = fixtures::conditional_tail();
    let mut config = Config {
        strategy: Strategy::CodeDuplication,
        ..Config::default()
    };
    let baseline = checked(&input, &config);
    let anchor = root_anchor(&input, &config, &baseline);
    choose(&mut config, anchor.clone(), StructurePreference::Dispatcher);
    let artifact = checked(&input, &config);
    let decision = artifact
        .readability
        .iter()
        .find(|d| d.region == anchor)
        .expect("unavailable preference has an explicit receipt");
    assert_eq!(decision.preference, StructurePreference::Dispatcher);
    assert_eq!(decision.selected, StructureForm::Natural);
    assert_eq!(decision.reason, ReadabilityReason::PreferenceFallback);
    assert_eq!(artifact.config.strategy, Strategy::CodeDuplication);
    oracle::certify(
        &input,
        &artifact,
        fixtures::host(&input, &[1, 0]),
        &ExecutionLimits::default(),
    );
}

#[test]
fn inline_and_shared_procedure_policies_preserve_dynamic_arguments_and_external_effects() {
    let input = fixtures::sequential();
    for profile in PROFILES {
        for preference in [ProcedurePreference::Inline, ProcedurePreference::Shared] {
            let mut config = Config::default();
            config.readability.profile = profile;
            config.readability.procedures = preference;
            let artifact = checked(&input, &config);
            let high = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
            if preference == ProcedurePreference::Inline {
                assert!(high.procedures.is_empty());
                assert!(high.invocations.is_empty());
            } else {
                assert!(
                    !high.procedures.is_empty(),
                    "fixture has useful repeated source regions"
                );
                assert!(high.invocations.len() >= 2);
            }
            for words in [[4, 9], [1, 2], [0, 9], [4, 0]] {
                let report = oracle::certify(
                    &input,
                    &artifact,
                    fixtures::sequential_host(&input, &words),
                    &ExecutionLimits::default(),
                );
                if preference == ProcedurePreference::Inline {
                    assert!(report.calls.is_empty());
                }
                if words == [4, 9] {
                    for (opcode, expected) in [(0x55, 6), (0xa0, 2), (0xfa, 2), (0x3e, 2)] {
                        assert_eq!(
                            report
                                .host
                                .invocations
                                .iter()
                                .filter(|call| call.opcode == opcode)
                                .count(),
                            expected
                        );
                    }
                    if preference == ProcedurePreference::Shared {
                        assert!(!report.calls.is_empty());
                    }
                }
            }
        }
    }
}

#[test]
fn expanding_one_arithmetic_call_uses_its_own_source_and_preserves_other_occurrences() {
    assert_expansion(fixtures::repeated_squares());
}

#[test]
fn explicit_sharing_retains_expandable_parameterized_arithmetic_templates() {
    assert_expansion(evm_decompile::Input::new(
        evm_bytecode::parse_hex(include_str!(
            "fixtures/shared-procedures/arithmetic-template-inputs.hex"
        ))
        .unwrap(),
        evm_decompile::Fork::Osaka,
    ));
}

fn assert_expansion(input: evm_decompile::Input) {
    use std::collections::BTreeSet;

    let mut config = Config::default();
    config.readability.procedures = ProcedurePreference::Shared;
    let artifact = checked(&input, &config);
    let original = artifact.clone();
    let ir = &artifact.current().unwrap().ir;
    let high = ir.lifted.as_ref().unwrap();
    let mut renderer = high.render_index(ir);
    let base = high.render_base_index_with_budget(ir, |_| true);
    let call = high
        .invocations
        .iter()
        .find(|call| renderer.expression(call.source).is_some())
        .expect("expression invocation");
    let other = high
        .invocations
        .iter()
        .find(|other| other.source != call.source && other.procedure == call.procedure)
        .expect("same definition with distinct source bindings");
    let prefix = format!("pure_arithmetic_{}(", call.procedure.0);
    assert!(
        renderer
            .render_expression(call.source, |_| true)
            .unwrap()
            .contains(&prefix)
    );
    let statement = high
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.inputs.contains(&call.source))
        .expect("the arithmetic result feeds a retained primitive");
    let expected_expression = base.render_expression(call.source, |_| true).unwrap();
    let expected_statement = base
        .render_statement_with_dependencies(statement, false, |_| true)
        .unwrap();
    renderer
        .expand_shared_sources(&BTreeSet::from([call.source]), |_| true)
        .unwrap();
    assert!(renderer.shared_invocation(call.source).is_none());
    assert_eq!(renderer.shared_invocation(other.source), Some(other));
    assert_eq!(
        renderer.render_expression(call.source, |_| true).unwrap(),
        expected_expression
    );
    assert_eq!(
        renderer
            .render_statement_with_dependencies(statement, false, |_| true)
            .unwrap(),
        expected_statement
    );
    assert!(
        renderer
            .render_expression(other.source, |_| true)
            .unwrap()
            .contains(&prefix)
    );
    assert!(matches!(
        renderer.expand_shared_sources(&BTreeSet::from([EntityId(u64::MAX)]), |_| true),
        Err(evm_decompile::LiftedRenderError::InvalidSharedSource(
            EntityId(u64::MAX)
        ))
    ));
    assert!(
        renderer.shared_invocation(call.source).is_none(),
        "a rejected edit must retain the installed expansion"
    );
    assert!(
        renderer
            .expand_shared_sources(&BTreeSet::new(), |_| false)
            .is_err()
    );
    assert!(renderer.shared_invocation(call.source).is_none());
    renderer
        .expand_shared_sources(&BTreeSet::new(), |_| true)
        .unwrap();
    assert_eq!(renderer.shared_invocation(call.source), Some(call));
    assert_eq!(artifact, original);
    oracle::certify(
        &input,
        &artifact,
        fixtures::host(&input, &[5, 9, 17]),
        &ExecutionLimits::default(),
    );
}

#[test]
fn automatic_profiles_choose_different_verified_forms_for_a_shared_indirect_suffix() {
    let input = fixtures::indirect_shared_suffix();
    let mut choices = Vec::new();
    for profile in [ReadabilityProfile::Balanced, ReadabilityProfile::Compact] {
        let mut config = Config::default();
        config.readability.profile = profile;
        config.readability.procedures = ProcedurePreference::Inline;
        let artifact = checked(&input, &config);
        let entry = artifact.current().unwrap().ir.entry.unwrap();
        let decision = artifact
            .readability
            .iter()
            .find(|d| d.region.entry == entry && d.region.continuation.is_none())
            .unwrap();
        eprintln!("automatic public fixture {profile:?}: {decision:?}");
        choices.push(decision.selected);
        for word in [0, 1, 2, 3] {
            let (outcome, host, _) = support::source_execution(
                &input,
                &artifact,
                fixtures::host(&input, &[word]),
                &ExecutionLimits::default(),
            );
            assert!(outcome.is_ok());
            assert_eq!(
                host.state.storage[&evm_decompile::U256::ZERO],
                evm_decompile::U256::from(7)
            );
            assert_eq!(
                host.invocations
                    .iter()
                    .filter(|call| call.opcode == 0x55)
                    .count(),
                1
            );
        }
    }
    assert_eq!(choices, [StructureForm::Natural, StructureForm::Dispatcher]);
}

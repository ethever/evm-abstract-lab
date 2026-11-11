//! Public execution/export boundaries use cheap inputs with very large ceilings.

#![cfg(target_pointer_width = "64")]

use evm_decompile::{
    Cancellation, Completion, Config, DiagnosticCode, Fork, Input, Limits, Phase, StageKind,
    TextExportStatus, decompile, decompile_with_cancel, decompile_with_progress,
    export_text_with_observer,
};

fn legacy_config() -> Config {
    Config {
        limits: Limits {
            max_bytecode_bytes: 100_000,
            max_blocks: 8_192,
            max_operations: 100_000,
            max_values: 500_000,
            max_work: 100_000_000,
            max_iterations: 100_000,
            max_constant_set: 8,
            max_cloned_blocks: 2_048,
            max_structuring_depth: 128,
            max_history_entities: 2_000_000,
        },
        ..Config::default()
    }
}

#[test]
fn explicit_legacy_configuration_changes_identity_only_with_declared_schema() {
    let config = legacy_config();
    let artifact = decompile(&Input::new(vec![0], Fork::Osaka), &config);
    assert_eq!(artifact.completion, Completion::Complete);
    assert_eq!(artifact.config, config);
    // Preserve the frozen schema-2 key from base 2bc96535. The same explicit
    // request gets a new key solely because current producers declare schema 7 and reading preferences.
    let mut schema_two = b"osaka\0\0".to_vec();
    schema_two.extend_from_slice(
        format!(
            "schema=2;config=Config {{ strategy: {:?}, limits: {:?} }}",
            config.strategy, config.limits
        )
        .as_bytes(),
    );
    assert_eq!(
        format!("{:x}", alloy_primitives::keccak256(schema_two)),
        "3c2f30491ba389dd359ad41295e2c42732a631ed7c827b94272b443d4427e3b8"
    );
    let mut schema_three = b"osaka\0\0".to_vec();
    schema_three.extend_from_slice(
        format!(
            "schema=3;config=Config {{ strategy: {:?}, limits: {:?} }}",
            config.strategy, config.limits
        )
        .as_bytes(),
    );
    assert_eq!(
        format!("{:x}", alloy_primitives::keccak256(schema_three)),
        "1cbc4f5e0e58c2624819ab4a8a6bd995420398b147f8bbade434acfb4b5e5dfd"
    );
    assert_eq!(artifact.schema_version, 10);
    let mut schema_current = b"osaka\0\0".to_vec();
    schema_current.extend_from_slice(format!("schema=10;config={config:?}").as_bytes());
    assert_eq!(
        artifact.id.0,
        format!("{:x}", alloy_primitives::keccak256(schema_current))
    );
}

#[test]
fn stop_with_wide_resource_ceilings_preserves_config_and_does_only_actual_work() {
    let input = Input::new(vec![0], Fork::Osaka);
    let expected = decompile(&input, &legacy_config());
    assert_eq!(expected.completion, Completion::Complete);
    for count in [
        4_294_967_296,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let config = Config {
            limits: Limits {
                max_bytecode_bytes: count,
                max_blocks: count,
                max_operations: count,
                max_values: count,
                max_work: count,
                max_iterations: count,
                max_cloned_blocks: count,
                max_history_entities: count,
                max_constant_set: 8,
                max_structuring_depth: 512,
            },
            ..Config::default()
        };
        let mut observations = Vec::new();
        let artifact = decompile_with_progress(&input, &config, &Cancellation::new(), |p| {
            observations.push(p)
        });
        assert_eq!(artifact.completion, Completion::Complete, "{count}");
        assert_eq!(artifact.config, config);
        assert_eq!(artifact.statistics, expected.statistics);
        assert_eq!(artifact.stages, expected.stages);
        assert_eq!(artifact.events, expected.events);
        assert_eq!(artifact.mappings, expected.mappings);
        assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
        let final_progress = observations.last().unwrap();
        assert_eq!(final_progress.phase, Phase::Complete);
        assert_eq!(final_progress.work, artifact.statistics.work);
        assert_eq!(final_progress.blocks, artifact.statistics.blocks);
        assert_eq!(final_progress.operations, artifact.statistics.operations);
        assert_eq!(final_progress.stage, artifact.current_stage);
        assert!(observations.windows(2).all(|p| p[0].work <= p[1].work));
        assert!(artifact.statistics.work > 0 && artifact.statistics.work < 1_000_000);
        assert_eq!(artifact.statistics.blocks, 1);
        assert_eq!(artifact.statistics.operations, 1);
        let encoded = serde_json::to_vec(&artifact).unwrap();
        let decoded: evm_decompile::Artifact = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, artifact);

        let mut paid: Vec<u64> = Vec::new();
        let export = export_text_with_observer(&artifact, &Cancellation::new(), count, |work| {
            paid.push(work)
        });
        assert_eq!(export.status, TextExportStatus::Complete);
        assert!(export.text.contains("evm.stop();"));
        assert!(export.work > 0 && export.work < 1_000_000);
        assert_eq!(paid.last(), Some(&export.work));
        assert!(paid.windows(2).all(|p| p[0] <= p[1]));
    }
}

#[test]
fn zero_and_small_allowances_retain_typed_limit_and_cancellation_semantics() {
    let input = Input::new(vec![0], Fork::Osaka);
    for max_work in [0, 1] {
        let mut config = Config::default();
        config.limits.max_work = max_work;
        let artifact = decompile(&input, &config);
        assert_eq!(artifact.config, config);
        assert_eq!(artifact.completion, Completion::Limited);
        assert!(artifact.statistics.work <= max_work);
        assert!(artifact.stages.is_empty());
        assert!(
            artifact
                .diagnostics
                .iter()
                .any(|d| d.code == DiagnosticCode::Limit)
        );
    }
    let artifact = decompile(&input, &Config::default());
    for max_work in [0, 1] {
        let export = export_text_with_observer(&artifact, &Cancellation::new(), max_work, |_| {});
        assert_eq!(export.status, TextExportStatus::Limited);
        assert!(export.work <= max_work);
    }
    let cancel = Cancellation::new();
    cancel.cancel();
    let cancelled = decompile_with_cancel(&input, &Config::default(), &cancel);
    assert_eq!(cancelled.completion, Completion::Cancelled);
    assert_eq!(cancelled.statistics.work, 0);
    assert!(cancelled.stages.is_empty());
    assert!(
        cancelled
            .diagnostics
            .iter()
            .any(|d| d.code == DiagnosticCode::Cancelled)
    );
    assert_eq!(
        export_text_with_observer(&artifact, &cancel, u64::MAX, |_| {}).status,
        TextExportStatus::Cancelled
    );
}

#[test]
fn hard_depth_and_nonzero_precision_guards_survive_maximum_resource_defaults() {
    let input = Input::new(vec![0], Fork::Osaka);
    let mut config = Config::default();
    config.limits.max_structuring_depth = 512;
    assert_eq!(decompile(&input, &config).completion, Completion::Complete);
    for (field, config) in [
        (
            "max_structuring_depth",
            Config {
                limits: Limits {
                    max_structuring_depth: 513,
                    ..Limits::default()
                },
                ..Config::default()
            },
        ),
        (
            "max_constant_set",
            Config {
                limits: Limits {
                    max_constant_set: 0,
                    ..Limits::default()
                },
                ..Config::default()
            },
        ),
    ] {
        let artifact = decompile(&input, &config);
        assert_eq!(artifact.config, config);
        assert_eq!(artifact.completion, Completion::Failed);
        assert!(artifact.stages.is_empty());
        assert!(
            artifact
                .diagnostics
                .iter()
                .any(|d| { d.code == DiagnosticCode::Limit && d.message.contains(field) }),
            "{field}: {:?}",
            artifact.diagnostics
        );
    }
}

//! Reading preferences retain strict transport and recovery-source boundaries.
use evm_decompile_model::*;
use serde_json::json;

fn empty_artifact(config: Config) -> Artifact {
    Artifact {
        schema_version: SCHEMA_VERSION,
        id: ArtifactId("0".repeat(64)),
        input: Input::new(vec![0], Fork::Osaka),
        config,
        completion: Completion::Complete,
        diagnostics: Vec::new(),
        stages: Vec::new(),
        events: Vec::new(),
        mappings: SourceMappings::default(),
        current_stage: None,
        statistics: Statistics::default(),
        readability: Vec::new(),
    }
}

#[test]
fn region_identity_survives_reading_changes_but_not_recovery_changes() {
    let input = Input::new(vec![0x5f, 0x00], Fork::Osaka);
    let mut config = Config::default();
    let source = readability_source(&input, &config);
    config.readability.profile = ReadabilityProfile::Linear;
    config.readability.procedures = ProcedurePreference::Inline;
    config.readability.regions.push(RegionPreference {
        region: RegionAnchor {
            source: source.clone(),
            entry: EntityId(1),
            continuation: Some(EntityId(9)),
            loop_header: None,
        },
        structure: StructurePreference::Natural,
    });
    assert_eq!(source, readability_source(&input, &config));
    config.limits.max_constant_set += 1;
    assert_ne!(source, readability_source(&input, &config));
    assert_ne!(
        source,
        readability_source(&Input::new(vec![0x00], Fork::Osaka), &Config::default())
    );
    assert_ne!(
        source,
        readability_source(
            &Input::new(input.runtime_bytecode, Fork::Cancun),
            &Config::default()
        )
    );
}

#[test]
fn historical_config_omission_is_distinct_from_malformed_preferences() {
    let value = serde_json::to_value(Config::default()).unwrap();
    assert!(value.get("readability").is_none());
    assert_eq!(
        serde_json::from_value::<Config>(value.clone()).unwrap(),
        Config::default()
    );
    for invalid in [json!(null), json!([]), json!({"profile":"linear"})] {
        let mut malformed = value.clone();
        malformed["readability"] = invalid;
        assert!(serde_json::from_value::<Config>(malformed).is_err());
    }
    let mut config = Config::default();
    config.readability.profile = ReadabilityProfile::Linear;
    assert_eq!(
        serde_json::from_value::<Config>(serde_json::to_value(&config).unwrap()).unwrap(),
        config
    );
}

#[test]
fn older_schema_cannot_advertise_new_preference_or_decision_contracts() {
    let mut artifact = empty_artifact(Config::default());
    artifact.schema_version = 6;
    assert!(artifact.admits_schema());
    artifact.config.readability.profile = ReadabilityProfile::Compact;
    assert!(!artifact.admits_schema());
    artifact.config.readability = ReadabilityConfig::default();
    artifact.readability.push(RegionDecision {
        region: RegionAnchor {
            source: readability_source(&artifact.input, &artifact.config),
            entry: EntityId(1),
            continuation: None,
            loop_header: None,
        },
        preference: StructurePreference::Dispatcher,
        selected: StructureForm::Dispatcher,
        reason: ReadabilityReason::UserPreference,
        natural: None,
        dispatcher: Some(ReadabilityMetrics::default()),
    });
    assert!(!artifact.admits_schema());
    artifact.schema_version = SCHEMA_VERSION;
    assert!(artifact.admits_schema());
}

#[test]
fn precise_dispatcher_reasons_roundtrip_and_require_schema_nine() {
    for (reason, wire) in [
        (ReadabilityReason::ResidualCycle, "residual_cycle"),
        (ReadabilityReason::ControlReentry, "control_reentry"),
        (ReadabilityReason::ValidationFallback, "validation_fallback"),
    ] {
        let mut artifact = empty_artifact(Config::default());
        artifact.readability.push(RegionDecision {
            region: RegionAnchor {
                source: artifact.readability_source(),
                entry: EntityId(1),
                continuation: None,
                loop_header: None,
            },
            preference: StructurePreference::Natural,
            selected: StructureForm::Dispatcher,
            reason,
            natural: None,
            dispatcher: Some(ReadabilityMetrics::default()),
        });
        assert!(artifact.admits_schema());
        let encoded = serde_json::to_value(&artifact).unwrap();
        assert_eq!(encoded["readability"][0]["reason"], wire);
        assert_eq!(
            serde_json::from_value::<Artifact>(encoded).unwrap(),
            artifact
        );
        for schema in [7, 8] {
            artifact.schema_version = schema;
            let decoded: Artifact =
                serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
            assert!(
                !decoded.admits_schema(),
                "{reason:?} cannot be advertised under schema {schema}"
            );
        }
    }
}

#[test]
fn legacy_dispatcher_label_retains_its_wire_value_and_source_namespace() {
    for schema in [7, 8] {
        let mut artifact = empty_artifact(Config::default());
        artifact.schema_version = schema;
        let source = artifact.readability_source();
        assert_ne!(
            source,
            readability_source(&artifact.input, &artifact.config)
        );
        artifact.readability.push(RegionDecision {
            region: RegionAnchor {
                source,
                entry: EntityId(1),
                continuation: None,
                loop_header: None,
            },
            preference: StructurePreference::Auto,
            selected: StructureForm::Dispatcher,
            reason: ReadabilityReason::MultiEntryControl,
            natural: None,
            dispatcher: Some(ReadabilityMetrics::default()),
        });
        assert!(artifact.admits_schema());
        let encoded = serde_json::to_string(&artifact).unwrap();
        assert!(encoded.contains("\"reason\":\"multi_entry_control\""));
        let decoded: Artifact = serde_json::from_str(&encoded).unwrap();
        assert!(decoded.admits_schema());
        assert_eq!(decoded, artifact);
        assert_eq!(serde_json::to_string(&decoded).unwrap(), encoded);
    }
}

use evm_decompile_model::{
    Artifact, ArtifactId, Completion, Config, EntityId, Fork, Input, Ir, Limits, Progress,
    SCHEMA_VERSION, SourceMappings, SourceSpan, Stage, StageDiff, StageId, StageKind, Statistics,
    StructuredKind, StructuredNode, StructuredProgram, Validation, ValidationStatus,
};
use serde::{Serialize, de::DeserializeOwned};

fn strict<T: Serialize + DeserializeOwned>(value: &T) {
    let wire = serde_json::to_value(value).unwrap();
    let object = wire.as_object().unwrap();
    let _: T = serde_json::from_value(wire.clone()).unwrap();
    for key in object.keys() {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(
            serde_json::from_value::<T>(missing).is_err(),
            "missing {key}"
        );
    }
    let mut unknown = wire.clone();
    unknown["unexpected_field"] = serde_json::json!(null);
    assert!(serde_json::from_value::<T>(unknown).is_err());
    assert!(
        serde_json::from_value::<T>(serde_json::Value::Array(object.values().cloned().collect()))
            .is_err()
    );
    if let Some((key, value)) = object.iter().next() {
        let json = serde_json::to_string(&wire).unwrap();
        let duplicate = format!("{{{key:?}:{value},{}", &json[1..]);
        assert!(serde_json::from_str::<T>(&duplicate).is_err());
    }
}

#[test]
fn flat_structure_wire_roundtrips_deep_trees_and_rejects_invalid_references() {
    let mut root = StructuredNode {
        id: EntityId(1),
        kind: StructuredKind::Break,
    };
    for level in 0..100_u64 {
        root = StructuredNode {
            id: EntityId(level * 2 + 2),
            kind: StructuredKind::While {
                header: Box::new(StructuredNode {
                    id: EntityId(level * 2 + 3),
                    kind: StructuredKind::Sequence(Vec::new()),
                }),
                condition: None,
                condition_nonzero: true,
                body: Box::new(root),
                exit_edge: None,
            },
        };
    }
    let program = StructuredProgram { root };
    let bytes = serde_json::to_vec(&program).unwrap();
    let restored: StructuredProgram = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored, program);
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(wire["root"].is_string());
    assert_eq!(wire["nodes"].as_array().unwrap().len(), 201);
    strict(&program);

    for malformed in [
        // Missing root, missing child, duplicated identity, a cycle, a shared
        // child, and unreachable records are independent admission failures.
        serde_json::json!({"root":"1","nodes":[]}),
        serde_json::json!({"root":"1","nodes":[{"id":"1","kind":{"sequence":["2"]}}]}),
        serde_json::json!({"root":"1","nodes":[{"id":"1","kind":"break"},{"id":"1","kind":"break"}]}),
        serde_json::json!({"root":"1","nodes":[{"id":"1","kind":{"sequence":["1"]}}]}),
        serde_json::json!({"root":"1","nodes":[{"id":"1","kind":{"sequence":["2","2"]}},{"id":"2","kind":"break"}]}),
        serde_json::json!({"root":"1","nodes":[{"id":"1","kind":"break"},{"id":"2","kind":"break"}]}),
    ] {
        assert!(serde_json::from_value::<StructuredProgram>(malformed).is_err());
    }
    let mut missing_nullable = wire;
    missing_nullable["nodes"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find_map(|node| node["kind"].get_mut("while"))
        .unwrap()
        .as_object_mut()
        .unwrap()
        .remove("condition");
    assert!(serde_json::from_value::<StructuredProgram>(missing_nullable).is_err());
}

#[test]
fn artifact_records_reject_arrays_and_missing_nullable_fields() {
    let artifact = Artifact {
        schema_version: SCHEMA_VERSION,
        id: ArtifactId("0".repeat(64)),
        input: Input::new(Vec::new(), Fork::Osaka),
        config: Config::default(),
        completion: Completion::Limited,
        diagnostics: Vec::new(),
        stages: Vec::new(),
        events: Vec::new(),
        mappings: SourceMappings::default(),
        current_stage: None,
        statistics: Statistics::default(),
    };
    let stage = Stage {
        id: StageId(0),
        parent: None,
        kind: StageKind::Decoded,
        ir: Ir::default(),
        validation: Validation {
            status: ValidationStatus::Passed,
            checks: Vec::new(),
        },
    };
    strict(&artifact);
    strict(&artifact.input);
    strict(&artifact.config);
    strict(&artifact.config.limits);
    strict(&artifact.mappings);
    strict(&artifact.statistics);
    strict(&stage);
    strict(&stage.ir);
    strict(&stage.validation);
    strict(&Progress::default());
    strict(&SourceSpan { pc: 0, end_pc: 1 });
    strict(&StageDiff::default());
}

#[test]
fn request_records_are_strict_objects_with_no_defaulted_missing_limits() {
    assert_eq!(SCHEMA_VERSION, 1);
    let config = Config::default();
    let wire = serde_json::to_value(&config).unwrap();
    assert_eq!(
        serde_json::from_value::<Config>(wire.clone()).unwrap(),
        config
    );
    let mut typo = wire.clone();
    typo["limits"]["max_cloned_bloks"] = serde_json::json!(0);
    assert!(serde_json::from_value::<Config>(typo).is_err());
    let mut missing = wire;
    missing["limits"]
        .as_object_mut()
        .unwrap()
        .remove("max_work");
    assert!(serde_json::from_value::<Config>(missing).is_err());
    let values = serde_json::to_value(Limits::default())
        .unwrap()
        .as_object()
        .unwrap()
        .values()
        .cloned()
        .collect::<Vec<_>>();
    assert!(serde_json::from_value::<Limits>(serde_json::Value::Array(values)).is_err());
    assert!(serde_json::from_str::<Input>("[[0],\"osaka\"]").is_err());
    assert!(serde_json::from_str::<Config>("[\"dispatcher\",{}]").is_err());
}
#[test]
fn entity_ids_are_decimal_strings_and_fork_roundtrips_strictly() {
    for id in [0, (1_u64 << 53) + 1, u64::MAX] {
        let identity = EntityId(id);
        let encoded = serde_json::to_string(&identity).unwrap();
        assert_eq!(encoded, format!("\"{id}\""));
        assert_eq!(
            serde_json::from_str::<EntityId>(&encoded).unwrap(),
            identity
        );
        assert!(serde_json::from_str::<EntityId>(&id.to_string()).is_err());
    }
    for fork in [Fork::Cancun, Fork::Prague, Fork::Osaka] {
        let input = Input::new(vec![0x5f, 0x00], fork);
        assert_eq!(
            serde_json::from_slice::<Input>(&serde_json::to_vec(&input).unwrap()).unwrap(),
            input
        );
    }
    assert!(serde_json::from_str::<Input>(r#"{"runtime_bytecode":[],"fork":"future"}"#).is_err());
}

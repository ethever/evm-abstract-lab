//! Schema and bounded transport for executable definition/invocation records.
use evm_decompile_model::{
    Artifact, EntityId, LiftedProgram, MAX_SHARED_REGION_ENTITIES, SCHEMA_VERSION, SharedBinding,
    SharedContinuation, SharedInterface, SharedInvocation, SharedProcedure,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

fn program() -> LiftedProgram {
    LiftedProgram {
        procedures: vec![SharedProcedure {
            kind: evm_decompile_model::SharedProcedureKind::Normal,
            id: EntityId(100),
            body: EntityId(21),
            interface: SharedInterface {
                inputs: vec![EntityId(11), EntityId(12)],
                outputs: vec![EntityId(13), EntityId(14)],
                memory_inputs: vec![EntityId(15)],
                memory_outputs: vec![EntityId(16)],
                effect_inputs: vec![EntityId(17)],
                effect_outputs: vec![EntityId(18)],
                continuations: vec![SharedContinuation {
                    edge: EntityId(19),
                    target: EntityId(20),
                }],
            },
        }],
        invocations: vec![SharedInvocation {
            id: EntityId(101),
            procedure: EntityId(100),
            source: EntityId(22),
            owner: Some(EntityId(23)),
            position: 1,
            continuation: Some(EntityId(24)),
            bindings: vec![SharedBinding {
                formal: EntityId(11),
                actual: EntityId(31),
            }],
        }],
        ..LiftedProgram::default()
    }
}

fn strict<T: Serialize + DeserializeOwned>(value: &T) {
    let wire = serde_json::to_value(value).unwrap();
    let fields = wire.as_object().unwrap();
    assert!(serde_json::from_value::<T>(wire.clone()).is_ok());
    for key in fields.keys() {
        let mut missing = wire.clone();
        missing.as_object_mut().unwrap().remove(key);
        if key == "kind" {
            // A missing contract is a legacy record, never an implicit claim
            // that an old body satisfies the new local-return rules.
            assert!(serde_json::from_value::<T>(missing).is_ok());
            continue;
        }
        assert!(
            serde_json::from_value::<T>(missing).is_err(),
            "missing {key}"
        );
    }
    let mut unknown = wire.clone();
    unknown["forged"] = json!(null);
    assert!(serde_json::from_value::<T>(unknown).is_err());
    assert!(serde_json::from_value::<T>(json!(fields.values().collect::<Vec<_>>())).is_err());
    let (key, value) = fields.iter().next().unwrap();
    let wire = serde_json::to_string(&wire).unwrap();
    let duplicate = format!("{{{key:?}:{value},{}", &wire[1..]);
    assert!(serde_json::from_str::<T>(&duplicate).is_err());
}

#[test]
fn shared_definition_call_and_state_maps_are_strict_lossless_records() {
    let high = program();
    strict(&high.procedures[0]);
    strict(&high.procedures[0].interface);
    strict(&high.procedures[0].interface.continuations[0]);
    strict(&high.invocations[0]);
    strict(&high.invocations[0].bindings[0]);
    let encoded = serde_json::to_vec(&high).unwrap();
    assert_eq!(
        serde_json::from_slice::<LiftedProgram>(&encoded).unwrap(),
        high
    );
    let mut huge_ids = high.invocations[0].clone();
    huge_ids.id = EntityId(u64::MAX);
    huge_ids.bindings[0].actual = EntityId(u64::MAX - 1);
    huge_ids.position = usize::MAX;
    assert_eq!(
        serde_json::from_slice::<SharedInvocation>(&serde_json::to_vec(&huge_ids).unwrap())
            .unwrap(),
        huge_ids
    );
    assert_eq!(high.invocations[0].actual(EntityId(11)), Some(EntityId(31)));
    assert_eq!(
        high.invocations[0].actual(EntityId(12)),
        Some(EntityId(12)),
        "omitted identity bindings remain executable"
    );
}

#[test]
fn checked_procedures_require_schema_six_and_retain_legacy_wire_shapes() {
    assert_eq!(SCHEMA_VERSION, 8);
    let legacy = include_str!("fixtures/partial-artifact.json").trim_end();
    let original: Artifact = serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_string(&original).unwrap(), legacy);
    for version in 1..=5 {
        let mut artifact = original.clone();
        artifact.schema_version = version;
        assert!(artifact.admits_schema(), "legacy v{version}");
    }
    let mut shared = original;
    shared.schema_version = 6;
    let stage = shared.stages.last_mut().unwrap();
    stage.kind = evm_decompile_model::StageKind::Lifted;
    stage.ir.lifted = Some(program());
    assert!(shared.admits_schema());
    for version in [1, 2, 3, 4, 5, SCHEMA_VERSION + 1] {
        let mut wrong = shared.clone();
        wrong.schema_version = version;
        assert!(
            !wrong.admits_schema(),
            "procedure semantics forged as schema {version}"
        );
    }
    let mut legacy_shared = shared.clone();
    legacy_shared.schema_version = 5;
    let high = legacy_shared
        .stages
        .last_mut()
        .unwrap()
        .ir
        .lifted
        .as_mut()
        .unwrap();
    high.procedures[0].kind = evm_decompile_model::SharedProcedureKind::Legacy;
    let wire = serde_json::to_value(&high.procedures[0]).unwrap();
    assert!(wire.get("kind").is_none());
    assert_eq!(
        serde_json::from_value::<SharedProcedure>(wire).unwrap(),
        high.procedures[0]
    );
    assert!(legacy_shared.admits_schema());
    for legacy_high in [
        r#"{"expressions":[],"blocks":[],"transfers":[],"controls":[]}"#,
        r#"{"expressions":[],"blocks":[],"transfers":[],"controls":[],"failures":[]}"#,
    ] {
        let decoded: LiftedProgram = serde_json::from_str(legacy_high).unwrap();
        assert!(decoded.procedures.is_empty());
        assert!(decoded.invocations.is_empty());
    }
    for field in ["procedures", "invocations"] {
        let mut invalid = serde_json::to_value(LiftedProgram::default()).unwrap();
        invalid[field] = json!(null);
        assert!(
            serde_json::from_value::<LiftedProgram>(invalid).is_err(),
            "null {field}"
        );
    }
}

#[test]
fn binding_limit_precedes_an_unparsed_malformed_suffix() {
    let mut prefix = String::from(
        r#"{"id":"101","procedure":"100","source":"22","owner":null,"position":0,"continuation":null,"bindings":["#,
    );
    for index in 0..=MAX_SHARED_REGION_ENTITIES {
        if index != 0 {
            prefix.push(',');
        }
        prefix.push_str(r#"{"formal":"11","actual":"31"}"#);
    }
    prefix.push_str(",{THIS_SUFFIX_MUST_NOT_BE_PARSED");
    let error = serde_json::from_str::<SharedInvocation>(&prefix)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("limit"),
        "resource admission must precede suffix decode: {error}"
    );
}

#[test]
fn flat_admission_rejects_invalid_references_and_noncanonical_sparse_maps() {
    for mutation in [
        "procedure",
        "body",
        "source",
        "duplicate_call_identity",
        "unsorted",
        "duplicate_formal",
        "zero_formal",
        "zero_actual",
    ] {
        let mut high = program();
        match mutation {
            "procedure" => high.invocations[0].procedure = EntityId(999),
            "body" => high.procedures[0].body = EntityId(0),
            "source" => high.invocations[0].source = EntityId(0),
            "duplicate_call_identity" => {
                let mut call = high.invocations[0].clone();
                call.source = EntityId(25);
                high.invocations.push(call);
            }
            "unsorted" => {
                high.invocations[0].bindings.push(SharedBinding {
                    formal: EntityId(10),
                    actual: EntityId(30),
                });
            }
            "duplicate_formal" => {
                high.invocations[0].bindings.push(SharedBinding {
                    formal: EntityId(11),
                    actual: EntityId(32),
                });
            }
            "zero_formal" => high.invocations[0].bindings[0].formal = EntityId(0),
            "zero_actual" => high.invocations[0].bindings[0].actual = EntityId(0),
            _ => unreachable!(),
        }
        assert!(high.validate().is_err(), "flat model admitted {mutation}");
        let wire = serde_json::to_vec(&high).unwrap();
        assert!(
            serde_json::from_slice::<LiftedProgram>(&wire).is_err(),
            "wire admitted {mutation}"
        );
    }
}

#[test]
fn wide_simultaneous_transfer_bounds_the_combined_row_before_rhs_aggregation() {
    use evm_decompile_model::{
        Edge, EdgeKind, Ir, LiftedBlock, LiftedExpression, LiftedExpressionKind as Expression,
        LiftedRenderError, LiftedTransfer, MAX_LIFTED_EXPRESSION_TEXT, U256,
    };
    let source = EntityId(1000);
    let target = EntityId(2000);
    let mut high = LiftedProgram::default();
    for id in 1..=10_u64 {
        let kind = if id == 1 {
            Expression::Word(U256::from(1))
        } else {
            Expression::WordOp {
                opcode: 0x0a,
                arguments: vec![EntityId(id - 1); 2],
            }
        };
        high.expressions.push(LiftedExpression {
            id: EntityId(id),
            block: source,
            ty: kind.ty(),
            kind,
        });
    }
    let root = EntityId(10);
    let width = 128;
    high.blocks.push(LiftedBlock {
        id: EntityId(4000),
        block: target,
        parameters: (10000..10000 + width).map(EntityId).collect(),
        statements: vec![],
        condition: None,
        target: None,
        exit_values: vec![],
    });
    let transfer = LiftedTransfer {
        id: EntityId(5000),
        edge: EntityId(3000),
        arguments: vec![root; usize::try_from(width).unwrap()],
    };
    high.transfers.push(transfer.clone());
    high.validate().unwrap();
    let ir = Ir {
        edges: vec![Edge {
            id: transfer.edge,
            from: source,
            to: target,
            kind: EdgeKind::Fallthrough,
            arguments: vec![],
            memory: EntityId(2001),
            effect: EntityId(2002),
        }],
        ..Ir::default()
    };
    let index = high.render_index(&ir);
    let child = index.render_expression(root, |_| true).unwrap();
    assert!(
        child.len() < MAX_LIFTED_EXPRESSION_TEXT,
        "each individual RHS must be valid"
    );
    assert!(child.len() * usize::try_from(width).unwrap() > MAX_LIFTED_EXPRESSION_TEXT);
    assert_eq!(
        index.render_transfer(&transfer, |_| true),
        Err(LiftedRenderError::Limit),
        "combined row must reject while appending rather than materializing all128 RHS strings"
    );
}

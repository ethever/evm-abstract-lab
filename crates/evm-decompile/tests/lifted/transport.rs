use super::oracle::{certify, evaluate};
use super::support::{calldata, checked, comparison_branch, validated};
use super::{assert_unchanged_witness, entry_condition, lifted, loop_registers, structured};
use evm_decompile::{
    Artifact, Cancellation, Completion, Config, EntityId, EntityRef, Fork, Input,
    LiftedExpressionKind as Expr, LiftedProgram, LiftedRelation as Relation, LiftedType,
    LiftedValue, MemoryHost, Phase, StageKind, U256, ValidationStatus, decompile,
    decompile_with_progress, validate_stage,
};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn synthesized_mask_constant_inherits_rewrite_provenance() {
    let input = Input::new(
        vec![
            0x5f, 0x35, 0x60, 0xf0, 0x16, 0x60, 0xcc, 0x16, 0x5f, 0x55, 0x00,
        ],
        Fork::Osaka,
    );
    let artifact = checked(&input);
    for word in [
        U256::ZERO,
        U256::from(1),
        U256::from(0x40),
        U256::from(0x80),
        U256::from(0xcc),
        U256::from(0xf0),
        U256::from(0xff),
        U256::from(1) << 255,
        U256::MAX,
    ] {
        let (raw, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[word])),
        );
        assert_eq!(observed.storage[&U256::ZERO], word & U256::from(0xc0));
        for opcode in [0x35, 0x55] {
            assert_eq!(raw.trace.iter().filter(|op| op.opcode == opcode).count(), 1);
        }
    }
    assert_unchanged_witness(&artifact);
    let (stage, program) = lifted(&artifact);
    let parent = structured(&artifact);
    let responsible = parent
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .rfind(|op| op.opcode == 0x16)
        .unwrap();
    let source = responsible.source.unwrap();
    let mask = program
        .expressions
        .iter()
        .find(|expr| expr.kind == Expr::Word(U256::from(0xc0)))
        .expect("the combined mask is a new third constant, distinct from 0xf0 and 0xcc");
    let occurrence = EntityRef {
        stage: stage.id,
        entity: mask.id,
    };
    let origin = artifact.origin(occurrence).unwrap();
    assert!(
        !origin.source.is_empty(),
        "synthesized mask lost source: {origin:?}"
    );
    assert!(
        !origin.parents.is_empty(),
        "synthesized mask lost parents: {origin:?}"
    );
    assert!(origin.source.contains(&source));
    let parent_records = evm_decompile::records(&parent.ir);
    for ancestor in &origin.parents {
        assert_eq!(ancestor.stage, parent.id);
        assert!(parent_records.contains_key(&ancestor.entity));
        assert!(artifact.origin(*ancestor).is_some());
    }
    let responsible = EntityRef {
        stage: parent.id,
        entity: responsible.id,
    };
    assert!(origin.parents.contains(&responsible));
    assert!(artifact.derived_from(responsible).contains(&occurrence));
    assert!(artifact.descendants(source.pc).contains(&occurrence));
    let restored: Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(restored, artifact);
    assert_eq!(restored.origin(occurrence), Some(origin));
    assert!(restored.descendants(source.pc).contains(&occurrence));
}

#[test]
fn lifted_entities_have_roundtrip_provenance_and_deterministic_artifact_diffs() {
    let input = comparison_branch(0x10, 3);
    let artifact = checked(&input);
    assert_eq!(artifact, decompile(&input, &Config::default()));
    assert_eq!(
        artifact.schema_version, 12,
        "new high semantics require a truthful wire version"
    );
    let encoded = serde_json::to_vec(&artifact).unwrap();
    let restored: Artifact = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, artifact);
    let (stage, program) = lifted(&artifact);
    let diff = artifact.diff(structured(&artifact).id, stage.id).unwrap();
    let ids: BTreeSet<_> = program.entity_ids().collect();
    assert!(!ids.is_empty());
    assert_eq!(diff.added.iter().copied().collect::<BTreeSet<_>>(), ids);
    assert!(diff.removed.is_empty() && diff.changed.is_empty());
    for id in ids {
        let occurrence = EntityRef {
            stage: stage.id,
            entity: id,
        };
        let origin = artifact
            .origin(occurrence)
            .expect("every new high semantic entity has an origin");
        assert!(
            !origin.reason.is_empty() && !origin.parents.is_empty() && !origin.source.is_empty(),
            "untraceable high entity {origin:?}"
        );
        for parent in &origin.parents {
            assert!(artifact.origin(*parent).is_some());
        }
        for span in &origin.source {
            assert!(span.pc < span.end_pc && span.end_pc <= input.runtime_bytecode.len());
            assert!(restored.descendants(span.pc).contains(&occurrence));
        }
    }
    let root = entry_condition(&artifact);
    let occurrence = EntityRef {
        stage: stage.id,
        entity: root,
    };
    for source_pc in 5..=8 {
        assert!(
            artifact.descendants(source_pc).contains(&occurrence),
            "UGE loses consumed comparison/ISZERO ancestry at PC {source_pc}"
        );
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn malformed_predicate_types_scope_cycles_and_duplicate_records_are_rejected() {
    let artifact = checked(&comparison_branch(0x10, 3));
    let (stage, program) = lifted(&artifact);
    let root = entry_condition(&artifact);
    let root_index = program
        .expressions
        .iter()
        .position(|expr| expr.id == root)
        .unwrap();
    let mut mutations = Vec::new();
    let mut semantic = stage.clone();
    let Expr::Compare { relation, .. } =
        &mut semantic.ir.lifted.as_mut().unwrap().expressions[root_index].kind
    else {
        panic!("UGE predicate");
    };
    *relation = Relation::Ult;
    mutations.push(semantic);
    let mut wrong_sort = stage.clone();
    wrong_sort.ir.lifted.as_mut().unwrap().expressions[root_index].ty = LiftedType::Word256;
    mutations.push(wrong_sort);
    let mut wrong_scope = stage.clone();
    wrong_scope.ir.lifted.as_mut().unwrap().expressions[root_index].block = stage
        .ir
        .blocks
        .iter()
        .find(|block| block.id != program.expressions[root_index].block)
        .unwrap()
        .id;
    mutations.push(wrong_scope);
    let mut cycle = stage.clone();
    cycle.ir.lifted.as_mut().unwrap().expressions[root_index].kind = Expr::Not(root);
    mutations.push(cycle);
    let mut duplicate = stage.clone();
    duplicate
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .expressions
        .push(program.expressions[root_index].clone());
    mutations.push(duplicate);
    for candidate in mutations {
        assert_eq!(
            validate_stage(&candidate).status,
            ValidationStatus::Failed,
            "corrupt high semantic candidate validated"
        );
    }
    let mut wrong_transfer = checked(&loop_registers(false)).current().unwrap().clone();
    let transfer = wrong_transfer
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .transfers
        .iter_mut()
        .find(|transfer| transfer.arguments.len() >= 3)
        .unwrap();
    transfer.arguments.swap(0, 1);
    assert_eq!(
        validate_stage(&wrong_transfer).status,
        ValidationStatus::Failed,
        "swapped phi RHSs passed projection certification"
    );
}

#[test]
fn wire_admission_is_strict_and_legacy_absent_high_fields_remain_explicit() {
    let artifact = checked(&comparison_branch(0x10, 3));
    let (_, program) = lifted(&artifact);
    let wire = serde_json::to_string(program).unwrap();
    let unknown = wire.replacen('{', "{\"unrecognized_high_field\":null,", 1);
    assert!(serde_json::from_str::<LiftedProgram>(&unknown).is_err());
    let duplicate = wire.replacen('{', "{\"expressions\":[],", 1);
    assert!(serde_json::from_str::<LiftedProgram>(&duplicate).is_err());
    let mut positional = serde_json::to_value(program).unwrap();
    let expression = positional["expressions"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|expr| expr["kind"].get("compare").is_some())
        .unwrap();
    let payload = expression["kind"]["compare"].as_object().unwrap();
    expression["kind"]["compare"] =
        serde_json::json!([payload["relation"], payload["left"], payload["right"]]);
    assert!(
        serde_json::from_value::<LiftedProgram>(positional).is_err(),
        "named comparison payload silently became positional"
    );
    let legacy = include_str!("../../../evm-decompile-model/tests/fixtures/partial-artifact.json");
    let restored: Artifact =
        serde_json::from_str(legacy).expect("supported v1 with absent high payload");
    assert_eq!(restored.schema_version, 1);
    assert!(
        restored
            .stages
            .iter()
            .all(|stage| stage.ir.lifted.is_none())
    );
    let mut structured_wire = serde_json::to_value(&structured(&artifact).ir).unwrap();
    structured_wire.as_object_mut().unwrap().remove("lifted");
    let absent: evm_decompile::Ir = serde_json::from_value(structured_wire.clone()).unwrap();
    assert!(absent.lifted.is_none());
    structured_wire["lifted"] = serde_json::Value::Null;
    let explicit_null: evm_decompile::Ir = serde_json::from_value(structured_wire).unwrap();
    assert_eq!(absent, explicit_null);
}

#[test]
fn deep_shared_expression_arena_evaluates_serializes_and_renders_with_bounds() {
    use evm_decompile::LiftedExpression;
    let block = EntityId(99_999);
    let mut program = LiftedProgram::default();
    program.expressions.push(LiftedExpression {
        id: EntityId(1),
        block,
        ty: LiftedType::Word256,
        kind: Expr::Value(EntityId(999)),
    });
    for next in 2..=4096 {
        program.expressions.push(LiftedExpression {
            id: EntityId(next),
            block,
            ty: LiftedType::Word256,
            kind: Expr::WordOp {
                opcode: 0x01,
                arguments: vec![EntityId(next - 1); 2],
            },
        });
    }
    assert!(program.validate().is_ok());
    let wire = serde_json::to_vec(&program).unwrap();
    let restored: LiftedProgram = serde_json::from_slice(&wire).unwrap();
    assert_eq!(restored, program);
    assert_eq!(
        evaluate(
            &program,
            EntityId(4096),
            block,
            &BTreeMap::from([(EntityId(999), U256::from(1))])
        ),
        LiftedValue::Word(U256::ZERO)
    );
    let rendered = program.render_expression(EntityId(4096));
    assert!(rendered.len() <= evm_decompile::MAX_LIFTED_EXPRESSION_TEXT);
    assert!(
        rendered.contains("expression_limit"),
        "shared DAG must not expand exponentially"
    );
}

#[test]
fn lift_cancellation_and_resource_refusal_retain_exact_structured_transaction() {
    let input = comparison_branch(0x10, 3);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel();
        }
    });
    assert_eq!(before.completion, Completion::Cancelled);
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let retained = |artifact: &Artifact| {
        validated(artifact);
        assert_eq!(artifact.current().unwrap().kind, StageKind::Structured);
        assert_eq!(artifact.stages, before.stages);
        assert_eq!(artifact.events, before.events);
        assert_eq!(artifact.mappings, before.mappings);
        assert!(
            artifact
                .stages
                .iter()
                .all(|stage| stage.ir.lifted.is_none())
        );
    };
    for phase in [Phase::Validate, Phase::Provenance] {
        let cancel = Cancellation::new();
        let mut lifting = false;
        let artifact = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
            lifting |= progress.phase == Phase::Lift;
            if lifting && progress.phase == phase {
                cancel.cancel();
            }
        });
        assert_eq!(artifact.completion, Completion::Cancelled);
        retained(&artifact);
    }
    let mut config = Config::default();
    config.limits.max_work = before.statistics.work;
    let artifact = decompile(&input, &config);
    assert_eq!(artifact.completion, Completion::Limited);
    assert!(artifact.statistics.work <= config.limits.max_work);
    retained(&artifact);
    let mut config = Config::default();
    config.limits.max_history_entities = before.statistics.history_entities;
    let artifact = decompile(&input, &config);
    assert_eq!(artifact.completion, Completion::Limited);
    assert!(artifact.statistics.history_entities <= config.limits.max_history_entities);
    retained(&artifact);
}

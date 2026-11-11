//! Captured code is an input-bound premise that legacy snapshots cannot claim.
use evm_decompile_model::{
    Artifact, Block, Effects, EntityId, ExitKind, Ir, LiftedBlock, LiftedConstruction,
    LiftedErrorPayload, LiftedProgram, LiftedRevert, LiftedStatement, Operation, SCHEMA_VERSION,
    SourceSpan, StructuredKind, StructuredNode, StructuredProgram, Terminator,
};
use serde_json::json;

fn captured() -> Artifact {
    let mut artifact: Artifact =
        serde_json::from_str(include_str!("fixtures/partial-artifact.json")).unwrap();
    artifact.schema_version = SCHEMA_VERSION;
    for stage in &mut artifact.stages {
        stage.ir.runtime_bytecode = Some(artifact.input.runtime_bytecode.clone());
    }
    artifact
}

fn proof(code_hash: Option<alloy_primitives::B256>) -> LiftedRevert {
    LiftedRevert {
        execution_domain: Default::default(),
        execution_fork: None,
        code_hash,
        id: EntityId(100),
        block: EntityId(101),
        operation: EntityId(102),
        memory: EntityId(103),
        offset: EntityId(104),
        size: EntityId(105),
        payload: EntityId(106),
        writes: vec![EntityId(107)],
    }
}

#[test]
fn captured_runtime_roundtrips_and_must_match_the_artifact_input() {
    let artifact = captured();
    assert!(artifact.admits_schema());
    assert_eq!(
        serde_json::from_slice::<Artifact>(&serde_json::to_vec(&artifact).unwrap()).unwrap(),
        artifact
    );
    for change in ["bytes", "length", "input", "schema"] {
        let mut corrupt = artifact.clone();
        match change {
            "bytes" => {
                corrupt.stages[0].ir.runtime_bytecode.as_mut().unwrap()[0] ^= 1;
            }
            "length" => corrupt.stages[0].ir.bytecode_len += 1,
            "input" => corrupt.input.runtime_bytecode[0] ^= 1,
            "schema" => corrupt.schema_version = 9,
            _ => unreachable!(),
        }
        assert!(!corrupt.admits_schema(), "accepted {change}");
    }
    let mut old = artifact;
    for stage in &mut old.stages {
        stage.ir.runtime_bytecode = None;
    }
    old.schema_version = 9;
    assert!(old.admits_schema());
}

#[test]
fn code_hash_is_strict_optional_and_tied_to_a_captured_runtime() {
    let mut artifact = captured();
    let hash = alloy_primitives::keccak256(&artifact.input.runtime_bytecode);
    let certified = proof(Some(hash));
    let wire = serde_json::to_value(&certified).unwrap();
    assert_eq!(
        serde_json::from_value::<LiftedRevert>(wire.clone()).unwrap(),
        certified
    );
    for invalid in [json!("0x00"), json!(32), json!(vec![0; 31])] {
        let mut malformed = wire.clone();
        malformed["code_hash"] = invalid;
        assert!(serde_json::from_value::<LiftedRevert>(malformed).is_err());
    }
    let mut legacy = wire;
    legacy.as_object_mut().unwrap().remove("code_hash");
    assert_eq!(
        serde_json::from_value::<LiftedRevert>(legacy).unwrap(),
        proof(None)
    );
    artifact.stages.last_mut().unwrap().ir.lifted = Some(LiftedProgram {
        reverts: vec![certified],
        ..LiftedProgram::default()
    });
    assert!(artifact.admits_schema());
    for change in ["hash", "missing_source", "legacy_schema"] {
        let mut corrupt = artifact.clone();
        let stage = corrupt.stages.last_mut().unwrap();
        match change {
            "hash" => {
                stage.ir.lifted.as_mut().unwrap().reverts[0].code_hash =
                    Some(alloy_primitives::keccak256(b"different runtime"));
            }
            "missing_source" => stage.ir.runtime_bytecode = None,
            "legacy_schema" => corrupt.schema_version = 9,
            _ => unreachable!(),
        }
        assert!(!corrupt.admits_schema(), "accepted {change}");
    }
}

#[test]
fn runtime_witness_is_omitted_on_legacy_ir_and_requires_byte_values() {
    let wire = serde_json::to_value(Ir::default()).unwrap();
    assert!(wire.get("runtime_bytecode").is_none());
    for invalid in [json!("00"), json!([-1]), json!([256]), json!({})] {
        let mut malformed = wire.clone();
        malformed["runtime_bytecode"] = invalid;
        assert!(serde_json::from_value::<Ir>(malformed).is_err());
    }
}

fn source_capsule() -> (Ir, LiftedProgram) {
    let code = vec![0x39, 0xfd];
    let proof = proof(Some(alloy_primitives::keccak256(&code)));
    let copy = Operation {
        id: proof.writes[0],
        source: Some(SourceSpan { pc: 0, end_pc: 1 }),
        opcode: 0x39,
        name: "CODECOPY".into(),
        inputs: vec![proof.offset, EntityId(110), EntityId(111)],
        outputs: vec![],
        effects: Effects {
            memory_write: true,
            ..Effects::default()
        },
        memory_input: Some(EntityId(108)),
        memory_output: Some(proof.memory),
        replacement: None,
        effect_input: Some(EntityId(109)),
        effect_output: Some(EntityId(112)),
    };
    let revert = Operation {
        id: proof.operation,
        source: Some(SourceSpan { pc: 1, end_pc: 2 }),
        opcode: 0xfd,
        name: "REVERT".into(),
        inputs: vec![proof.offset, proof.size],
        outputs: vec![],
        effects: Effects {
            memory_read: true,
            terminate: true,
            ..Effects::default()
        },
        memory_input: Some(proof.memory),
        memory_output: None,
        replacement: None,
        effect_input: Some(EntityId(112)),
        effect_output: Some(EntityId(113)),
    };
    let ir = Ir {
        bytecode_len: code.len(),
        runtime_bytecode: Some(code),
        blocks: vec![Block {
            id: proof.block,
            source: None,
            parameters: vec![],
            memory_parameter: EntityId(108),
            effect_parameter: EntityId(109),
            operations: vec![copy, revert],
            terminator: Terminator::Exit {
                kind: ExitKind::Revert,
                values: vec![proof.offset, proof.size],
            },
        }],
        structured: Some(StructuredProgram {
            root: StructuredNode {
                id: EntityId(210),
                kind: StructuredKind::Sequence(vec![
                    StructuredNode {
                        id: EntityId(211),
                        kind: StructuredKind::Block { block: proof.block },
                    },
                    StructuredNode {
                        id: EntityId(212),
                        kind: StructuredKind::Exit { block: proof.block },
                    },
                ]),
            },
        }),
        ..Ir::default()
    };
    let mut bytes = vec![0; 100];
    bytes[..4].copy_from_slice(&[8, 0xc3, 0x79, 0xa0]);
    bytes[35] = 32;
    bytes[67] = 1;
    bytes[68] = b'x';
    let program = LiftedProgram {
        blocks: vec![LiftedBlock {
            id: EntityId(200),
            block: proof.block,
            parameters: vec![],
            statements: [proof.writes[0], proof.operation]
                .into_iter()
                .enumerate()
                .map(|(i, operation)| LiftedStatement {
                    id: EntityId(201 + i as u64),
                    operation,
                    inputs: vec![],
                    outputs: vec![],
                })
                .collect(),
            condition: None,
            target: None,
            exit_values: vec![],
        }],
        payloads: vec![LiftedErrorPayload {
            id: proof.payload,
            message: "x".into(),
            bytes,
        }],
        constructions: vec![LiftedConstruction {
            kind: evm_decompile_model::LiftedConstructionKind::Constructor,
            id: EntityId(203),
            block: proof.block,
            start: 0,
            end: 2,
            revert: proof.id,
        }],
        reverts: vec![proof],
        ..LiftedProgram::default()
    };
    (ir, program)
}

#[test]
fn copy_capsule_source_admission_requires_the_captured_code_and_original_effects() {
    // This checks source ownership only. Core separately replays actual bytes.
    let (ir, program) = source_capsule();
    program.validate_guard_sources(&ir).unwrap();
    for change in [
        "missing_hash",
        "wrong_hash",
        "missing_source",
        "wrong_source",
        "wrong_length",
        "missing_copy_source",
        "wrong_copy_source",
        "wrong_copy_width",
        "changed_copy_opcode",
        "wrong_arity",
        "missing_memory",
        "extra_output",
        "extra_effect",
        "external_code",
    ] {
        let mut ir = ir.clone();
        let mut program = program.clone();
        match change {
            "missing_hash" => program.reverts[0].code_hash = None,
            "wrong_hash" => {
                program.reverts[0].code_hash = Some(alloy_primitives::keccak256(b"foreign"));
            }
            "missing_source" => ir.runtime_bytecode = None,
            "wrong_source" => ir.runtime_bytecode.as_mut().unwrap()[0] ^= 1,
            "wrong_length" => ir.bytecode_len += 1,
            "missing_copy_source" => ir.blocks[0].operations[0].source = None,
            "wrong_copy_source" => {
                ir.blocks[0].operations[0].source = Some(SourceSpan { pc: 1, end_pc: 2 });
            }
            "wrong_copy_width" => {
                ir.blocks[0].operations[0].source = Some(SourceSpan { pc: 0, end_pc: 2 });
            }
            "changed_copy_opcode" => {
                ir.runtime_bytecode.as_mut().unwrap()[0] = 0x3c;
                program.reverts[0].code_hash = Some(alloy_primitives::keccak256(
                    ir.runtime_bytecode.as_ref().unwrap(),
                ));
            }
            "wrong_arity" => {
                ir.blocks[0].operations[0].inputs.pop().unwrap();
            }
            "missing_memory" => ir.blocks[0].operations[0].memory_output = None,
            "extra_output" => ir.blocks[0].operations[0].outputs.push(EntityId(300)),
            "extra_effect" => ir.blocks[0].operations[0].effects.storage_read = true,
            "external_code" => ir.blocks[0].operations[0].opcode = 0x3c,
            _ => unreachable!(),
        }
        assert!(
            program.validate_guard_sources(&ir).is_err(),
            "accepted {change}"
        );
    }
}

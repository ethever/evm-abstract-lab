//! Presentation fixtures. Browser cases separately exercise real decompilation.

use evm_abstract_protocol::{
    self as api,
    decompile_model::{
        Artifact, ArtifactId, Block, CloneRelation, Completion, Config, Edge, EdgeKind,
        EdgeRedirect, Effects, EntityId, EntityRef, ExitKind, FactScope, Fork, Input, Ir, Limits,
        Operation, Origin, Progress, Record, SCHEMA_VERSION, SourceMappings, SourceSpan, Stage,
        StageId, StageKind, Statistics, Strategy, StructuredKind, StructuredNode,
        StructuredProgram, TransformEvent, U256, Validation, ValidationCheck, ValidationStatus,
        Value, ValueFact, ValueKind, records,
    },
};

pub(super) fn passed() -> Validation {
    Validation {
        status: ValidationStatus::Passed,
        checks: vec![ValidationCheck {
            name: "fixture identity".into(),
            passed: true,
            message: "Presentation fixture".into(),
        }],
    }
}

pub(super) fn artifact(strategy: Strategy) -> Artifact {
    let op = Operation {
        id: EntityId(2),
        source: Some(SourceSpan { pc: 0, end_pc: 2 }),
        opcode: 0x60,
        name: "PUSH1".into(),
        inputs: vec![],
        outputs: vec![EntityId(3)],
        effects: Effects::default(),
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    };
    let store = Operation {
        id: EntityId(4),
        source: Some(SourceSpan { pc: 2, end_pc: 3 }),
        opcode: 0x55,
        name: "SSTORE".into(),
        inputs: vec![EntityId(3), EntityId(3)],
        outputs: vec![],
        effects: Effects {
            storage_write: true,
            ..Effects::default()
        },
        memory_input: Some(EntityId(5)),
        memory_output: Some(EntityId(6)),
        replacement: None,
        effect_input: Some(EntityId(12)),
        effect_output: Some(EntityId(13)),
    };
    let first = Block {
        id: EntityId(1),
        source: Some(SourceSpan { pc: 0, end_pc: 3 }),
        parameters: vec![],
        memory_parameter: EntityId(5),
        effect_parameter: EntityId(12),
        operations: vec![op, store],
        terminator: Terminator::Fallthrough { edge: EntityId(9) },
    };
    let last = Block {
        id: EntityId(8),
        source: Some(SourceSpan { pc: 3, end_pc: 4 }),
        parameters: vec![],
        memory_parameter: EntityId(10),
        effect_parameter: EntityId(11),
        operations: vec![],
        terminator: Terminator::Exit {
            kind: ExitKind::Stop,
            values: vec![],
        },
    };
    let baseline = Stage {
        id: StageId(0),
        parent: None,
        kind: StageKind::Baseline,
        validation: passed(),
        ir: Ir {
            bytecode_len: 4,
            fact_scope: FactScope::CompleteLocalCfg,
            entry: Some(first.id),
            blocks: vec![first, last],
            values: vec![
                Value {
                    id: EntityId(3),
                    kind: ValueKind::Constant(U256::from(1)),
                    fact: ValueFact::Constants(vec![U256::from(1)]),
                },
                Value {
                    id: EntityId(5),
                    kind: ValueKind::Memory { operation: None },
                    fact: ValueFact::Unknown,
                },
                Value {
                    id: EntityId(6),
                    kind: ValueKind::Memory {
                        operation: Some(EntityId(4)),
                    },
                    fact: ValueFact::Unknown,
                },
                Value {
                    id: EntityId(11),
                    kind: ValueKind::EffectParameter { block: EntityId(8) },
                    fact: ValueFact::Unknown,
                },
                Value {
                    id: EntityId(12),
                    kind: ValueKind::Effect { operation: None },
                    fact: ValueFact::Unknown,
                },
                Value {
                    id: EntityId(13),
                    kind: ValueKind::Effect {
                        operation: Some(EntityId(4)),
                    },
                    fact: ValueFact::Unknown,
                },
                Value {
                    id: EntityId(10),
                    kind: ValueKind::MemoryParameter { block: EntityId(8) },
                    fact: ValueFact::Unknown,
                },
            ],
            edges: vec![Edge {
                id: EntityId(9),
                from: EntityId(1),
                to: EntityId(8),
                kind: EdgeKind::Fallthrough,
                arguments: vec![],
                memory: EntityId(6),
                effect: EntityId(13),
            }],
            structured: None,
        },
    };
    let mut current = baseline.clone();
    current.id = StageId(1);
    current.parent = Some(baseline.id);
    current.kind = StageKind::Structured;
    current.ir.blocks[0].operations.remove(0);
    let mut copy = current.ir.blocks[1].clone();
    copy.id = EntityId(18);
    copy.memory_parameter = EntityId(20);
    copy.effect_parameter = EntityId(21);
    current.ir.blocks.push(copy);
    current.ir.values.push(Value {
        id: EntityId(20),
        kind: ValueKind::MemoryParameter {
            block: EntityId(18),
        },
        fact: ValueFact::Unknown,
    });
    current.ir.values.push(Value {
        id: EntityId(21),
        kind: ValueKind::EffectParameter {
            block: EntityId(18),
        },
        fact: ValueFact::Unknown,
    });
    current.ir.edges[0].to = EntityId(18);
    current.ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(30),
            kind: StructuredKind::Sequence(vec![
                StructuredNode {
                    id: EntityId(31),
                    kind: StructuredKind::Block { block: EntityId(1) },
                },
                StructuredNode {
                    id: EntityId(32),
                    kind: StructuredKind::Exit {
                        block: EntityId(18),
                    },
                },
            ]),
        },
    });
    let stages = vec![baseline, current];
    let mut origins = Vec::new();
    for stage in &stages {
        for (entity, record) in records(&stage.ir) {
            let source = match record {
                Record::Block(block) => block.source.into_iter().collect(),
                Record::Operation(operation) => operation.source.into_iter().collect(),
                _ => vec![],
            };
            let original = match entity.0 {
                18 => Some(EntityId(8)),
                20 => Some(EntityId(10)),
                21 => Some(EntityId(11)),
                _ => stages[0].ir.block(entity).map(|block| block.id),
            };
            let parents = if stage.id == StageId(1) {
                original
                    .map(|entity| EntityRef {
                        stage: StageId(0),
                        entity,
                    })
                    .into_iter()
                    .collect()
            } else {
                vec![]
            };
            origins.push(Origin {
                entity: EntityRef {
                    stage: stage.id,
                    entity,
                },
                source,
                parents,
                introduced_by: u64::from(stage.id.0),
                reason: if matches!(entity.0, 18 | 20 | 21) {
                    "copied entity".into()
                } else {
                    "fixture construction".into()
                },
            });
        }
    }
    let events = stages
        .iter()
        .map(|stage| TransformEvent {
            id: u64::from(stage.id.0),
            from_stage: stage.parent,
            to_stage: stage.id,
            rule: if stage.id == StageId(0) {
                "baseline".into()
            } else {
                "split_and_structure".into()
            },
            reason: "Presentation fixture with explicit copy and edge redirection".into(),
            inputs: if stage.id == StageId(1) {
                records(&stages[0].ir)
                    .keys()
                    .map(|entity| EntityRef {
                        stage: StageId(0),
                        entity: *entity,
                    })
                    .collect()
            } else {
                vec![]
            },
            outputs: records(&stage.ir)
                .keys()
                .map(|entity| EntityRef {
                    stage: stage.id,
                    entity: *entity,
                })
                .collect(),
            created: if stage.id == StageId(0) {
                records(&stage.ir).keys().copied().collect()
            } else {
                vec![
                    EntityId(18),
                    EntityId(20),
                    EntityId(21),
                    EntityId(30),
                    EntityId(31),
                    EntityId(32),
                ]
            },
            deleted: if stage.id == StageId(1) {
                vec![EntityId(2)]
            } else {
                vec![]
            },
            clones: if stage.id == StageId(1) {
                vec![
                    CloneRelation {
                        original: EntityRef {
                            stage: StageId(0),
                            entity: EntityId(8),
                        },
                        copy: EntityRef {
                            stage: StageId(1),
                            entity: EntityId(18),
                        },
                    },
                    CloneRelation {
                        original: EntityRef {
                            stage: StageId(0),
                            entity: EntityId(10),
                        },
                        copy: EntityRef {
                            stage: StageId(1),
                            entity: EntityId(20),
                        },
                    },
                    CloneRelation {
                        original: EntityRef {
                            stage: StageId(0),
                            entity: EntityId(11),
                        },
                        copy: EntityRef {
                            stage: StageId(1),
                            entity: EntityId(21),
                        },
                    },
                ]
            } else {
                vec![]
            },
            redirects: if stage.id == StageId(1) {
                vec![EdgeRedirect {
                    edge: EntityId(9),
                    before: EntityId(8),
                    after: EntityId(18),
                }]
            } else {
                vec![]
            },
            validation: passed(),
        })
        .collect();
    Artifact {
        schema_version: SCHEMA_VERSION,
        id: ArtifactId(format!("fixture-{strategy:?}")),
        input: Input::new(vec![0x60, 0x01, 0x55, 0x00], Fork::Osaka),
        config: Config {
            strategy,
            limits: Limits::default(),
        },
        completion: Completion::Complete,
        diagnostics: vec![],
        stages,
        events,
        mappings: SourceMappings { origins },
        current_stage: Some(StageId(1)),
        statistics: Statistics::default(),
    }
}

pub(super) fn version(value: u64) -> api::PublicationVersion {
    api::PublicationVersion(api::DecimalU64(value))
}

pub(super) fn snapshot(
    id: u64,
    state: api::JobState,
    publication: Option<api::PublicationVersion>,
) -> api::DecompileJobReply {
    api::DecompileJobReply {
        result: Ok(api::DecompileJobSnapshot {
            id: api::JobId(id),
            state,
            progress: Progress::default(),
            queue_ns: api::DecimalU64(0),
            processing_ns: api::DecimalU64(0),
            publication,
        }),
    }
}

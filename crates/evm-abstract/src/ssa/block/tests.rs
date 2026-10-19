use crate::{
    analysis::{InstructionProgress, MachineEdgeKind},
    ssa::{
        EffectInput, EffectPhi, FramePhi, Instruction, NormalizedInstruction, NormalizedOperation,
        PartialBlockCoverage, PartialInstruction, PartialWorldBlock, Transition, ValueOrigin,
    },
};

fn block() -> PartialWorldBlock {
    PartialWorldBlock {
        state: 91,
        coverage: PartialBlockCoverage::Current,
        phis: vec![FramePhi {
            frame: 1,
            slot: 0,
            result: 1,
            inputs: vec![(73, 2), (74, 3)],
        }],
        instructions: vec![PartialInstruction {
            instruction: Instruction {
                pc: 12,
                opcode: 1,
                immediate: None,
                operands: vec![2, 3],
                results: vec![4],
                fault: false,
            },
            progress: InstructionProgress::Completed,
            effect_input: 2,
            effect_result: Some(3),
        }],
        exit_frames: vec![vec![4], vec![1, 4]],
        effect: EffectPhi {
            result: 1,
            inputs: vec![EffectInput {
                transition: 73,
                effect: 4,
            }],
        },
        exit_effect: 3,
        open_incoming: vec![75],
        incoming_complete: false,
    }
}

#[test]
fn mapping_preserves_the_envelope_and_both_public_json_layouts() {
    let source = block();
    let mapped = source
        .try_map_instructions(|index, step| {
            assert_eq!(index, 0);
            Ok::<_, ()>(NormalizedInstruction {
                origin: step.instruction.clone(),
                progress: step.progress,
                effect_input: step.effect_input,
                effect_result: step.effect_result,
                operation: NormalizedOperation::Opcode {
                    opcode: step.instruction.opcode,
                    operands: Vec::new(),
                },
            })
        })
        .unwrap();
    let prefix = concat!(
        r#"{"state":91,"coverage":"Current","phis":[{"frame":1,"slot":0,"result":1,"inputs":[[73,2],[74,3]]}],"instructions":["#,
        r#"{"pc":12,"opcode":1,"immediate":null,"operands":[2,3],"results":[4],"fault":false,"progress":"Completed","effect_input":2,"effect_result":3"#,
    );
    let suffix = r#"],"exit_frames":[[4],[1,4]],"effect":{"result":1,"inputs":[{"transition":73,"effect":4}]},"exit_effect":3,"open_incoming":[75],"incoming_complete":false}"#;
    assert_eq!(
        serde_json::to_string(&source).unwrap(),
        format!("{prefix}}}{suffix}")
    );
    assert_eq!(
        serde_json::to_string(&mapped).unwrap(),
        format!("{prefix},\"operation\":{{\"Opcode\":{{\"opcode\":1,\"operands\":[]}}}}}}{suffix}")
    );
    let restored = mapped
        .try_map_instructions(|_, step| {
            Ok::<_, ()>(PartialInstruction {
                instruction: step.origin.clone(),
                progress: step.progress,
                effect_input: step.effect_input,
                effect_result: step.effect_result,
            })
        })
        .unwrap();
    assert_eq!(
        serde_json::to_string(&restored).unwrap(),
        serde_json::to_string(&source).unwrap()
    );
}

#[test]
fn value_traversal_renames_all_definitions_and_uses_but_no_other_namespace() {
    let mut block = block();
    let names = [9, 8, 7, 6, 5];
    for value in block.frame_values_mut() {
        *value = names[*value];
    }
    for step in &mut block.instructions {
        for value in step.instruction.values_mut() {
            *value = names[*value];
        }
    }
    assert_eq!(block.phis[0].result, 8);
    assert_eq!(block.phis[0].inputs, [(73, 7), (74, 6)]);
    assert_eq!(block.exit_frames, [vec![5], vec![8, 5]]);
    let step = &block.instructions[0];
    assert_eq!(step.instruction.operands, [7, 6]);
    assert_eq!(step.instruction.results, [5]);
    assert_eq!(
        block
            .definitions(|step| &step.instruction)
            .collect::<Vec<_>>(),
        [
            (
                8,
                ValueOrigin::Phi {
                    state: 91,
                    frame: 1,
                    slot: 0
                }
            ),
            (
                5,
                ValueOrigin::Instruction {
                    state: 91,
                    index: 0,
                    pc: 12,
                    opcode: 1
                }
            ),
        ]
    );
    assert_eq!(block.state, 91);
    assert_eq!(block.phis[0].frame, 1);
    assert_eq!(block.phis[0].slot, 0);
    assert_eq!(block.open_incoming, [75]);
    assert_eq!(block.effect.result, 1);
    assert_eq!(block.effect.inputs[0].transition, 73);
    assert_eq!(block.effect.inputs[0].effect, 4);
    assert_eq!(block.exit_effect, 3);
    assert_eq!(step.instruction.pc, 12);
    assert_eq!(step.instruction.opcode, 1);
    assert_eq!(step.effect_input, 2);
    assert_eq!(step.effect_result, Some(3));

    let mut transition = Transition {
        edge: 74,
        kind: MachineEdgeKind::Return,
        operands: vec![2, 3],
        stacks: vec![vec![1, 4], vec![]],
        effect_input: 2,
        effect_result: 3,
        result: Some(4),
    };
    for value in transition.values_mut() {
        *value = names[*value];
    }
    assert_eq!(transition.operands, [7, 6]);
    assert_eq!(transition.stacks, [vec![8, 5], vec![]]);
    assert_eq!(transition.result, Some(5));
    assert_eq!(transition.edge, 74);
    assert_eq!(transition.kind, MachineEdgeKind::Return);
    assert_eq!(transition.effect_input, 2);
    assert_eq!(transition.effect_result, 3);
}

#[test]
fn instruction_mapping_stops_at_the_first_failure_without_changing_the_source() {
    let mut source = block();
    source.instructions = vec![source.instructions[0].clone(); 3];
    let before = serde_json::to_string(&source).unwrap();
    let mut visited = Vec::new();
    let mapped = source.try_map_instructions(|index, _| {
        visited.push(index);
        if index == 1 { Err(index) } else { Ok(index) }
    });
    assert_eq!(mapped.unwrap_err(), 1);
    assert_eq!(visited, [0, 1]);
    assert_eq!(serde_json::to_string(&source).unwrap(), before);
}

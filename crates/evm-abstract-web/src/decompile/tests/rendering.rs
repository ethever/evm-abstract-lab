use super::{Strategy, fixture};
use crate::decompile::structured;
use evm_abstract_protocol::decompile_model::{
    EntityId, IndirectArm, StructuredKind, StructuredNode, StructuredProgram, Terminator, U256,
};

fn text(stage: &evm_abstract_protocol::decompile_model::Stage) -> String {
    structured::lines(stage)
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn indirect_alternatives_preserve_exception_partial_and_conditional_false_continuations() {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    let stage = &mut artifact.stages[1];
    for (unresolved, invalid_targets) in [
        (false, vec![U256::from(1)]),
        (false, vec![]),
        (true, vec![]),
    ] {
        stage.ir.blocks[0].terminator = Terminator::Jump {
            target: EntityId(3),
            edges: vec![EntityId(9)],
            unresolved,
            invalid_targets,
        };
        stage.ir.structured = Some(StructuredProgram {
            root: StructuredNode {
                id: EntityId(40),
                kind: StructuredKind::If {
                    condition: EntityId(3),
                    then_branch: Box::new(StructuredNode {
                        id: EntityId(41),
                        kind: StructuredKind::Indirect {
                            block: EntityId(1),
                            unresolved,
                            arms: vec![IndirectArm {
                                target_pc: 3,
                                body: StructuredNode {
                                    id: EntityId(42),
                                    kind: StructuredKind::Exit {
                                        block: EntityId(18),
                                    },
                                },
                            }],
                        },
                    }),
                    else_branch: Box::new(StructuredNode {
                        id: EntityId(43),
                        kind: StructuredKind::Exit { block: EntityId(8) },
                    }),
                },
            },
        });
        let output = text(stage);
        assert!(output.contains("target 0x3:"), "{output}");
        assert!(output.contains("} else {\nevm.stop();"), "{output}");
        if unresolved {
            assert!(output.contains("unresolved_target: partial_exit"));
        } else {
            assert!(output.contains("otherwise: exception(\"invalid jump destination\")"));
        }
        let lines = structured::lines(stage);
        assert!(lines.iter().any(|line| line.entity == EntityId(41)
            && line.text.contains(if unresolved {
                "partial_exit"
            } else {
                "invalid jump"
            })));
    }
}

#[test]
fn faulting_final_op_is_a_source_attributed_fault_and_jumpi_falloff_is_explicit_stop() {
    let mut artifact = fixture::artifact(Strategy::Dispatcher);
    let stage = &mut artifact.stages[1];
    stage.ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(40),
            kind: StructuredKind::Sequence(vec![
                StructuredNode {
                    id: EntityId(41),
                    kind: StructuredKind::Block { block: EntityId(1) },
                },
                StructuredNode {
                    id: EntityId(42),
                    kind: StructuredKind::Exit { block: EntityId(1) },
                },
            ]),
        },
    });
    let original = stage.ir.blocks[0].operations[0].clone();
    for (opcode, name, reason) in [
        (0x01, "ADD", "stack underflow"),
        (0x55, "SSTORE", "stack underflow"),
        (0x60, "PUSH1", "stack overflow"),
    ] {
        let mut fault = original.clone();
        fault.id = EntityId(50);
        fault.opcode = opcode;
        fault.name = name.into();
        fault.inputs.clear();
        fault.outputs = vec![EntityId(51)];
        stage.ir.blocks[0].operations = vec![original.clone(), fault];
        stage.ir.blocks[0].terminator = Terminator::Trap {
            reason: reason.into(),
        };
        let output = text(stage);
        assert_eq!(output.matches("evm.sstore(").count(), 1, "{output}");
        assert!(!output.contains("v51 ="), "{output}");
        assert!(output.contains(&format!("fault o50 {name} at source 0x0002: {reason}")));
        assert!(
            structured::lines(stage)
                .iter()
                .any(|line| line.entity == EntityId(50) && line.text.contains("fault"))
        );
    }
    stage.ir.blocks[0].operations.clear();
    stage.ir.blocks[0].terminator = Terminator::Branch {
        condition: EntityId(3),
        target: EntityId(3),
        then_edges: vec![EntityId(9)],
        else_edge: None,
        unresolved: false,
        invalid_targets: vec![],
    };
    let output = text(stage);
    assert_eq!(output.matches("evm.stop()").count(), 1, "{output}");
    assert!(output.contains("JUMPI false continuation: end of code"));
    assert!(!output.contains("branch "));
    assert!(structured::terminator(&stage.ir.blocks[0].terminator).contains("branch "));
    assert!(
        structured::terminator(&stage.ir.blocks[0].terminator)
            .contains("else evm.stop() (end of code)")
    );
}

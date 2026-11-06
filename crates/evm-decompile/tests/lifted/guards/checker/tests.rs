//! Distinguish the specified certificate domain from raw numeric facts and
//! concrete termination; these regressions must fail under the former oracle.
use super::*;
use evm_decompile::{IndirectArm, LiftedExpressionKind, ValueFact};

#[test]
fn typed_zero_seed_with_unknown_raw_size_keeps_independent_closure_and_execution() {
    for opcode in [0x18, 0x03] {
        let mut asm = Asm::default();
        // Capture x before the branch; failure itself performs no world reads.
        asm.push(32)
            .op(0x35)
            .op(0x5f)
            .op(0x35)
            .target("pass")
            .op(0x57)
            .op(0x80)
            .op(opcode)
            .op(0x5f)
            .op(0xfd)
            .dest("pass")
            .push(0x55)
            .op(0x5f)
            .op(0x55)
            .op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let (stage, program) = lifted(&artifact);
        assert_eq!(guards(&artifact), 1);
        let failure = stage
            .ir
            .blocks
            .iter()
            .find(|block| {
                matches!(
                    block.terminator,
                    Terminator::Exit {
                        kind: ExitKind::Revert,
                        ..
                    }
                )
            })
            .unwrap();
        let Terminator::Exit { values, .. } = &failure.terminator else {
            unreachable!()
        };
        assert_eq!(
            stage.ir.value(values[1]).unwrap().fact,
            ValueFact::Unknown,
            "fixture must distinguish the typed proof from raw fact propagation"
        );
        let size = program.block(failure.id).unwrap().exit_values[1];
        assert_eq!(
            program.expression(size).unwrap().kind,
            LiftedExpressionKind::Word(U256::ZERO)
        );
        assert!(empty_seed(stage, failure.id));
        assert!(failure_set(stage).contains(&failure.id));
        certificate(stage);
        for pass in [U256::ZERO, U256::from(1)] {
            for x in [U256::ZERO, U256::from(1), U256::from(1) << 255, U256::MAX] {
                let (raw, host) = certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[pass, x])),
                );
                assert_eq!(raw.trace.iter().filter(|op| op.opcode == 0x35).count(), 2);
                if pass == U256::ZERO {
                    assert_eq!(reverted(&raw, &host), Some(Vec::new()));
                    assert!(host.storage.is_empty());
                } else {
                    assert_eq!(host.storage[&U256::ZERO], U256::from(0x55));
                }
            }
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn typed_zero_does_not_replace_the_terminal_witness_or_transparent_opcode_premises() {
    let artifact = checked(&fixtures::direct(
        false,
        fixtures::Failure::Empty(U256::ZERO),
    ));
    let (stage, program) = lifted(&artifact);
    let seed = program
        .failures
        .iter()
        .find(|fact| fact.rank == 0)
        .unwrap()
        .block;
    assert!(empty_seed(stage, seed));
    for invalid in [
        "typed_nonzero",
        "terminal_opcode",
        "terminal_inputs",
        "earlier_revert",
        "unsupported_pure",
    ] {
        let mut wrong = stage.clone();
        if invalid == "typed_nonzero" {
            let program = wrong.ir.lifted.as_mut().unwrap();
            let size = program.block(seed).unwrap().exit_values[1];
            program
                .expressions
                .iter_mut()
                .find(|expression| expression.id == size)
                .unwrap()
                .kind = LiftedExpressionKind::Word(U256::from(1));
            let Terminator::Exit { values, .. } = &wrong.ir.block(seed).unwrap().terminator else {
                unreachable!()
            };
            assert_eq!(
                wrong.ir.value(values[1]).unwrap().fact.singleton(),
                Some(U256::ZERO),
                "raw zero alone must not admit a nonzero typed size"
            );
        } else {
            let block = wrong
                .ir
                .blocks
                .iter_mut()
                .find(|block| block.id == seed)
                .unwrap();
            match invalid {
                "terminal_opcode" => block.operations.last_mut().unwrap().opcode = 0xf3,
                "terminal_inputs" => block.operations.last_mut().unwrap().inputs.clear(),
                "earlier_revert" => {
                    let earlier = block.operations.last().unwrap().clone();
                    block.operations.insert(block.operations.len() - 1, earlier);
                }
                "unsupported_pure" => {
                    let mut earlier = block
                        .operations
                        .iter()
                        .find(|op| op.effects.is_pure())
                        .unwrap()
                        .clone();
                    earlier.opcode = 0x0c;
                    assert!(earlier.effects.is_pure());
                    block.operations.insert(block.operations.len() - 1, earlier);
                }
                _ => unreachable!(),
            }
        }
        assert!(!empty_seed(&wrong, seed), "{invalid}");
        assert!(!failure_set(&wrong).contains(&seed), "{invalid}");
    }
}

#[test]
fn unresolved_or_empty_indirect_and_partial_source_escapes_keep_following_suffixes() {
    let artifact = checked(&fixtures::direct(
        false,
        fixtures::Failure::Empty(U256::ZERO),
    ));
    let (stage, _) = lifted(&artifact);
    let terminal = stage
        .ir
        .blocks
        .iter()
        .find(|block| {
            matches!(
                block.terminator,
                Terminator::Exit {
                    kind: ExitKind::Stop,
                    ..
                }
            )
        })
        .unwrap()
        .id;
    let node = |id, kind| StructuredNode {
        id: EntityId(id),
        kind,
    };
    let closed = node(1, StructuredKind::Exit { block: terminal });
    assert!(!falls_through(stage, &closed));
    for (unresolved, open, empty, expected) in [
        (false, false, false, false),
        (true, false, false, true),
        (false, false, true, true),
        (true, false, true, true),
        (false, true, false, true),
        (true, true, false, true),
    ] {
        let arms = if empty {
            Vec::new()
        } else {
            vec![IndirectArm {
                target_pc: 0,
                body: if open {
                    node(2, StructuredKind::Sequence(Vec::new()))
                } else {
                    closed.clone()
                },
            }]
        };
        let indirect = node(
            3,
            StructuredKind::Indirect {
                block: stage.ir.entry.unwrap(),
                arms,
                unresolved,
            },
        );
        assert_eq!(
            falls_through(stage, &indirect),
            expected,
            "an unknown alternative cannot be closed by a terminal known arm"
        );
        // The same escape nested in an If must not license deleting its
        // containing Sequence's following failure tail.
        let branch = node(
            4,
            StructuredKind::If {
                condition: EntityId(5),
                then_branch: Box::new(indirect),
                else_branch: Box::new(closed.clone()),
            },
        );
        assert_eq!(falls_through(stage, &branch), expected);
    }
    assert!(
        falls_through(
            stage,
            &node(6, StructuredKind::PartialExit { block: terminal })
        ),
        "PartialExit is conservatively open even if a referenced block is terminal"
    );
    assert!(
        falls_through(
            stage,
            &node(
                7,
                StructuredKind::Exit {
                    block: stage.ir.entry.unwrap()
                }
            )
        ),
        "source Exit closes only an actual raw terminal, not a branch/pending/unknown escape"
    );
}

//! Terminal classification uses retained decoded instruction provenance.
use evm_bytecode::{Fork, InvalidOpcodeReason, Program};
use evm_decompile_model::{
    Block, Effects, EntityId, ExitKind, Operation, SourceSpan, Terminator, U256,
    render_explicit_invalid,
};

fn decoded_fault(byte: u8) -> Block {
    decoded_fault_with_fork(byte, Fork::Osaka)
}

fn decoded_fault_with_fork(byte: u8, fork: Fork) -> Block {
    let program = Program::decode_with_fork(&[byte], fork).unwrap();
    let instruction = &program.blocks()[0].instructions[0];
    Block {
        id: EntityId(1),
        source: Some(SourceSpan {
            pc: instruction.pc,
            end_pc: instruction.pc + 1,
        }),
        parameters: vec![],
        memory_parameter: EntityId(2),
        effect_parameter: EntityId(3),
        operations: vec![Operation {
            id: EntityId(4),
            source: Some(SourceSpan {
                pc: instruction.pc,
                end_pc: instruction.pc + 1,
            }),
            opcode: instruction.opcode,
            name: instruction.name().into(),
            inputs: vec![],
            outputs: vec![],
            effects: Effects {
                terminate: true,
                ..Effects::default()
            },
            memory_input: None,
            memory_output: None,
            replacement: None,
            effect_input: None,
            effect_output: None,
        }],
        terminator: Terminator::Trap {
            reason: instruction
                .invalid_reason()
                .map_or_else(|| "EVM stack underflow".into(), |reason| reason.to_string()),
        },
    }
}

#[test]
fn actual_invalid_has_the_shared_terminal_spelling_and_keeps_source_identity() {
    let block = decoded_fault(0xfe);
    let original = block.clone();
    assert_eq!(render_explicit_invalid(&block), Some("evm.invalid();"));
    assert_eq!(block, original);
    // Names are display data; the original opcode/source/fault are authoritative.
    let mut renamed = block;
    renamed.operations[0].name = "different display label".into();
    assert_eq!(render_explicit_invalid(&renamed), Some("evm.invalid();"));
}

#[test]
fn other_opcodes_and_generic_faults_never_become_invalid() {
    // Real decoded undefined/fork-unavailable faults retain their own reason.
    for (byte, fork, expected) in [
        (0xf6, Fork::Osaka, InvalidOpcodeReason::UndefinedOpcode),
        (
            0x1e,
            Fork::Cancun,
            InvalidOpcodeReason::UnavailableInFork(Fork::Cancun),
        ),
    ] {
        let program = Program::decode_with_fork(&[byte], fork).unwrap();
        assert_eq!(
            program.blocks()[0].instructions[0].invalid_reason(),
            Some(expected)
        );
        assert_eq!(
            render_explicit_invalid(&decoded_fault_with_fork(byte, fork)),
            None
        );
    }
    for byte in [0x01, 0x56, 0x90, 0xf6, 0x5b] {
        let mut block = decoded_fault(byte);
        for reason in [
            "EVM stack underflow",
            "EVM stack overflow",
            "invalid jump destination",
            "undefined ordinary-bytecode opcode",
            "instruction unavailable in fork berlin",
            "explicit exceptional-halt instruction",
            "INVALID opcode",
        ] {
            block.terminator = Terminator::Trap {
                reason: reason.into(),
            };
            assert_eq!(
                render_explicit_invalid(&block),
                None,
                "byte={byte:#x} reason={reason}"
            );
        }
    }
}

#[test]
fn contradictory_or_missing_terminal_witnesses_keep_generic_diagnostics() {
    let block = decoded_fault(0xfe);
    for mutation in 0..14 {
        let mut modified = block.clone();
        match mutation {
            0 => modified.operations.clear(),
            1 => modified.source = None,
            2 => modified.operations[0].source = None,
            3 => modified.operations[0].source.as_mut().unwrap().end_pc = 2,
            4 => modified.operations[0].source.as_mut().unwrap().pc = 1,
            5 => modified.operations[0].inputs.push(EntityId(8)),
            6 => modified.operations[0].outputs.push(EntityId(8)),
            7 => modified.operations[0].replacement = Some(U256::ZERO),
            8 => modified.operations[0].effects.terminate = false,
            9 => {
                modified.terminator = Terminator::Trap {
                    reason: "EVM stack underflow".into(),
                }
            }
            10 => modified.terminator = Terminator::Pending,
            11 => {
                modified.terminator = Terminator::Exit {
                    kind: ExitKind::Stop,
                    values: vec![],
                }
            }
            12 => modified.source.as_mut().unwrap().end_pc = 0,
            13 => {
                let mut following = modified.operations[0].clone();
                following.opcode = 0x00;
                modified.operations.push(following);
            }
            _ => unreachable!(),
        }
        assert_eq!(
            render_explicit_invalid(&modified),
            None,
            "mutation={mutation}"
        );
    }
    assert_eq!(
        InvalidOpcodeReason::ExplicitInvalid.to_string(),
        "explicit exceptional-halt instruction"
    );
}

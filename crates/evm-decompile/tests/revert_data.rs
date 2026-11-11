//! Data reconstruction and decoding have distinct proof and execution oracles.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionError, ExecutionOutcome, ExitKind, Fork, Input,
    LiftedByteSegment, LiftedByteSource, LiftedDataWord, LiftedGuardPayload, LiftedRevertDataKind,
    MemoryHost, ProcedurePreference, U256, ValidationStatus, decompile, render_text,
    validate_stage,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
#[allow(dead_code)]
mod assembly;
#[path = "lifted/oracle/expressions.rs"]
mod oracle;
#[path = "lifted/error_payloads/oracle.rs"]
mod payload_oracle;
use assembly::Asm;

const PANIC: &str = include_str!("fixtures/revert-data/panic-41-guard.hex");
const BOUND: &str = include_str!("fixtures/revert-data/custom-symbolic-bound-guard.hex");
const LOCAL: &str = include_str!("fixtures/revert-data/custom-symbolic-failed-arm.hex");

fn input(hex: &str) -> Input {
    Input::new(evm_bytecode::parse_hex(hex.trim()).unwrap(), Fork::Osaka)
}

fn checked(input: &Input) -> Artifact {
    let mut config = Config::default();
    config.readability.procedures = ProcedurePreference::Inline;
    let artifact = decompile(input, &config);
    assert_eq!(
        artifact.completion,
        Completion::Complete,
        "{:?}",
        artifact.diagnostics
    );
    assert!(
        artifact
            .stages
            .iter()
            .all(|stage| stage.validation.status == ValidationStatus::Passed)
    );
    artifact
}

fn host(input: &Input, success: bool, words: &[U256]) -> MemoryHost {
    MemoryHost::new(
        input,
        std::iter::once(U256::from(u64::from(success)))
            .chain(words.iter().copied())
            .flat_map(|word| word.to_be_bytes::<32>())
            .collect(),
    )
}

#[test]
fn panic_and_symbolic_custom_data_preserve_lazy_source_requests_and_faults() {
    let words = [U256::from(17), U256::MAX, U256::from(0x80) << 248];
    for source in [PANIC, BOUND, LOCAL] {
        let input = input(source);
        let artifact = checked(&input);
        let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
        assert!(!program.revert_data.is_empty());
        assert!(
            program
                .revert_data
                .iter()
                .any(|data| matches!(data.content, LiftedRevertDataKind::Segments(_)))
        );
        let text = render_text(&artifact);
        if source == PANIC {
            assert!(text.contains("Panic(0x41)"), "{text}");
            assert!(program.controls.iter().any(|control| {
                control
                    .guard
                    .as_ref()
                    .is_some_and(|guard| matches!(guard.payload, LiftedGuardPayload::Data(_)))
            }));
        } else {
            assert!(text.contains("selector = 0xdeadbeef"), "{text}");
            assert!(text.contains("words = ["), "{text}");
            if source == BOUND {
                assert!(program.controls.iter().any(|control| {
                    control
                        .guard
                        .as_ref()
                        .is_some_and(|guard| matches!(guard.payload, LiftedGuardPayload::Data(_)))
                }));
                let ir = &artifact.current().unwrap().ir;
                let index = program.render_index(ir);
                let boundary = ir.structured.as_ref().unwrap().root.id;
                assert!(
                    program.controls.iter().any(|control| matches!(
                        index.if_plan(control.node, boundary),
                        evm_decompile::LiftedIfPlan::Require(_)
                    )),
                    "already bound custom arguments must admit the compact plan"
                );
                assert!(text.contains("require("), "{text}");
            } else {
                assert!(
                    program
                        .controls
                        .iter()
                        .all(|control| control.guard.is_none()),
                    "failed-route calldata observations must remain explicit"
                );
            }
        }
        for success in [false, true] {
            for limit in [0, 32, 64, 128, 4096] {
                let (outcome, state, capsules) = payload_oracle::certify(
                    &input,
                    &artifact,
                    host(&input, success, &words),
                    limit,
                );
                if success {
                    assert!(matches!(
                        outcome.unwrap(),
                        ExecutionOutcome::Exit {
                            kind: ExitKind::Stop,
                            ..
                        }
                    ));
                    assert_eq!(capsules, 0);
                } else if limit >= if source == PANIC { 64 } else { 128 } {
                    let expected = if source == PANIC {
                        [
                            vec![0x4e, 0x48, 0x7b, 0x71],
                            U256::from(0x41).to_be_bytes::<32>().to_vec(),
                        ]
                        .concat()
                    } else {
                        [
                            vec![0xde, 0xad, 0xbe, 0xef],
                            words
                                .into_iter()
                                .flat_map(|word| word.to_be_bytes::<32>())
                                .collect(),
                        ]
                        .concat()
                    };
                    assert_eq!(
                        payload_oracle::reverted(&outcome.unwrap(), &state),
                        Some(expected)
                    );
                } else {
                    assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
                }
            }
        }
    }
}

#[test]
fn decoding_failure_keeps_recovered_bytes_and_last_write_wins_fragments() {
    let mut a = Asm::default();
    a.word(U256::from(0xdeadbeef_u64) << 224).number(0).op(0x52);
    a.number(0).op(0x35).number(4).op(0x52);
    // One byte cuts the symbolic word; no whole-word custom-error argument
    // exists, but all byte identities are still recoverable.
    a.number(0xa5)
        .number(7)
        .op(0x53)
        .number(36)
        .number(0)
        .op(0xfd);
    let input = a.finish();
    let artifact = checked(&input);
    let data = artifact
        .current()
        .unwrap()
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .revert_data
        .first()
        .unwrap();
    let LiftedRevertDataKind::Segments(segments) = &data.content else {
        panic!("lost byte provenance when format decoding failed")
    };
    assert!(
        segments
            .iter()
            .any(|segment| matches!(segment, LiftedByteSegment::Word { size, .. } if *size < 32))
    );
    for word in [U256::ZERO, U256::MAX, U256::from(0x11223344)] {
        let state = MemoryHost::new(&input, word.to_be_bytes::<32>().to_vec());
        let (outcome, state, _) = payload_oracle::certify(&input, &artifact, state, 4096);
        let mut expected = [
            vec![0xde, 0xad, 0xbe, 0xef],
            word.to_be_bytes::<32>().to_vec(),
        ]
        .concat();
        expected[7] = 0xa5;
        assert_eq!(
            payload_oracle::reverted(&outcome.unwrap(), &state),
            Some(expected)
        );
    }
}

#[test]
fn unknown_alias_and_nonwrapping_failures_keep_the_original_memory_slice() {
    for dynamic_pointer in [false, true] {
        let mut a = Asm::default();
        if dynamic_pointer {
            a.number(0).op(0x35).op(0x80).number(0x42).op(0x90).op(0x52);
            a.number(32).op(0x90).op(0xfd);
        } else {
            a.number(0x42).number(0).op(0x52);
            a.number(0xff).number(0).op(0x35).op(0x52);
            a.number(32).number(0).op(0xfd);
        }
        let input = a.finish();
        let artifact = checked(&input);
        let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
        assert!(
            program
                .revert_data
                .iter()
                .any(|data| matches!(data.content, LiftedRevertDataKind::Raw))
        );
        assert!(program.constructions.is_empty());
        for pointer in [U256::ZERO, U256::from(128), U256::MAX] {
            let host = MemoryHost::new(&input, pointer.to_be_bytes::<32>().to_vec());
            let _ = payload_oracle::certify(&input, &artifact, host, 4096);
        }
    }
}

#[test]
fn numeric_certificate_rejects_forged_symbolic_bytes_and_writer_evidence() {
    let input = input(BOUND);
    let artifact = checked(&input);
    for corruption in 0..7 {
        let mut stage = artifact.current().unwrap().clone();
        let data = stage
            .ir
            .lifted
            .as_mut()
            .unwrap()
            .revert_data
            .first_mut()
            .unwrap();
        match corruption {
            0 => data.writes.reverse(),
            1 => {
                data.writes.pop();
            }
            2 => data.memory = data.offset,
            3 => data.offset = data.size,
            4 => data.code_hash = Some(alloy_primitives::B256::ZERO),
            5 | 6 => {
                let LiftedRevertDataKind::Segments(segments) = &mut data.content else {
                    unreachable!()
                };
                if corruption == 5 {
                    match &mut segments[0] {
                        LiftedByteSegment::Bytes(bytes) => bytes[0] ^= 1,
                        LiftedByteSegment::Word {
                            value: LiftedDataWord::Constant(value),
                            ..
                        } => *value ^= U256::from(1) << 248,
                        _ => unreachable!(),
                    }
                } else {
                    let segment = segments
                        .iter_mut()
                        .find(|segment| {
                            matches!(
                                segment,
                                LiftedByteSegment::Word {
                                    value: LiftedDataWord::SourceValue(_),
                                    ..
                                }
                            )
                        })
                        .unwrap();
                    let LiftedByteSegment::Word { value, .. } = segment else {
                        unreachable!()
                    };
                    *value = LiftedDataWord::Constant(U256::ZERO);
                }
            }
            _ => unreachable!(),
        }
        assert_ne!(
            validate_stage(&stage).status,
            ValidationStatus::Passed,
            "accepted forged data {corruption}"
        );
    }
}

fn call(a: &mut Asm) {
    a.number(0)
        .number(0)
        .number(0)
        .number(0)
        .number(0)
        .number(1)
        .number(1_000_000)
        .op(0xf1)
        .op(0x50);
}

fn forwarded(change_epoch: bool, overwrite: bool) -> Input {
    let mut a = Asm::default();
    call(&mut a);
    a.op(0x3d).number(0).number(0).op(0x3e);
    if change_epoch {
        call(&mut a);
    }
    if overwrite {
        a.number(0x42).number(0).op(0x53);
    }
    a.op(0x3d).number(0).op(0xfd);
    a.finish()
}

#[test]
fn whole_dynamic_returndata_is_a_versioned_slice_and_preserves_copy_faults() {
    let input = forwarded(false, false);
    let artifact = checked(&input);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let data = program.revert_data.first().unwrap();
    let [
        LiftedByteSegment::Slice {
            source: LiftedByteSource::ReturnData(epoch),
            ..
        },
    ] = data.content.segments()
    else {
        panic!(
            "dynamic whole-buffer recovery should not require constant bytes or constant length: {:?}",
            data.content
        )
    };
    assert!(
        stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|operation| operation.id == *epoch && operation.opcode == 0xf1)
    );
    for length in [0_usize, 1, 31, 32, 33, 257] {
        let bytes: Vec<_> = (0..length).map(|i| (i % 251) as u8).collect();
        for limit in [0, 32, 128, 4096] {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.responses.push_back(evm_decompile::ExternalResponse {
                result: U256::ZERO,
                return_data: bytes.clone(),
                ..Default::default()
            });
            let (outcome, host, _) = payload_oracle::certify(&input, &artifact, host, limit);
            if length.div_ceil(32) * 32 <= limit {
                assert_eq!(
                    payload_oracle::reverted(&outcome.unwrap(), &host),
                    Some(bytes.clone())
                );
            } else {
                assert_eq!(outcome, Err(ExecutionError::MemoryLimit));
            }
        }
    }
    let mut forged = stage.clone();
    let data = forged
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .revert_data
        .first_mut()
        .unwrap();
    let LiftedRevertDataKind::Segments(parts) = &mut data.content else {
        unreachable!()
    };
    let LiftedByteSegment::Slice { source, .. } = &mut parts[0] else {
        unreachable!()
    };
    *source = LiftedByteSource::Calldata;
    assert_ne!(validate_stage(&forged).status, ValidationStatus::Passed);
}

#[test]
fn changed_returndata_epoch_or_intervening_write_prevents_whole_buffer_claim() {
    for (change_epoch, overwrite) in [(true, false), (false, true)] {
        let input = forwarded(change_epoch, overwrite);
        let artifact = checked(&input);
        let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
        assert!(
            program
                .revert_data
                .iter()
                .all(|data| matches!(data.content, LiftedRevertDataKind::Raw))
        );
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(evm_decompile::ExternalResponse {
            result: U256::ZERO,
            return_data: vec![1, 2, 3],
            ..Default::default()
        });
        if change_epoch {
            host.responses.push_back(evm_decompile::ExternalResponse {
                result: U256::ZERO,
                return_data: vec![9; 40],
                ..Default::default()
            });
        }
        let _ = payload_oracle::certify(&input, &artifact, host, 4096);
    }
}

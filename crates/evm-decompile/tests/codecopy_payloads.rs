//! Code-backed error bytes retain the original lazy memory/fault semantics.
use evm_decompile::{
    Artifact, Completion, Config, ExecutionError, ExecutionOutcome, ExitKind, Fork, Input,
    MemoryHost, ProcedurePreference, U256, ValidationStatus, decompile, render_text,
    validate_stage,
};

#[path = "lifted/error_payloads/fixtures/asm.rs"]
mod assembly;
#[path = "lifted/oracle/expressions.rs"]
mod oracle;
#[path = "lifted/error_payloads/oracle.rs"]
mod payload_oracle;
use assembly::Asm;

const MESSAGE: &[u8] = b"UniswapV2Library: INSUFFICIENT_LIQUIDITY";

#[derive(Clone, Copy)]
enum Padding {
    Untouched,
    ZeroBefore,
    DirtyBefore,
    DirtyAfter,
    UnknownCopy,
}
#[derive(Clone, Copy)]
enum Source {
    Embedded,
    Short,
    Huge,
}

fn relative(a: &mut Asm, offset: u64, value: U256) {
    a.word(value).op(0x81).number(offset).op(0x01).op(0x52);
}

fn fixture(message: &[u8], padding: Padding, source: Source, copy_size: usize) -> Input {
    let mut entry = Asm::default();
    entry.number(128).number(64).op(0x52);
    let mut a = Asm::default();
    a.number(0).op(0x35).target("success").op(0x57);
    match padding {
        Padding::ZeroBefore | Padding::DirtyBefore => {
            let value = if matches!(padding, Padding::DirtyBefore) {
                U256::MAX
            } else {
                U256::ZERO
            };
            a.word(value)
                .number(128 + 68 + message.len() as u64)
                .op(0x52);
        }
        Padding::UnknownCopy => {
            a.number(32).op(0x35).number(64).number(128 + 96).op(0x37);
        }
        _ => {}
    }
    a.number(64).op(0x51);
    relative(&mut a, 0, U256::from(0x08c379a0_u64) << 224);
    relative(&mut a, 4, U256::from(32));
    relative(&mut a, 36, U256::from(message.len()));
    a.number(copy_size as u64);
    if matches!(source, Source::Huge) {
        a.word(U256::MAX);
    } else {
        a.target("data");
    }
    a.op(0x82).number(68).op(0x01).op(0x39);
    if matches!(padding, Padding::DirtyAfter) {
        a.number(0xa5)
            .op(0x81)
            .number(68 + message.len() as u64)
            .op(0x01)
            .op(0x53);
    }
    a.number((68 + message.len().div_ceil(32) * 32) as u64)
        .op(0x01)
        .number(64)
        .op(0x51)
        .op(0x80)
        .op(0x91)
        .op(0x03)
        .op(0x90)
        .op(0xfd);
    a.dest("success").op(0x00);
    a.data(
        "data",
        if matches!(source, Source::Short) {
            &message[..2]
        } else {
            message
        },
    );
    entry.append(a);
    entry.finish()
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
            .all(|s| s.validation.status == ValidationStatus::Passed)
    );
    artifact
}

fn host(input: &Input, success: bool, length: u64) -> MemoryHost {
    let data = [U256::from(u64::from(success)), U256::from(length)]
        .into_iter()
        .flat_map(|v| v.to_be_bytes::<32>())
        .chain([0xff; 64])
        .collect();
    MemoryHost::new(input, data)
}

#[test]
fn codecopy_strings_with_unwritten_or_explicit_zero_padding_have_lazy_constructors() {
    for message in [
        MESSAGE,
        b"x",
        b"1234567890123456789012345678901234",
        b"12345678901234567890123456789012345",
    ] {
        for padding in [Padding::Untouched, Padding::ZeroBefore] {
            let input = fixture(message, padding, Source::Embedded, message.len());
            let artifact = checked(&input);
            let stage = artifact.current().unwrap();
            let program = stage.ir.lifted.as_ref().unwrap();
            let payload = program
                .payloads
                .iter()
                .find(|p| p.message.as_bytes() == message)
                .unwrap();
            assert_eq!(payload.bytes, payload_oracle::abi(message));
            let revert = program
                .reverts
                .iter()
                .find(|r| r.payload == payload.id)
                .unwrap();
            assert_eq!(
                revert.code_hash,
                Some(alloy_primitives::keccak256(&input.runtime_bytecode))
            );
            assert!(
                program
                    .render_index(&stage.ir)
                    .construction(revert.block)
                    .is_some()
            );
            assert!(render_text(&artifact).contains(std::str::from_utf8(message).unwrap()));
            for success in [false, true] {
                let (outcome, state, capsules) =
                    payload_oracle::certify(&input, &artifact, host(&input, success, 0), 4096);
                if success {
                    assert!(matches!(
                        outcome.unwrap(),
                        ExecutionOutcome::Exit {
                            kind: ExitKind::Stop,
                            ..
                        }
                    ));
                    assert_eq!(capsules, 0);
                } else {
                    assert_eq!(
                        payload_oracle::reverted(&outcome.unwrap(), &state),
                        Some(payload_oracle::abi(message))
                    );
                    assert_eq!(capsules, 1);
                }
            }
        }
    }
}

#[test]
fn unknown_or_dirty_padding_cannot_be_promoted_to_canonical_error_bytes() {
    for padding in [
        Padding::DirtyBefore,
        Padding::DirtyAfter,
        Padding::UnknownCopy,
    ] {
        let input = fixture(MESSAGE, padding, Source::Embedded, MESSAGE.len());
        let artifact = checked(&input);
        assert!(
            artifact
                .current()
                .unwrap()
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .reverts
                .is_empty()
        );
        for length in [0, 64] {
            let _ = payload_oracle::certify(&input, &artifact, host(&input, false, length), 4096);
        }
    }
}

#[test]
fn codecopy_zero_extension_does_not_wrap_a_huge_source_offset() {
    for source in [Source::Short, Source::Huge] {
        let mut message = vec![0; 40];
        if matches!(source, Source::Short) {
            message[..2].copy_from_slice(b"xy");
        }
        let input = fixture(&message, Padding::Untouched, source, message.len());
        let artifact = checked(&input);
        assert!(
            artifact
                .current()
                .unwrap()
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .payloads
                .iter()
                .any(|p| p.bytes == payload_oracle::abi(&message))
        );
        let (outcome, state, _) =
            payload_oracle::certify(&input, &artifact, host(&input, false, 0), 4096);
        assert_eq!(
            payload_oracle::reverted(&outcome.unwrap(), &state),
            Some(payload_oracle::abi(&message))
        );
    }
}

#[test]
fn compact_codecopy_preserves_the_first_memory_fault_and_does_not_run_on_success() {
    let input = fixture(MESSAGE, Padding::Untouched, Source::Embedded, MESSAGE.len());
    let artifact = checked(&input);
    for limit in [128, 256] {
        let (result, _, _) =
            payload_oracle::certify(&input, &artifact, host(&input, false, 0), limit);
        assert_eq!(result, Err(ExecutionError::MemoryLimit));
        let (result, _, capsules) =
            payload_oracle::certify(&input, &artifact, host(&input, true, 0), limit);
        assert!(matches!(
            result.unwrap(),
            ExecutionOutcome::Exit {
                kind: ExitKind::Stop,
                ..
            }
        ));
        assert_eq!(capsules, 0);
    }
    // The proof only reads 64 overlapping bytes. Execution still performs the
    // original large copy and must retain its original allocation failure.
    let input = fixture(MESSAGE, Padding::Untouched, Source::Embedded, 1 << 24);
    let artifact = checked(&input);
    assert!(
        !artifact
            .current()
            .unwrap()
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .reverts
            .is_empty()
    );
    let (result, _, _) = payload_oracle::certify(&input, &artifact, host(&input, false, 0), 4096);
    assert_eq!(result, Err(ExecutionError::MemoryLimit));
}

#[test]
fn independent_replay_rejects_changed_literal_bytes_even_with_an_updated_code_hash() {
    let input = fixture(MESSAGE, Padding::Untouched, Source::Embedded, MESSAGE.len());
    let artifact = checked(&input);
    let mut stage = artifact.current().unwrap().clone();
    let code = stage.ir.runtime_bytecode.as_mut().unwrap();
    let start = code.len() - MESSAGE.len();
    code[start] ^= 1;
    let changed_hash = alloy_primitives::keccak256(code);
    for proof in &mut stage.ir.lifted.as_mut().unwrap().reverts {
        proof.code_hash = Some(changed_hash);
    }
    for data in &mut stage.ir.lifted.as_mut().unwrap().revert_data {
        data.code_hash = Some(changed_hash);
    }
    stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .validate_guard_sources(&stage.ir)
        .unwrap();
    assert_eq!(validate_stage(&stage).status, ValidationStatus::Failed);
}

#[test]
fn captured_router_constructor_browser_fixtures_distinguish_zero_and_dirty_tails() {
    for (hex, certified) in [
        (
            include_str!("fixtures/error-payloads/router-codecopy-liquidity-guard.hex"),
            true,
        ),
        (
            include_str!("fixtures/error-payloads/router-codecopy-liquidity-dirty.hex"),
            false,
        ),
    ] {
        let input = Input::new(evm_bytecode::parse_hex(hex).unwrap(), Fork::Osaka);
        let artifact = checked(&input);
        assert_eq!(
            !artifact
                .current()
                .unwrap()
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .reverts
                .is_empty(),
            certified
        );
        let (outcome, state, capsules) =
            payload_oracle::certify(&input, &artifact, host(&input, false, 0), 4096);
        let returned = payload_oracle::reverted(&outcome.unwrap(), &state).unwrap();
        assert_eq!(&returned[68..108], MESSAGE);
        assert_eq!(returned[108..].iter().all(|byte| *byte == 0), certified);
        // Both windows are fully reconstructible. Only the zero-padded one
        // admits the Error(string) format and its compact require plan.
        assert_eq!(capsules, 1);
        if !certified {
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
            assert!(matches!(
                data.content,
                evm_decompile::LiftedRevertDataKind::Segments(_)
            ));
            assert_eq!(
                data.decode_format(|_| true).unwrap(),
                evm_decompile::LiftedRevertFormat::Raw
            );
            assert!(!render_text(&artifact).contains("require("));
            assert!(render_text(&artifact).contains("evm.codecopy("));
        }
    }
}

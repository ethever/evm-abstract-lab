//! Installed-entry semantics for the independent local decompiler.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .arg("decompile")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn local_code_produces_structured_output_and_inspectable_json_without_a_world() {
    let output = run(&["--hex", "600260030100", "--format", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Complete);
    assert_eq!(
        artifact.config.strategy,
        evm_decompile::Strategy::Dispatcher
    );
    assert!(!artifact.stages.is_empty());
    assert!(!artifact.events.is_empty());
    assert!(artifact.current_stage.is_some());
    let text = run(&["--hex", "600260030100"]);
    assert_eq!(text.status.code(), Some(0));
    assert!(!text.stdout.is_empty());
}

#[test]
fn unknown_indirect_jump_retains_a_result_but_returns_incomplete_exit_code() {
    let output = run(&["--hex", "5f3556", "--format", "json"]);
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Partial);
    assert!(!artifact.diagnostics.is_empty());
    assert!(!artifact.stages.is_empty());
}

#[test]
fn selected_policy_and_fork_are_retained_and_change_artifact_identity() {
    let first = run(&["--hex", "00", "--format", "json", "--fork", "cancun"]);
    let second = run(&[
        "--hex",
        "00",
        "--format",
        "json",
        "--fork",
        "cancun",
        "--strategy",
        "code-duplication",
    ]);
    assert!(first.status.success());
    assert!(second.status.success());
    let first: evm_decompile::Artifact = serde_json::from_slice(&first.stdout).unwrap();
    let second: evm_decompile::Artifact = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(
        second.config.strategy,
        evm_decompile::Strategy::CodeDuplication
    );
    assert_eq!(second.input.fork, evm_decompile::Fork::Cancun);
    assert_ne!(first.id, second.id);
}

#[test]
fn work_limit_produces_a_typed_inspectable_refusal() {
    let output = run(&[
        "--hex",
        "600260030100",
        "--format",
        "json",
        "--max-work",
        "1",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_ne!(artifact.completion, evm_decompile::Completion::Complete);
    assert!(!artifact.diagnostics.is_empty());
}

#[test]
fn valid_hex_decode_failures_are_inspectable_and_byte_limit_precedes_decode() {
    let output = run(&["--hex", "ef00", "--format", "json"]);
    assert_eq!(output.status.code(), Some(2));
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Failed);
    assert!(
        artifact
            .diagnostics
            .iter()
            .any(|d| d.code == evm_decompile::DiagnosticCode::Decode)
    );
    let text = run(&["--hex", "ef00"]);
    assert_eq!(text.status.code(), Some(2));
    assert!(!text.stdout.is_empty());

    let output = run(&[
        "--hex",
        "ef00",
        "--max-bytecode-bytes",
        "0",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(2));
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Limited);
    assert!(
        !artifact
            .diagnostics
            .iter()
            .any(|d| d.code == evm_decompile::DiagnosticCode::Decode)
    );
    assert!(
        artifact
            .diagnostics
            .iter()
            .any(|d| d.code == evm_decompile::DiagnosticCode::Limit)
    );
}

#[test]
fn malformed_hex_is_an_adapter_error_and_has_no_semantic_artifact() {
    let output = run(&["--hex", "0xgg", "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[test]
fn default_text_and_json_expose_lifted_unsigned_predicate_with_retained_raw_stage() {
    let bytecode = "6020355f3510151515600f575f5ffd5b00";
    let output = run(&["--hex", bytecode, "--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        artifact.current().unwrap().kind,
        evm_decompile::StageKind::Lifted
    );
    let raw = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == evm_decompile::StageKind::Structured)
        .expect("retained raw Structured stage");
    assert!(raw.ir.structured.is_some());
    assert!(
        raw.ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .any(|op| op.opcode == 0x15)
    );
    let output = run(&["--hex", bytecode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(" >= "), "{text}");
    assert!(!text.contains("iszero"), "{text}");
}

#[test]
fn installed_cli_names_distinct_call_arguments_and_keeps_json_operand_identity() {
    for (bytecode, opcode, expected_values, expected_text) in [
        (
            "6077606660556044603360226011f15000",
            0xf1,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            "6077606660556044603360226011f25000",
            0xf2,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "callcode(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            "606660556044603360226011f45000",
            0xf4,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "delegatecall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            "606660556044603360226011fa5000",
            0xfa,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "staticcall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
    ] {
        let json = run(&["--hex", bytecode, "--format", "json"]);
        assert!(
            json.status.success(),
            "{}",
            String::from_utf8_lossy(&json.stderr)
        );
        let artifact: evm_decompile::Artifact = serde_json::from_slice(&json.stdout).unwrap();
        let stage = artifact.current().unwrap();
        assert_eq!(stage.kind, evm_decompile::StageKind::Lifted);
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|operation| operation.opcode == opcode)
            .unwrap();
        let actual: Vec<_> = operation
            .inputs
            .iter()
            .map(|id| stage.ir.value(*id).unwrap().fact.singleton().unwrap())
            .collect();
        assert_eq!(
            actual,
            expected_values
                .iter()
                .copied()
                .map(evm_decompile::U256::from)
                .collect::<Vec<_>>()
        );
        let raw = artifact
            .stages
            .iter()
            .find(|stage| stage.kind == evm_decompile::StageKind::Structured)
            .unwrap();
        let mut witness = stage.ir.clone();
        witness.lifted = None;
        assert_eq!(witness, raw.ir);
        let output = run(&["--hex", bytecode]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.matches(expected_text).count(), 1, "{text}");
    }
}

#[test]
fn installed_cli_terminal_text_names_offset_size_and_beneficiary() {
    for (bytecode, expected) in [
        ("60226011f3", "return(offset = 0x11, size = 0x22);"),
        ("60226011fd", "revert(offset = 0x11, size = 0x22);"),
        ("6011ff", "selfdestruct(beneficiary = 0x11);"),
        ("00", "stop();"),
    ] {
        let output = run(&["--hex", bytecode]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert_eq!(text.matches(expected).count(), 1, "{text}");
    }
}

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

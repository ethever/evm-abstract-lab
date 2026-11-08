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
fn retained_add_uses_the_same_operator_style_as_inline_words_and_keeps_named_memory_operands() {
    // CALLDATALOAD(4), ADD(input, 0xa4), cross-block use, MSTORE(0, sum), RETURN(0, 32).
    let bytecode = "60043560a4900161000b565b5f5260205ff3";
    let json = run(&["--hex", bytecode, "--format", "json"]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Complete);
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let operations: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .collect();
    let read = operations
        .iter()
        .find(|operation| operation.opcode == 0x35)
        .unwrap();
    let add = operations
        .iter()
        .find(|operation| operation.opcode == 0x01)
        .unwrap();
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == add.id)
        .unwrap();
    assert_eq!(statement.outputs, add.outputs);
    let renderer = program.render_index(&stage.ir);
    assert_eq!(
        renderer.statement_word_rhs(statement, |_| true).unwrap(),
        None,
        "operator spelling must also apply when there is no algebraic rewrite"
    );
    let expected = format!("v{} = (v{} + 0xa4);", add.outputs[0].0, read.outputs[0].0);
    assert_eq!(
        renderer
            .render_statement_with_dependencies(statement, false, |_| true)
            .unwrap(),
        expected
    );
    assert!(
        artifact
            .origin(evm_decompile::EntityRef {
                stage: stage.id,
                entity: statement.id
            })
            .unwrap()
            .source
            .contains(&add.source.unwrap())
    );
    let raw = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == evm_decompile::StageKind::Structured)
        .unwrap();
    let mut witness = stage.ir.clone();
    witness.lifted = None;
    assert_eq!(witness, raw.ir);

    let text = run(&["--hex", bytecode]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    let text = String::from_utf8(text.stdout).unwrap();
    assert_eq!(
        text.trim_end(),
        evm_decompile::render_text(&artifact).trim_end()
    );
    assert_eq!(text.matches(&expected).count(), 1, "{text}");
    assert!(!text.contains("evm.add("), "{text}");
    assert!(text.contains("evm.calldataload(offset = 0x4)"), "{text}");
    assert!(
        text.contains(&format!(
            "evm.mstore(offset = 0x0, value = v{});",
            add.outputs[0].0
        )),
        "{text}"
    );
    assert!(
        text.contains("evm.return(offset = 0x0, size = 0x20);"),
        "{text}"
    );
    let roundtrip: evm_decompile::Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(roundtrip, artifact);
}

#[test]
fn retained_mask_binding_simplifies_without_changing_raw_witness_or_source() {
    // The jump forces the second AND result to remain a named cross-block value.
    let bytecode = "600a5473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16610033565b5f5500";
    let json = run(&["--hex", bytecode, "--format", "json"]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Complete);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    let masks: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter(|operation| operation.opcode == 0x16)
        .collect();
    assert_eq!(
        masks.len(),
        2,
        "both original AND instructions remain inspectable"
    );
    let outer = masks[1];
    let statement = high
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == outer.id)
        .unwrap();
    assert_eq!(statement.outputs, outer.outputs);
    assert_eq!(statement.inputs.len(), 2);
    let line = high
        .render_index(&stage.ir)
        .render_statement(statement, |_| true)
        .unwrap();
    assert!(
        line.starts_with(&format!("v{} = ", outer.outputs[0].0)),
        "{line}"
    );
    assert_eq!(
        line.matches("0xffffffffffffffffffffffffffffffffffffffff")
            .count(),
        1,
        "{line}"
    );
    assert!(!line.contains("evm.and("), "{line}");
    assert_eq!(line.matches(" & ").count(), 1, "{line}");
    let origin = artifact
        .mappings
        .origins
        .iter()
        .find(|origin| origin.entity.stage == stage.id && origin.entity.entity == statement.id)
        .unwrap();
    assert!(origin.source.contains(&outer.source.unwrap()));
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
    assert_eq!(
        text.trim_end(),
        evm_decompile::render_text(&artifact).trim_end()
    );
    assert_eq!(text.matches(&line).count(), 1, "{text}");
    assert_eq!(text.matches("evm.sload(").count(), 1, "{text}");
    assert_eq!(text.matches("evm.sstore(").count(), 1, "{text}");
    assert!(text.find("evm.sload(").unwrap() < text.find(&line).unwrap());
    assert!(text.find(&line).unwrap() < text.find("evm.sstore(").unwrap());
    let decoded: evm_decompile::Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(decoded, artifact);
}

#[test]
fn default_cli_exports_real_certified_definitions_calls_and_retained_raw_witness() {
    let bytecode =
        include_str!("../../evm-decompile/tests/fixtures/shared-procedures/pure-call-inputs.hex")
            .trim();
    let output = run(&["--hex", bytecode, "--format", "json"]);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.schema_version, 5);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    assert!(
        !high.procedures.is_empty(),
        "installed CLI must publish actual reusable definitions"
    );
    assert!(
        high.invocations.len() >= 2,
        "installed CLI must publish actual invocation sites"
    );
    high.validate_shared_sources(&stage.ir, |_| true).unwrap();
    let text = run(&["--hex", bytecode]);
    assert_eq!(text.status.code(), Some(0));
    let text = String::from_utf8(text.stdout).unwrap();
    assert_eq!(
        text.trim_end(),
        evm_decompile::render_text(&artifact).trim_end()
    );
    let index = high.render_index(&stage.ir);
    for procedure in index.procedures() {
        let header = index
            .render_procedure_header(procedure, true, |_| true)
            .unwrap();
        assert!(
            text.contains(&header),
            "CLI/model definition formatting diverged: {header}"
        );
    }
    for call in &high.invocations {
        let invocation = index
            .render_shared_invocation(call, true, |_| true)
            .unwrap();
        assert!(
            text.contains(&invocation),
            "CLI/model call formatting diverged: {invocation}"
        );
    }
    let raw = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == evm_decompile::StageKind::Structured)
        .unwrap();
    let mut witness = stage.ir.clone();
    witness.lifted = None;
    assert_eq!(
        witness, raw.ir,
        "sharing must preserve the exact raw Structured/IR artifact"
    );
    let decoded: evm_decompile::Artifact =
        serde_json::from_slice(&serde_json::to_vec(&artifact).unwrap()).unwrap();
    assert_eq!(
        decoded, artifact,
        "IDs, source maps and certificate bindings roundtrip"
    );
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
    assert_eq!(artifact.schema_version, 5);
    assert!(artifact.admits_schema());
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
    let current = artifact.current().unwrap();
    let program = current.ir.lifted.as_ref().unwrap();
    let control = program
        .controls
        .iter()
        .find(|control| control.guard.is_some())
        .expect("CLI JSON exposes its certified guard");
    let guard = control.guard.as_ref().unwrap();
    assert!(guard.pass_nonzero);
    let index = program.render_index(&current.ir);
    let evm_decompile::LiftedIfPlan::Require(plan) = index.if_plan(
        control.node,
        current.ir.structured.as_ref().unwrap().root.id,
    ) else {
        panic!("certified empty revert should select require")
    };
    let require = index.render_require(plan, |_| true).unwrap();
    let mut witness = current.ir.clone();
    witness.lifted = None;
    assert_eq!(witness, raw.ir);
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
    assert_eq!(text.matches(&require).count(), 1, "{text}");
    assert!(!text.contains("if ("), "{text}");
    assert!(!text.contains("} else {"), "{text}");
}

#[test]
fn installed_cli_keeps_a_pass_reachable_join_revert_after_the_store() {
    // if (calldataload(0)) { sstore(0, 0x55); } else {} ; revert(0,0)
    let bytecode = "5f356008576010565b60555f556010565b5f5ffd";
    let output = run(&["--hex", bytecode]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let failure_if = text.find("require(").expect("certified guard");
    let store = text.find("evm.sstore(").unwrap();
    let revert = text.rfind("evm.revert(").expect("pass-reachable REVERT");
    assert!(failure_if < store && store < revert, "{text}");
}

#[test]
fn installed_cli_names_distinct_call_arguments_and_keeps_json_operand_identity() {
    for (bytecode, opcode, expected_values, expected_text) in [
        (
            "6077606660556044603360226011f15000",
            0xf1,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            "6077606660556044603360226011f25000",
            0xf2,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.callcode(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            "606660556044603360226011f45000",
            0xf4,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.delegatecall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            "606660556044603360226011fa5000",
            0xfa,
            &[0x11_u64, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.staticcall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
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
fn installed_cli_terminals_keep_named_memory_ranges_and_positional_selfdestruct() {
    for (bytecode, expected) in [
        ("60226011f3", "evm.return(offset = 0x11, size = 0x22);"),
        ("60226011fd", "evm.revert(offset = 0x11, size = 0x22);"),
        ("6011ff", "evm.selfdestruct(0x11);"),
        ("00", "evm.stop();"),
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

#[cfg(target_pointer_width = "64")]
#[test]
fn installed_cli_accepts_wide_work_limits_and_emits_exact_numeric_configuration() {
    for max_work in [
        4_294_967_296_u64,
        10_000_000_000,
        9_007_199_254_740_993,
        u64::MAX,
    ] {
        let allowance = max_work.to_string();
        let output = run(&["--hex", "00", "--format", "json", "--max-work", &allowance]);
        assert!(
            output.status.success(),
            "{allowance}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(artifact.completion, evm_decompile::Completion::Complete);
        assert_eq!(artifact.config.limits.max_work, max_work);
        assert!(artifact.statistics.work > 0 && artifact.statistics.work < 1_000_000);
        assert_eq!(artifact.statistics.blocks, 1);
        assert_eq!(artifact.statistics.operations, 1);
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            wire["config"]["limits"]["max_work"].as_u64(),
            Some(max_work)
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&format!("\"max_work\": {max_work}"))
        );
    }
}

#[cfg(target_pointer_width = "64")]
#[test]
fn installed_cli_defaults_match_shared_maximum_policy_without_spending_the_ceiling() {
    let output = run(&["--hex", "00", "--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(artifact.completion, evm_decompile::Completion::Complete);
    assert_eq!(artifact.config.limits, evm_decompile::Limits::default());
    let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    for field in [
        "max_bytecode_bytes",
        "max_blocks",
        "max_operations",
        "max_values",
        "max_work",
        "max_iterations",
        "max_cloned_blocks",
        "max_history_entities",
    ] {
        assert_eq!(
            wire["config"]["limits"][field].as_u64(),
            Some(u64::MAX),
            "{field}"
        );
    }
    assert_eq!(artifact.config.limits.max_constant_set, 8);
    assert_eq!(artifact.config.limits.max_structuring_depth, 512);
    assert!(artifact.statistics.work > 0 && artifact.statistics.work < 1_000_000);
}

#[test]
fn installed_cli_rejects_overflow_fractional_and_negative_allowances_before_execution() {
    for invalid in ["18446744073709551616", "-1", "0.5"] {
        let output = run(&["--hex", "00", "--format", "json", "--max-work", invalid]);
        assert!(!output.status.success(), "{invalid}");
        assert!(output.stdout.is_empty(), "{invalid}");
        assert!(!output.stderr.is_empty(), "{invalid}");
    }
}

#[test]
fn installed_cli_error_guards_use_exact_router_bytes_and_keep_raw_construction_witnesses() {
    for (bytecode, message) in [
        (
            include_str!(
                "../../evm-decompile/tests/fixtures/error-payloads/router-ds-math-guard.hex"
            )
            .trim(),
            "ds-math-mul-overflow",
        ),
        (
            include_str!(
                "../../evm-decompile/tests/fixtures/error-payloads/router-transfer-guard.hex"
            )
            .trim(),
            "TransferHelper: TRANSFER_FAILED",
        ),
    ] {
        let output = run(&["--hex", bytecode, "--format", "json"]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let artifact: evm_decompile::Artifact = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(artifact.schema_version, 5);
        assert!(artifact.admits_schema());
        let current = artifact.current().unwrap();
        let program = current.ir.lifted.as_ref().unwrap();
        let payload = program
            .payloads
            .iter()
            .find(|p| p.message == message)
            .unwrap();
        assert_eq!(payload.bytes.len(), 100);
        assert!(
            payload.bytes[68 + message.len()..]
                .iter()
                .all(|byte| *byte == 0)
        );
        let control = program
            .controls
            .iter()
            .find(|c| {
                c.guard.as_ref().is_some_and(|g| {
                    g.payload == evm_decompile::LiftedGuardPayload::ErrorString(payload.id)
                })
            })
            .unwrap();
        let index = program.render_index(&current.ir);
        index.validate_guard_sources().unwrap();
        let evm_decompile::LiftedIfPlan::Require(plan) = index.if_plan(
            control.node,
            current.ir.structured.as_ref().unwrap().root.id,
        ) else {
            panic!("exact Error(string) should select require")
        };
        let require = index.render_require(plan, |_| true).unwrap();
        let guard = control.guard.as_ref().unwrap();
        assert!(guard.pass_nonzero);
        let raw = artifact
            .stages
            .iter()
            .find(|s| s.kind == evm_decompile::StageKind::Structured)
            .unwrap();
        let mut witness = current.ir.clone();
        witness.lifted = None;
        assert_eq!(witness, raw.ir);
        let proof = program
            .reverts
            .iter()
            .find(|r| r.payload == payload.id)
            .unwrap();
        let construction = index.construction(proof.block).unwrap();
        assert_eq!(construction.revert, proof.id);
        assert_eq!(proof.writes.len(), 4);
        for id in std::iter::once(payload.id).chain([proof.id, construction.id]) {
            assert!(
                artifact
                    .origin(evm_decompile::EntityRef {
                        stage: current.id,
                        entity: id
                    })
                    .is_some()
            );
        }
        let text = run(&["--hex", bytecode]);
        assert!(text.status.success());
        let text = String::from_utf8(text.stdout).unwrap();
        assert_eq!(text.matches(&require).count(), 1, "{text}");
        assert!(!text.contains("if ("), "{text}");
        assert!(!text.contains("evm.revert(offset = "), "{text}");
        assert_eq!(
            text.matches("evm.mstore(").count(),
            1,
            "retain prefix store: {text}"
        );
        assert!(
            !text.contains("evm.mload("),
            "constructor belongs to lazy require: {text}"
        );
    }
}

#[test]
fn installed_cli_retains_the_actual_constructor_and_bad_padding_stays_raw() {
    let bytecode =
        include_str!("../../evm-decompile/tests/fixtures/error-payloads/router-ds-math-guard.hex")
            .trim();
    // Prefix and complete unrelocated 102-byte Router02 constructor, without
    // the fixture's admission branch or successful SSTORE arm.
    let standalone = format!("6080604052{}", &bytecode[24..228]);
    let text = run(&["--hex", &standalone]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert_eq!(text.matches("evm.revert(offset = ").count(), 1, "{text}");
    assert!(
        text.contains("evm.mstore(") && text.contains("evm.mload("),
        "standalone construction retains its primitive source body: {text}"
    );
    let needle = "64732d6d6174682d6d756c2d6f766572666c6f77000000000000000000000000";
    assert!(bytecode.contains(needle));
    let corrupt_word = format!("{}01{}", &needle[..40], &needle[42..]);
    let corrupt = bytecode.replacen(needle, &corrupt_word, 1);
    let json = run(&["--hex", &corrupt, "--format", "json"]);
    assert!(json.status.success());
    let artifact: evm_decompile::Artifact = serde_json::from_slice(&json.stdout).unwrap();
    let program = artifact.current().unwrap().ir.lifted.as_ref().unwrap();
    assert!(program.payloads.is_empty() && program.constructions.is_empty());
    let text = run(&["--hex", &corrupt]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    assert!(
        !text.contains("require(") && !text.contains("evm.revert(\""),
        "{text}"
    );
    assert!(
        text.contains("evm.mstore(") && text.contains("evm.revert(offset"),
        "{text}"
    );
}

#[test]
fn installed_cli_keeps_observing_and_invalid_failed_arms_explicit_in_both_polarities() {
    for (bytecode, failure_nonzero, invalid) in [
        ("34600a575f54505f5ffd5b00", false, false),
        ("34600557005b5f54505f5ffd", true, false),
        ("346007575f5ffe5b00", false, true),
    ] {
        let json = run(&["--hex", bytecode, "--format", "json"]);
        assert!(
            json.status.success(),
            "{}",
            String::from_utf8_lossy(&json.stderr)
        );
        let artifact: evm_decompile::Artifact = serde_json::from_slice(&json.stdout).unwrap();
        let stage = artifact.current().unwrap();
        let high = stage.ir.lifted.as_ref().unwrap();
        let root = &stage.ir.structured.as_ref().unwrap().root;
        let index = high.render_index(&stage.ir);
        let plan = high
            .controls
            .iter()
            .find_map(|c| index.failure_first(c.node, root.id))
            .unwrap();
        assert_eq!(plan.failure_nonzero(), failure_nonzero);
        let failed_if = format!(
            "if ({}) {{",
            index
                .render_failure_first_condition(plan, |_| true)
                .unwrap()
        );
        let text = run(&["--hex", bytecode]);
        assert!(text.status.success());
        let text = String::from_utf8(text.stdout).unwrap();
        assert_eq!(text.matches(&failed_if).count(), 1, "{text}");
        assert!(
            !text.contains("require(") && !text.contains("} else {"),
            "{text}"
        );
        assert_eq!(
            text.matches("evm.callvalue(").count(),
            1,
            "condition capture executes once: {text}"
        );
        if invalid {
            assert!(text.contains("evm.invalid();"), "{text}");
            assert!(!text.contains("evm.revert("), "{text}");
        } else {
            assert_eq!(text.matches("evm.sload(").count(), 1, "{text}");
            assert_eq!(text.matches("evm.revert(").count(), 1, "{text}");
        }
        assert_eq!(
            text.lines().filter(|s| s.starts_with("evm.stop(")).count(),
            1,
            "normal STOP must be outside the failed arm: {text}"
        );
    }
}

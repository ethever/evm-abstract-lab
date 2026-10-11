//! Real CLI defaults expose one final SSA with native identities and evidence.

use super::{analyze, run_concrete};
use serde_json::{Value, json};

fn report(args: &[&str], exit: i32) -> Value {
    let output = run_concrete(args);
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn default_single_ssa_folds_and_copies_without_losing_bytecode_identity() {
    let folded = report(&["ssa", "--hex", "600160020100", "--format", "json"], 0);
    let ir = &folded["ssa"];
    assert_eq!(ir["complete"], true);
    assert!(folded.get("partial_ssa").is_none());
    assert_eq!(ir["blocks"][0]["instructions"][2]["opcode"], 0x01);
    assert_eq!(ir["blocks"][0]["instructions"][2]["pc"], 4);
    assert_eq!(
        ir["blocks"][0]["instructions"][2]["operation"],
        json!({"Constant": {"value": "0x3"}})
    );
    let mappings = ir["mappings"].as_array().unwrap();
    assert_eq!(mappings.len(), 3);
    for (index, mapping) in mappings.iter().enumerate() {
        assert_eq!(mapping["value"], index);
    }
    assert_eq!(mappings[2]["origin"]["Instruction"]["opcode"], 0x01);
    assert_eq!(
        folded["state_mapping"],
        json!([{"local_state":0,"machine_state":0}])
    );
    assert!(folded["machine_analysis"].is_object());
    assert!(ir["facts"].as_array().unwrap().iter().any(|fact| {
        fact["value"] == 2 && fact["scope"] == "Definition" && fact["constant"] == "0x3"
    }));

    let copied = report(&["ssa", "--hex", "345f015000", "--format", "json"], 0);
    let row = &copied["ssa"]["blocks"][0]["instructions"][2];
    assert_eq!(row["operation"], json!({"Copy":{"value":{"Value":0}}}));
    assert_eq!(row["opcode"], 0x01);
    for command in ["ssa", "explain"] {
        let output = run_concrete(&[command, "--hex", "345f015000"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("%2 = COPY %0 ; origin=ADD"), "{text}");
        assert!(text.contains("Single-program state mapping"));
    }
}

#[test]
fn partial_normalization_keeps_observations_scoped_and_pending_results_absent() {
    let report = report(
        &[
            "ssa",
            "--hex",
            "602a60405100",
            "--allow-partial-ssa",
            "--max-memory-bytes",
            "32",
            "--format",
            "json",
        ],
        2,
    );
    assert!(report.get("ssa").is_none());
    let ir = &report["partial_ssa"];
    assert_eq!(ir["complete"], false);
    let pending = &ir["blocks"][0]["instructions"][2];
    assert_eq!(pending["opcode"], 0x51);
    assert_eq!(pending["progress"], "OperandsConsumed");
    assert!(pending["results"].as_array().unwrap().is_empty());
    assert!(pending["operation"]["Opcode"].is_object());
    assert!(ir["facts"].as_array().unwrap().iter().any(|fact| {
        fact["source"] == "Instruction"
            && fact["scope"] == json!({"State":{"state":0}})
            && fact["validity"] == "Current"
    }));
    assert_eq!(ir["value_count"], 2);
    assert!(!ir["frontiers"].as_array().unwrap().is_empty());
}

#[test]
fn default_world_analysis_and_explain_share_final_operations_and_facts() {
    let output = analyze("call-return-branch", &["--ssa", "--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let ir = &report["ssa"];
    assert_eq!(ir["complete"], true);
    assert_eq!(
        ir["mappings"].as_array().unwrap().len(),
        ir["value_count"].as_u64().unwrap() as usize
    );
    assert!(!ir["facts"].as_array().unwrap().is_empty());
    assert!(
        ir["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|block| block["instructions"].as_array().unwrap())
            .any(|row| row["operation"]["Constant"].is_object())
    );
    let path = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for extra in [vec![], vec!["--verbose"]] {
        let mut args = vec![
            "explain",
            "--world",
            &path,
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
        ];
        args.extend(extra);
        let output = run_concrete(&args);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("CONST ") && text.contains("origin=PUSH"),
            "{text}"
        );
        assert_eq!(text.matches("Verified cross-contract SSA:").count(), 1);
        assert!(text.contains("CALL result") || text.contains("kind=Return"));
    }
}

#[test]
fn a_faulting_push_keeps_its_immediate_and_has_no_constant_definition() {
    let code = format!("{}00", "6001".repeat(1025));
    let report = report(&["ssa", "--hex", &code, "--format", "json"], 0);
    let rows = report["ssa"]["blocks"][0]["instructions"]
        .as_array()
        .unwrap();
    let fault = rows.last().unwrap();
    assert_eq!(fault["pc"], 2048);
    assert_eq!(fault["immediate"], "0x1");
    assert_eq!(fault["progress"], "Faulted");
    assert!(fault["results"].as_array().unwrap().is_empty());
    assert!(fault["operation"]["Opcode"].is_object());
    assert_eq!(report["ssa"]["value_count"], 1024);
    let output = run_concrete(&["ssa", "--hex", &code]);
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("0800: PUSH1 0x1 ; exceptional halt")
    );
}

#[test]
fn single_program_native_precompile_does_not_acquire_a_bytecode_block_pc() {
    let args = [
        "ssa",
        "--hex",
        "600160020100",
        "--evm.to",
        "0x0000000000000000000000000000000000000004",
        "--evm.calldata",
        "0x",
        "--evm.value",
        "0",
        "--evm.gas",
        "1000000",
    ];
    let output = run_concrete(&args);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(
        text.contains("native execution; no bytecode instructions"),
        "{text}"
    );
    assert!(text.contains("Verified cross-contract SSA:"));
    assert!(
        !text
            .lines()
            .any(|line| line.trim_start().starts_with('B') && line.contains(" @ 0x")),
        "{text}"
    );
    assert!(
        !text.lines().any(|line| {
            line.trim_start().split_once(": ").is_some_and(|(pc, _)| {
                pc.len() >= 4 && pc.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        }),
        "{text}"
    );
    assert!(text.contains("projected σᵖ₀ → machine σ₀"));
    let mut json_args = args.to_vec();
    json_args.extend(["--format", "json"]);
    let report = report(&json_args, 0);
    assert_eq!(
        report["state_mapping"],
        json!([{"local_state":0,"machine_state":0}])
    );
    assert_eq!(
        report["machine_analysis"]["states"][0]["key"]["frames"][0]["mode"],
        json!({"Precompile":"0x0000000000000000000000000000000000000004"})
    );
    assert!(
        report["ssa"]["blocks"][0]["instructions"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

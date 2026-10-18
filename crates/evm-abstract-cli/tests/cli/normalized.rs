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
    const CODE: &str = "602a60405100";
    let complete = report(
        &[
            "ssa",
            "--hex",
            CODE,
            "--max-memory-bytes",
            "32",
            "--format",
            "json",
        ],
        0,
    );
    assert_eq!(complete["ssa"]["complete"], true);
    assert_eq!(
        complete["machine_analysis"]["config"]["max_memory_bytes"],
        32
    );
    assert_eq!(complete["machine_analysis"]["frontiers"], json!([]));
    assert_eq!(
        complete["analysis"]["states"][0]["exit_stack"][0]["Constants"],
        json!(["0x2a"])
    );
    assert_eq!(
        complete["analysis"]["states"][0]["exit_stack"][1]["Constants"],
        json!(["0x0"])
    );
    assert_eq!(
        complete["ssa"]["blocks"][0]["instructions"][2]["results"],
        json!([2])
    );
    assert!(
        complete["ssa"]["facts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|fact| {
                fact["value"] == 2 && fact["scope"] == "Definition" && fact["constant"] == "0x0"
            })
    );
    let work = super::work_limit_visiting_pc(4, |limit| {
        let work = limit.map(|limit| limit.to_string());
        let mut args = vec![
            if limit.is_none() { "ssa" } else { "cfg" },
            "--hex",
            CODE,
            "--max-memory-bytes",
            "32",
            "--format",
            "json",
            "--no-root-terminal-coverage",
        ];
        if let Some(work) = &work {
            args.extend(["--max-work", work.as_str()]);
        }
        run_concrete(&args)
    })
    .to_string();
    let report = report(
        &[
            "ssa",
            "--hex",
            CODE,
            "--allow-partial-ssa",
            "--max-memory-bytes",
            "32",
            "--max-work",
            &work,
            "--no-root-terminal-coverage",
            "--format",
            "json",
        ],
        2,
    );
    assert!(report.get("ssa").is_none());
    let ir = &report["partial_ssa"];
    assert_eq!(ir["complete"], false);
    assert_eq!(ir["blocks"][0]["coverage"], "Current");
    let pending = &ir["blocks"][0]["instructions"][2];
    assert_eq!(pending["pc"], 4);
    assert_eq!(pending["opcode"], 0x51);
    assert_eq!(pending["progress"], "OperandsConsumed");
    assert_eq!(pending["operands"], json!([1]));
    assert!(pending["results"].as_array().unwrap().is_empty());
    assert!(pending["operation"]["Opcode"].is_object());
    assert_eq!(ir["blocks"][0]["exit_frames"], json!([[0]]));
    assert_eq!(ir["blocks"][0]["instructions"].as_array().unwrap().len(), 3);
    let mut effect = ir["blocks"][0]["effect"]["result"].as_u64().unwrap();
    for instruction in ir["blocks"][0]["instructions"].as_array().unwrap() {
        assert_eq!(instruction["effect_input"], effect);
        let next = instruction["effect_result"].as_u64().unwrap();
        assert_ne!(next, effect);
        effect = next;
    }
    assert_eq!(ir["blocks"][0]["exit_effect"], effect);
    assert_eq!(ir["effect_count"], 4);
    let machine = &report["machine_analysis"];
    assert_eq!(machine["states"][0]["executed_pcs"], json!([0, 2, 4]));
    assert_eq!(
        machine["states"][0]["exit_stack"].as_array().unwrap().len(),
        1
    );
    assert_eq!(
        machine["states"][0]["exit_stack"][0]["Constants"],
        json!(["0x2a"])
    );
    let outcomes = machine["outcomes"].as_array().unwrap();
    let [pre_entry, post_touch] = outcomes.as_slice() else {
        panic!("expected the pre-entry and block-entry Failure alternatives: {outcomes:?}")
    };
    assert_eq!(machine["states"].as_array().unwrap().len(), 1);
    let state = &machine["states"][0];
    assert_eq!(state["id"], 0);
    assert_eq!(ir["blocks"][0]["state"], state["id"]);
    let frame = &state["entry"]["call_stack"]["root"]["state"];
    let saved = &frame["saved_store"];
    assert!(saved.is_object());
    assert_eq!(frame["returndata"]["length"]["Constants"], json!(["0x0"]));
    assert_eq!(frame["returndata"]["bytes"], json!({}));
    for outcome in outcomes {
        assert_eq!(outcome["state"], state["id"]);
        assert_eq!(outcome["kind"], "Failure");
        assert_eq!(outcome["data"], frame["returndata"]);
    }
    // engine::run_reusing records the untouched pre-entry checkpoint first.
    // A symbolic recipient then may-touches all accounts, enabling the separate
    // late_failure::at_block_entry envelope before either PUSH executes.
    assert_eq!(pre_entry["store"], *saved);
    let mut entered = state["entry"]["store"].clone();
    assert!(entered.get("touched_default").unwrap().is_null());
    entered["touched_default"] = json!(false);
    assert_eq!(entered, *saved);
    // MLOAD's positive read-scan fee stops this prefix before ranges::memory.
    // The second Failure restores the checkpoint, retaining only RIPEMD's
    // sticky MAY touch through finalization. Only owner 3 may gain join entries.
    let ripemd = "0x0000000000000000000000000000000000000003";
    let mut restored = post_touch["store"].clone();
    assert_ne!(restored, *saved);
    for (field, expected) in [
        ("balances", json!("Top")),
        ("nonces", json!("Top")),
        ("codes", json!("Unknown")),
        ("existence", json!("unknown")),
    ] {
        assert_eq!(
            restored[field].as_object_mut().unwrap().remove(ripemd),
            Some(expected),
            "{field}"
        );
    }
    assert_eq!(
        restored["persistent"]["defaults"]
            .as_object_mut()
            .unwrap()
            .remove(ripemd),
        Some(json!("Top"))
    );
    let retained = restored["persistent"]["roots"]
        .as_object_mut()
        .unwrap()
        .remove(ripemd)
        .unwrap();
    let nodes = retained["nodes"].as_array().unwrap();
    let merge = &nodes[usize::try_from(retained["root"].as_u64().unwrap()).unwrap()]["SlotsIte"];
    assert!(merge.is_object());
    let node = |field: &str| &nodes[usize::try_from(merge[field].as_u64().unwrap()).unwrap()];
    assert!(node("condition")["Fresh"].is_u64());
    assert_eq!(*node("else_state"), json!("SlotsZero"));
    let kept = &node("then_state")["SlotsInput"];
    assert_eq!(kept["bindings"], json!([]));
    assert_eq!(kept["source"]["namespace"], "0x300");
    let original = &saved["persistent"]["base"];
    let original_nodes = original["nodes"].as_array().unwrap();
    let original_source = &original_nodes
        [usize::try_from(original["root"].as_u64().unwrap()).unwrap()]["SlotsInput"]["source"];
    assert_eq!(retained["identity_scope"], original["identity_scope"]);
    assert_eq!(
        nodes[usize::try_from(kept["source"]["token"].as_u64().unwrap()).unwrap()],
        original_nodes[usize::try_from(original_source["token"].as_u64().unwrap()).unwrap()]
    );
    assert_eq!(restored, *saved);
    assert_eq!(ir["frontiers"], machine["frontiers"]);
    let frontiers = ir["frontiers"].as_array().unwrap();
    assert_eq!(frontiers.len(), 1);
    assert_eq!(frontiers[0]["reason"], "Work");
    assert_eq!(frontiers[0]["from"], 0);
    assert_eq!(frontiers[0]["pc"], 4);
    assert!(ir["facts"].as_array().unwrap().iter().any(|fact| {
        fact["source"] == "Instruction"
            && fact["scope"] == json!({"State":{"state":0}})
            && fact["validity"] == "Current"
    }));
    assert!(ir["facts"].as_array().unwrap().iter().all(|fact| {
        fact["value"] != 2
            && (fact["source"] != "Instruction" || fact["scope"] == json!({"State":{"state":0}}))
    }));
    assert_eq!(ir["value_count"], 2);
    assert_eq!(ir["mappings"].as_array().unwrap().len(), 2);
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

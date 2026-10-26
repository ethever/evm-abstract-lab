//! Real CLI defaults expose one final SSA with native identities and evidence.

use super::{analyze, run_concrete};
use evm_abstract::domain::{EvmValue, NumericValue};
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
    let facts = ir["facts"].as_array().unwrap();
    for (value, source, constant) in [
        (0, "Literal", "0x1"),
        (1, "Literal", "0x2"),
        (2, "ConstantFold", "0x3"),
    ] {
        let proof = facts
            .iter()
            .find(|fact| {
                fact["value"] == value && fact["source"] == source && fact["scope"] == "Definition"
            })
            .expect("derived word proof remains available without a numeric carrier");
        assert_eq!(proof["constant"], constant);
        assert!(proof["numeric"].is_null());
    }
    let observed = facts
        .iter()
        .find(|fact| fact["value"] == 2 && fact["source"] == "Instruction")
        .expect("the actual instruction observation is retained separately");
    assert!(observed["numeric"].is_object());
    assert_eq!(observed["constant"], "0x3");

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
fn cfg_retains_native_precompile_boundaries_and_their_states() {
    const PRECOMPILE: &str = "0x0000000000000000000000000000000000000004";
    for model in ["product", "finite-set"] {
        let mut args = vec![
            "cfg",
            "--hex",
            "5f5f5f5f5f60046207a120f15000",
            "--evm.to",
            "0x1111111111111111111111111111111111111111",
            "--evm.caller",
            "0x0000000000000000000000000000000000001000",
            "--evm.value",
            "0",
            "--evm.calldata",
            "0x",
            "--evm.gas",
            "1000000",
            "--numeric-model",
            model,
            "--no-root-terminal-coverage",
            "--format",
            "json",
        ];
        let cfg = report(&args, 0);
        args[0] = "ssa";
        let ssa = report(&args, 0);
        let machine = cfg
            .get("machine_analysis")
            .expect("CFG must retain the native graph behind its boundary edges");
        assert!(cfg.get("ssa").is_none());
        assert!(cfg.get("partial_ssa").is_none());
        assert_eq!(cfg["edge_endpoint_namespace"], "local");
        assert_eq!(cfg["boundary_endpoint_namespace"], "native");
        assert_eq!(
            super::native::canonical(machine),
            super::native::canonical(&ssa["machine_analysis"])
        );
        let mut projection = super::native::without_application_metadata(&cfg);
        projection
            .as_object_mut()
            .unwrap()
            .remove("machine_analysis");
        assert_eq!(
            super::native::canonical(&projection),
            super::native::canonical(&ssa["analysis"])
        );
        super::native::without_application_metadata(&ssa);

        let native_states = machine["states"].as_array().unwrap();
        let native_state = |id: &Value| {
            native_states
                .iter()
                .find(|state| state["id"] == *id)
                .expect("exported native endpoint must resolve to a machine state")
        };
        let boundaries = cfg["boundary_edges"].as_array().unwrap();
        assert!(!boundaries.is_empty());
        for edge in boundaries {
            native_state(&edge["from"]);
            native_state(&edge["to"]);
            assert!(machine["edges"].as_array().unwrap().contains(edge));
        }
        let precompile = native_states
            .iter()
            .find(|state| {
                state["entry"]["call_stack"]["children"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|child| child["state"]["code"]["Precompile"] == PRECOMPILE)
            })
            .expect("native graph retains the actual identity-precompile child frame");
        assert!(precompile["entry"]["store"].is_object());
        assert!(precompile["exit"]["store"].is_object());
        assert!(
            boundaries
                .iter()
                .any(|edge| { edge["kind"] == "Call" && edge["to"] == precompile["id"] })
        );
        assert!(
            boundaries
                .iter()
                .any(|edge| { edge["kind"] == "Return" && edge["from"] == precompile["id"] })
        );

        let mapping = cfg["execution_state_ids"].as_array().unwrap();
        let local_states = cfg["states"].as_array().unwrap();
        assert_eq!(mapping.len(), local_states.len());
        for local in local_states {
            let id = usize::try_from(local["id"].as_u64().unwrap()).unwrap();
            let native = native_state(&mapping[id]);
            let frames = native["key"]["frames"].as_array().unwrap();
            assert_eq!(frames.len(), 1);
            for field in ["basic_block_index", "stack_height"] {
                assert_eq!(local["key"][field], frames[0][field]);
            }
            assert_eq!(local["executed_pcs"], native["executed_pcs"]);
        }
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
        complete["analysis"]["states"][0]["exit_stack"][0]["numeric"]["constants"],
        json!(["0x2a"])
    );
    assert_eq!(
        complete["analysis"]["states"][0]["exit_stack"][1]["numeric"]["constants"],
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
        machine["states"][0]["exit_stack"][0]["numeric"]["constants"],
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
    assert_eq!(
        frame["returndata"]["length"]["numeric"]["constants"],
        json!(["0x0"])
    );
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
        (
            "balances",
            serde_json::to_value(EvmValue::<NumericValue>::top()).unwrap(),
        ),
        (
            "nonces",
            serde_json::to_value(EvmValue::<NumericValue>::top()).unwrap(),
        ),
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
        Some(serde_json::to_value(EvmValue::<NumericValue>::top()).unwrap())
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
    let report = crate::native::select(&report);
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
    const ENTRY: &str = "0x0000000000000000000000000000000000000004";
    const CODE: &str = "600160020100";
    for command in ["cfg", "ssa", "explain"] {
        let output = run_concrete(&[
            command,
            "--hex",
            CODE,
            "--evm.to",
            ENTRY,
            "--evm.calldata",
            "0x",
            "--evm.value",
            "0",
            "--evm.gas",
            "1000000",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(!text.contains("Single-program state mapping"), "{text}");
        if command != "cfg" {
            let (_, ssa) = text.split_once("Verified cross-contract SSA:").unwrap();
            assert!(
                ssa.contains("native execution; no bytecode instructions"),
                "{text}"
            );
            assert!(
                !ssa.lines()
                    .any(|line| line.trim_start().starts_with('B') && line.contains(" @ 0x")),
                "{text}"
            );
            assert!(
                !ssa.lines()
                    .any(
                        |line| line.trim_start().split_once(": ").is_some_and(|(pc, _)| {
                            pc.len() >= 4 && pc.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    ),
                "{text}"
            );
        }
    }
    for model in ["product", "finite-set"] {
        let mut args = vec![
            "cfg",
            "--hex",
            CODE,
            "--evm.to",
            ENTRY,
            "--evm.calldata",
            "0x",
            "--evm.value",
            "0",
            "--evm.gas",
            "1000000",
            "--numeric-model",
            model,
            "--format",
            "json",
        ];
        let cfg = report(&args, 0);
        args[0] = "ssa";
        let ssa = report(&args, 0);
        assert_eq!(cfg["numeric_model"], model);
        assert_eq!(ssa["numeric_model"], model);
        assert_eq!(
            super::native::canonical(&super::native::without_application_metadata(&cfg)),
            super::native::canonical(&ssa["analysis"])
        );
        assert_eq!(
            ssa["analysis"]["states"][0]["key"]["frames"][0]["mode"],
            json!({"Precompile":ENTRY})
        );
        assert_eq!(ssa["analysis"]["states"][0]["executed_pcs"], json!([]));
        assert_eq!(ssa["ssa"]["complete"], true);
        assert_eq!(ssa["ssa"]["blocks"][0]["instructions"], json!([]));
        assert!(ssa.get("state_mapping").is_none());
        assert!(cfg["postprocessing_work"].as_u64().unwrap() > 0);
        assert!(
            ssa["postprocessing_work"].as_u64().unwrap()
                >= cfg["postprocessing_work"].as_u64().unwrap()
        );
        // Native execution has no scalar instruction facts here; structural
        // SSA can use zero additional numeric work after the paid root query.
        super::native::without_application_metadata(&ssa);

        // The minimum positive execution allowance stops before recording any
        // frame. Paid source classification must still select the native view.
        args[0] = "cfg";
        args.extend(["--max-work", "1", "--no-root-terminal-coverage"]);
        let unopened = report(&args, 2);
        assert_eq!(unopened["status"], "Incomplete");
        assert_eq!(unopened["states"], json!([]));
        assert!(unopened["world"].is_object());
        assert!(unopened.get("program").is_none());
        assert_eq!(unopened["frontiers"][0]["reason"], "Work");
        assert!(unopened["frontiers"][0]["pc"].is_null());
        assert!(unopened["postprocessing_work"].as_u64().unwrap() > 0);
        super::native::without_application_metadata(&unopened);
        args[0] = "ssa";
        args.push("--allow-partial-ssa");
        let partial = report(&args, 2);
        assert_eq!(
            super::native::canonical(&super::native::without_application_metadata(&unopened)),
            super::native::canonical(&partial["analysis"])
        );
        assert_eq!(partial["partial_ssa"]["complete"], false);
        assert_eq!(partial["partial_ssa"]["blocks"], json!([]));
        assert!(partial.get("ssa").is_none());
        assert!(partial.get("state_mapping").is_none());
        assert!(
            partial["postprocessing_work"].as_u64().unwrap()
                >= unopened["postprocessing_work"].as_u64().unwrap()
        );
        super::native::without_application_metadata(&partial);
    }

    let unopened = run_concrete(&[
        "explain",
        "--hex",
        CODE,
        "--evm.to",
        ENTRY,
        "--max-work",
        "1",
        "--no-root-terminal-coverage",
    ]);
    assert_eq!(unopened.status.code(), Some(2));
    let text = String::from_utf8(unopened.stdout).unwrap();
    assert!(
        text.contains("Incomplete") && text.contains("SSA unavailable"),
        "{text}"
    );
    assert!(!text.contains("Single-program state mapping"));
    assert!(!text.contains("Verified cross-contract SSA:"));

    // Installed bytes cannot give a native precompile a bytecode block identity.
    let path = std::env::temp_dir().join(format!(
        "evm-cli-native-precompile-{}.json",
        std::process::id()
    ));
    std::fs::write(&path, json!({
        "fork":"osaka", "provenance":"cli:native-precompile-authority",
        "accounts":[{"address":ENTRY,"code":format!("0x{CODE}"),"balance":"0x0","nonce":"0x0","existence":"present"}],
    }).to_string()).unwrap();
    let mut args = vec![
        "analyze",
        "--world",
        path.to_str().unwrap(),
        "--evm.to",
        ENTRY,
        "--evm.calldata",
        "0x",
        "--evm.value",
        "0",
        "--evm.gas",
        "1000000",
        "--ssa",
    ];
    let output = run_concrete(&args);
    args.extend(["--format", "json"]);
    let machine = run_concrete(&args);
    std::fs::remove_file(path).unwrap();
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
        !text.lines().any(
            |line| line.trim_start().split_once(": ").is_some_and(|(pc, _)| {
                pc.len() >= 4 && pc.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        ),
        "{text}"
    );
    assert!(
        machine.status.success(),
        "{}",
        String::from_utf8_lossy(&machine.stderr)
    );
    let report: Value = serde_json::from_slice(&machine.stdout).unwrap();
    let report = crate::native::select(&report);
    assert_eq!(
        report["analysis"]["world"]["accounts"][ENTRY]["code"]["Runtime"]["bytecode"],
        format!("0x{CODE}")
    );
    assert_eq!(
        report["analysis"]["states"][0]["key"]["frames"][0]["mode"],
        json!({"Precompile":ENTRY})
    );
    assert_eq!(report["analysis"]["states"][0]["executed_pcs"], json!([]));
    assert_eq!(report["ssa"]["complete"], true);
    assert_eq!(report["ssa"]["blocks"][0]["instructions"], json!([]));
    assert!(
        report["ssa"]["blocks"][0]
            .get("basic_block_index")
            .is_none()
    );
    assert!(report.get("state_mapping").is_none());
}

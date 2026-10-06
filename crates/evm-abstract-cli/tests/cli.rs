//! 从安装后同名的真实二进制验证输入、状态、JSON 和 exit code 契约。

use std::process::Command;

#[path = "cli/explain.rs"]
mod explain;

#[path = "cli/domains.rs"]
mod domains;
#[path = "cli/numbers.rs"]
mod numbers;
#[path = "cli/rpc.rs"]
mod rpc;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn help_and_version_are_available() {
    assert!(run(&["--help"]).status.success());
    assert!(
        String::from_utf8(run(&["--version"]).stdout)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cfg_json_includes_status_contexts_and_unknown_jump_diagnostic() {
    let output = run(&[
        "cfg",
        "--hex",
        "600035565b00",
        "--format",
        "json",
        "--context-depth",
        "1",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "Converged");
    assert_eq!(json["config"]["context_depth"], 1);
    let states = json["states"].as_array().unwrap();
    for state in states {
        assert!(state["key"]["basic_block_index"].is_u64());
        assert!(state["key"].get("block").is_none());
    }
    assert!(
        states
            .iter()
            .any(|state| state["key"]["basic_block_index"] == 1)
    );
    assert_eq!(json["program"]["blocks"][1]["start_pc"], 4);
    assert!(
        json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["kind"] == "UnknownJump")
    );
}

#[test]
fn cfg_file_accepts_depth_ten_and_preserves_default_and_explicit_depths() {
    let path = format!(
        "{}/../../examples/internal-calls.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = run(&["cfg", "--file", &path, "--context-depth", "10"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(String::from_utf8_lossy(&text.stdout).contains("context_depth=10"));
    for explicit in [None, Some(0), Some(10), Some(usize::MAX)] {
        let mut args = vec!["cfg", "--file", &path, "--format", "json"];
        let depth = explicit.map(|value| value.to_string());
        if let Some(depth) = &depth {
            args.extend(["--context-depth", depth.as_str()]);
        }
        let output = run(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["status"], "Converged");
        assert_eq!(json["config"]["context_depth"], explicit.unwrap_or(8));
        let states = json["states"].as_array().unwrap();
        if explicit == Some(0) {
            assert!(
                states
                    .iter()
                    .all(|state| { state["key"]["context"].as_array().unwrap().is_empty() })
            );
        } else {
            assert!(
                states
                    .iter()
                    .any(|state| { state["key"]["context"].as_array().unwrap().len() > 3 })
            );
        }
    }
}

#[test]
fn ssa_json_carries_cfg_and_value_definitions() {
    let output = run(&["ssa", "--hex", "600160020100", "--format", "json"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["ssa"]["value_count"], 3);
    assert_eq!(json["analysis"]["status"], "Converged");
}

#[test]
fn budgets_exit_two_and_never_return_partial_ssa() {
    let cfg = run(&[
        "cfg",
        "--hex",
        "6003565b00",
        "--max-states",
        "1",
        "--format",
        "json",
    ]);
    assert_eq!(cfg.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&cfg.stdout).unwrap();
    assert_eq!(json["status"], "Incomplete");
    assert!(!json["frontiers"].as_array().unwrap().is_empty());
    let ssa = run(&["ssa", "--hex", "6003565b00", "--max-states", "1"]);
    assert_eq!(ssa.status.code(), Some(2));
    assert!(ssa.stdout.is_empty());
}

#[test]
fn invalid_hex_has_a_useful_error() {
    let output = run(&["disasm", "--hex", "zz"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("invalid hex")
    );
}

#[test]
fn input_choices_are_exclusive_and_required() {
    assert_eq!(run(&["cfg"]).status.code(), Some(2));
    assert_eq!(run(&["cfg", "--fork", "osaka"]).status.code(), Some(2));
    assert_eq!(
        run(&["cfg", "--hex", "00", "--file", "example.hex"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn fork_changes_clz_cfg_and_all_exports_report_the_selected_rules() {
    let code = "60011e60f79003565b602a00";
    let default = run(&["cfg", "--hex", code, "--format", "json"]);
    assert!(default.status.success());
    let json: serde_json::Value = serde_json::from_slice(&default.stdout).unwrap();
    assert_eq!(json["program"]["fork"], "osaka");
    assert_eq!(json["edges"].as_array().unwrap().len(), 1);
    for fork in ["cancun", "prague", "osaka"] {
        let output = run(&["cfg", "--hex", code, "--format", "json", "--fork", fork]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["program"]["fork"], fork);
        assert_eq!(
            json["edges"].as_array().unwrap().len(),
            usize::from(fork == "osaka")
        );
        let disasm = run(&["disasm", "--hex", code, "--fork", fork]);
        assert!(
            String::from_utf8(disasm.stdout)
                .unwrap()
                .contains(&format!("fork={fork}"))
        );
        let dot = run(&["cfg", "--hex", code, "--fork", fork, "--format", "dot"]);
        assert!(
            String::from_utf8(dot.stdout)
                .unwrap()
                .contains(&format!("fork={fork}"))
        );
        let ssa = run(&["ssa", "--hex", code, "--fork", fork, "--format", "json"]);
        assert!(ssa.status.success());
        let json: serde_json::Value = serde_json::from_slice(&ssa.stdout).unwrap();
        assert_eq!(json["analysis"]["program"]["fork"], fork);
    }
    assert_eq!(
        run(&["cfg", "--hex", code, "--fork", "amsterdam"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn delegation_code_reports_its_target_instead_of_a_false_completed_cfg() {
    let code = "ef01001111111111111111111111111111111111111111";
    let output = run(&["cfg", "--hex", code, "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("EIP-7702") && error.contains("0x1111111111111111111111111111111111111111")
    );
}

fn analyze(name: &str, extra: &[&str]) -> std::process::Output {
    let path = format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut args = vec!["analyze", "--world", &path];
    if !extra.contains(&"--entry") {
        args.extend(["--entry", "0x0000000000000000000000000000000000000101"]);
    }
    args.extend(extra);
    run(&args)
}

#[test]
fn world_analyze_uses_the_same_default_and_accepts_larger_context_depths() {
    for explicit in [None, Some(0), Some(10), Some(usize::MAX)] {
        let mut extra = vec!["--format", "json"];
        let depth = explicit.map(|value| value.to_string());
        if let Some(depth) = &depth {
            extra.extend(["--context-depth", depth.as_str()]);
        }
        let output = analyze("call-return-branch", &extra);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["status"], "Converged");
        assert_eq!(
            json["config"]["analysis"]["context_depth"],
            explicit.unwrap_or(8)
        );
    }
}

#[test]
fn primary_analyze_json_has_world_call_frames_returns_and_outcomes() {
    let output = analyze("call-return-branch", &["--format", "json"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "Converged");
    assert_eq!(json["world"]["fork"], "osaka");
    assert_eq!(json["world"]["provenance"], "offline:call-return-branch:v1");
    let edges = json["edges"].as_array().unwrap();
    assert!(edges.iter().any(|edge| edge["kind"] == "Call"));
    assert!(edges.iter().any(|edge| edge["kind"] == "Return"));
    assert!(
        json["states"]
            .as_array()
            .unwrap()
            .iter()
            .any(|state| state["key"]["frames"].as_array().unwrap().len() == 2)
    );
    for state in json["states"].as_array().unwrap() {
        for key in state["key"]["frames"].as_array().unwrap() {
            assert!(key["basic_block_index"].is_u64());
            assert!(key.get("block").is_none());
        }
        let stack = &state["entry"]["call_stack"];
        for frame in std::iter::once(&stack["root"]).chain(stack["children"].as_array().unwrap()) {
            assert!(frame["state"]["key"]["basic_block_index"].is_u64());
            assert!(frame["state"]["key"].get("block").is_none());
        }
    }
    assert!(!json["outcomes"].as_array().unwrap().is_empty());
}

#[test]
fn world_ssa_verifies_complete_calls_and_is_not_built_for_frontiers() {
    let output = analyze("proxy-storage", &["--format", "json", "--ssa"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["analysis"]["status"], "Converged");
    assert!(json["ssa"].is_object());
    let incomplete = analyze("missing-code", &["--format", "json", "--ssa"]);
    assert_eq!(incomplete.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&incomplete.stdout).unwrap();
    assert_eq!(json["status"], "Incomplete");
    assert!(json.get("ssa").is_none());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("SSA unavailable"));
}

#[test]
fn missing_code_and_shared_budgets_keep_typed_frontiers_and_exit_two() {
    for (name, args, reason) in [
        ("missing-code", vec!["--format", "json"], "MissingCode"),
        (
            "reentry",
            vec!["--format", "json", "--max-call-depth", "2"],
            "CallDepth",
        ),
        (
            "call-return-branch",
            vec!["--format", "json", "--max-work", "1"],
            "Work",
        ),
        (
            "call-return-branch",
            vec!["--format", "json", "--max-memory-bytes", "1"],
            "Memory",
        ),
    ] {
        let output = analyze(name, &args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{name} {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["status"], "Incomplete");
        assert!(
            json["frontiers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|frontier| frontier["reason"].to_string().contains(reason))
        );
    }
}

#[test]
fn world_text_and_dot_expose_code_storage_and_call_identity() {
    let text = analyze("proxy-storage", &[]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    let state_header = text
        .lines()
        .find(|line| line.contains("Stack height") && line.contains("Code hash"))
        .unwrap();
    let columns: Vec<_> = state_header.split('|').map(str::trim).collect();
    assert!(
        columns.contains(&"Code") && columns.contains(&"Address") && columns.contains(&"Caller")
    );
    for address in [
        "0x0000000000000000000000000000000000000101",
        "0x0000000000000000000000000000000000000201",
        "0x0000000000000000000000000000000000000202",
        "0x0000000000000000000000000000000000000300",
    ] {
        assert!(
            text.contains(address),
            "missing full account identity {address}"
        );
    }
    assert!(text.contains("Call") && text.contains("Return"));
    let dot = analyze("proxy-storage", &["--format", "dot"]);
    assert!(dot.status.success());
    let dot = String::from_utf8(dot.stdout).unwrap();
    assert!(dot.contains("digraph world") && dot.contains("code=") && dot.contains("address="));
    assert!(dot.contains("Call") && dot.contains("Return"));
}

#[test]
fn requested_world_text_is_readable_and_format_switches_keep_complete_evidence() {
    let text = analyze("returndata-copy", &[]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    for section in [
        "Analysis",
        "Snapshot",
        "States",
        "State details",
        "Transitions",
        "Outcomes",
        "Call summaries",
        "Diagnostics",
        "Frontiers",
    ] {
        assert!(
            text.lines().any(|line| line.trim() == section),
            "missing section {section}:\n{text}"
        );
    }
    assert!(text.contains("stack in") && text.contains("stack out"));
    let json = analyze("returndata-copy", &["--format", "json"]);
    assert!(json.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(json["status"], "Converged");
    let outcomes = json["outcomes"].as_array().unwrap();
    assert!(outcomes.iter().any(|outcome| outcome["kind"] == "Failure"));
    assert!(outcomes.iter().any(|outcome| outcome["kind"] == "Return"));
    let outcome_headers: Vec<_> = text
        .lines()
        .filter(|line| {
            let first = line.split('|').next().unwrap().trim();
            first.strip_prefix('O').is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
        .collect();
    assert_eq!(
        outcome_headers.len(),
        outcomes.len(),
        "text must preserve every JSON outcome"
    );
    for outcome in outcomes {
        assert!(outcome["data"]["length"].is_object());
        assert!(outcome["data"]["bytes"].is_object());
        assert!(outcome["data"]["default"].is_object());
        assert!(outcome["store"]["account_observations"].is_array());
    }
    let dot = analyze("returndata-copy", &["--format", "dot"]);
    assert!(dot.status.success());
    let dot = String::from_utf8(dot.stdout).unwrap();
    for (index, outcome) in outcomes.iter().enumerate() {
        assert!(dot.contains(&format!("O{index} [label=")));
        assert!(dot.contains(&format!(
            "S{} -> O{index}",
            outcome["state"].as_u64().unwrap()
        )));
    }
    let incomplete = analyze("missing-code", &[]);
    assert_eq!(incomplete.status.code(), Some(2));
    let incomplete = String::from_utf8(incomplete.stdout).unwrap();
    assert!(
        incomplete.contains("Incomplete")
            && incomplete.contains("Frontiers")
            && incomplete.contains("MissingCode")
    );
}

#[test]
fn world_entry_and_data_errors_are_reported_before_execution() {
    for (args, expected) in [
        (vec!["--entry", "0x01"], "entry address"),
        (vec!["--calldata", "0xzz"], "calldata hex"),
        (vec!["--value", "-1"], "unexpected argument"),
        (vec!["--value", "0xzz"], "ASCII decimal digits"),
    ] {
        let output = analyze("call-return-branch", &args);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn entry_static_mode_faults_at_write_and_returns_a_completed_failure() {
    let output = analyze("call-return-branch", &["--format", "json", "--static"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["entry"]["is_static"], true);
    assert_eq!(json["status"], "Converged");
    assert!(
        json["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| outcome["kind"] == "Failure")
    );
}

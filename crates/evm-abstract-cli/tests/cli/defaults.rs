//! Installed entrypoints share Web's policy while honoring explicit overrides.

use super::{analyze, run_concrete};
use evm_abstract_protocol::AnalysisLimits;
use serde_json::Value;
use std::process::Output;

fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let report = crate::native::select(&report);
    super::native::without_application_metadata(&report);
    assert_eq!(report["numeric_model"], "product");
    report
}

fn policy(report: &Value, world: bool, expected: &AnalysisLimits) {
    let config = if world {
        &report["config"]["analysis"]
    } else {
        &report["config"]
    };
    for (name, value) in [
        ("context_depth", expected.context_depth),
        ("max_states", expected.max_states),
        ("max_transfers", expected.max_transfers),
    ] {
        assert_eq!(config[name], value, "{name}: {config}");
    }
    for (name, value) in [
        ("max_nodes", expected.max_expression_nodes),
        ("max_depth", expected.max_expression_depth),
        ("max_constraints", expected.max_constraints),
    ] {
        assert_eq!(
            report["domain_spec"]["relations"][name], value,
            "{name}: {config}"
        );
    }
    assert_eq!(
        report["domain_spec"]["relations"]["enabled"],
        expected.relations_enabled
    );
    assert!(report["domain_spec"]["relations"].get("provider").is_none());
    assert!(report["domain_spec"]["relations"].get("rlimit").is_none());
    for name in [
        "domain_profile",
        "max_constants",
        "reduction_rounds",
        "max_facts",
        "relations",
    ] {
        assert!(config.get(name).is_none(), "{name}: {config}");
    }
    assert_eq!(report["domain_spec"]["capacity"], expected.max_constants);
    assert_eq!(
        report["domain_spec"]["reduction_rounds"],
        expected.reduction_rounds
    );
    assert_eq!(report["domain_spec"]["fact_limit"], expected.max_facts);
    if world {
        for (name, value) in [
            ("max_work", expected.max_work),
            ("max_call_depth", expected.max_call_depth),
            ("max_memory_bytes", expected.max_memory_bytes),
        ] {
            assert_eq!(report["config"][name], value, "{name}");
        }
        assert_eq!(report["config"]["use_summaries"], expected.use_summaries);
        assert_eq!(
            report["config"]["root_terminal_coverage"],
            expected.root_terminal_coverage
        );
    }
}

#[test]
fn cfg_ssa_and_world_json_use_the_protocol_web_defaults_in_the_real_engine() {
    let expected = AnalysisLimits::default();
    assert_eq!(expected.max_work, 2_000_000_000);
    assert!(expected.root_terminal_coverage);
    let path = format!(
        "{}/../../examples/straight-line.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    for command in ["cfg", "ssa"] {
        for (source, input) in [("--hex", "00"), ("--file", path.as_str())] {
            let json = report(run_concrete(&[command, source, input, "--format", "json"]));
            policy(
                if command == "ssa" {
                    &json["analysis"]
                } else {
                    &json
                },
                false,
                &expected,
            );
            if command == "ssa" {
                assert_eq!(
                    json["machine_analysis"]["config"]["max_work"],
                    2_000_000_000
                );
                assert_eq!(
                    json["machine_analysis"]["config"]["root_terminal_coverage"],
                    true
                );
            }
        }
    }
    let world = report(analyze("call-return-branch", &["--format", "json"]));
    policy(&world, true, &expected);
}

#[test]
fn old_explicit_world_limits_survive_in_the_report_without_clamping() {
    let expected = AnalysisLimits {
        max_constants: 8,
        reduction_rounds: 4,
        max_facts: 256,
        max_states: 4096,
        max_transfers: 100_000,
        max_work: 20_000_000,
        root_terminal_coverage: false,
        max_call_depth: 32,
        max_memory_bytes: 65_536,
        max_expression_nodes: 1024,
        max_expression_depth: 64,
        max_constraints: 128,
        ..AnalysisLimits::default()
    };
    let output = analyze(
        "call-return-branch",
        &[
            "--format",
            "json",
            "--max-constants",
            "8",
            "--reduction-rounds",
            "4",
            "--max-facts",
            "256",
            "--max-states",
            "4096",
            "--max-transfers",
            "100000",
            "--max-work",
            "20000000",
            "--no-root-terminal-coverage",
            "--max-call-depth",
            "32",
            "--max-memory-bytes",
            "65536",
            "--max-symbolic-nodes",
            "1024",
            "--max-symbolic-depth",
            "64",
            "--max-relations",
            "128",
        ],
    );
    let world: Value = serde_json::from_slice(&output.stdout).unwrap();
    super::native::assert_exit(&world, &output);
    assert!(world["native"].is_null());
    assert_eq!(world["hybrid"]["coverage"]["requested_detail_regions"], 1);
    policy(crate::native::global(&world), true, &expected);
}

#[test]
fn raw_commands_use_the_larger_memory_default_but_preserve_explicit_memory_frontiers() {
    // MSTORE touches bytes 65536..65568, just beyond the former implicit budget.
    // A following MLOAD checks the actual write under the exact-byte cap.
    let code = "60016201000052620100005100";
    assert_eq!(AnalysisLimits::default().max_memory_bytes, 64 * 1024 * 1024);
    for command in ["cfg", "ssa", "explain"] {
        let default = run_concrete(&[command, "--hex", code]);
        assert!(
            default.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&default.stderr)
        );
        let bounded = run_concrete(&[command, "--hex", code, "--max-memory-bytes", "65536"]);
        assert!(
            bounded.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&bounded.stderr)
        );
        // The default symbolic calldata Input needs three retained nodes.
        // This two-node control refuses initialization before MSTORE executes.
        let refused = run_concrete(&[
            command,
            "--hex",
            code,
            "--max-memory-bytes",
            "65536",
            "--max-symbolic-nodes",
            "2",
            "--no-root-terminal-coverage",
        ]);
        assert_eq!(refused.status.code(), Some(2), "{command}");
        let text = String::from_utf8(refused.stdout).unwrap();
        if command == "ssa" {
            assert!(
                text.is_empty(),
                "closed SSA must still refuse incomplete execution"
            );
            assert!(String::from_utf8_lossy(&refused.stderr).contains("SSA unavailable"));
        } else {
            assert!(
                text.contains("Incomplete")
                    && text.contains("Relations")
                    && text.contains("ExpressionLimit"),
                "{command}: {text}"
            );
        }
    }
    for explicit in [None, Some("65536")] {
        let mut args = vec!["ssa", "--hex", code, "--format", "json"];
        if let Some(cap) = explicit {
            args.extend(["--max-memory-bytes", cap]);
        }
        let complete = report(run_concrete(&args));
        let machine = &complete["machine_analysis"];
        assert_eq!(
            machine["config"]["max_memory_bytes"],
            explicit.map_or(64 * 1024 * 1024, |cap| cap.parse::<usize>().unwrap())
        );
        assert_eq!(machine["status"], "Converged");
        assert_eq!(machine["frontiers"], serde_json::json!([]));
        assert_eq!(
            machine["states"][0]["exit_stack"][0]["numeric"]["constants"],
            serde_json::json!(["0x1"])
        );
        let load = complete["ssa"]["blocks"][0]["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["opcode"] == 0x51)
            .unwrap();
        assert_eq!(load["progress"], "Completed");
        assert_eq!(load["results"].as_array().unwrap().len(), 1);
    }
    let refused = run_concrete(&[
        "cfg",
        "--hex",
        code,
        "--max-memory-bytes",
        "65536",
        "--max-symbolic-nodes",
        "2",
        "--no-root-terminal-coverage",
        "--format",
        "json",
    ]);
    assert_eq!(refused.status.code(), Some(2));
    let refused: Value = serde_json::from_slice(&refused.stdout).unwrap();
    let refused = crate::native::select(&refused);
    assert_eq!(refused["domain_spec"]["relations"]["max_nodes"], 2);
    assert_eq!(refused["status"], "Incomplete");
    assert_eq!(refused["states"], serde_json::json!([]));
    assert_eq!(refused["edges"], serde_json::json!([]));
    assert_eq!(refused["diagnostics"], serde_json::json!([]));
    assert_eq!(
        refused["frontiers"],
        serde_json::json!([{
            "from": null,
            "target": {"basic_block_index": 0, "stack_height": 0, "context": []},
            "reason": {"Relations": "ExpressionLimit"},
            "limit": "Model"
        }])
    );
    let partial = run_concrete(&[
        "ssa",
        "--hex",
        code,
        "--max-memory-bytes",
        "65536",
        "--max-symbolic-nodes",
        "2",
        "--no-root-terminal-coverage",
        "--allow-partial-ssa",
        "--format",
        "json",
    ]);
    assert_eq!(partial.status.code(), Some(2));
    let partial: Value = serde_json::from_slice(&partial.stdout).unwrap();
    let partial = crate::native::select(&partial);
    assert!(partial.get("ssa").is_none());
    assert_eq!(partial["analysis"]["config"], refused["config"]);
    assert_eq!(partial["analysis"]["states"], refused["states"]);
    assert_eq!(partial["analysis"]["edges"], refused["edges"]);
    assert_eq!(partial["analysis"]["frontiers"], refused["frontiers"]);
    assert_eq!(partial["state_mapping"], serde_json::json!([]));
    let machine = &partial["machine_analysis"];
    assert_eq!(machine["status"], "Incomplete");
    assert_eq!(machine["config"]["max_memory_bytes"], 65536);
    assert_eq!(machine["domain_spec"]["relations"]["max_nodes"], 2);
    assert_eq!(machine["states"], serde_json::json!([]));
    assert_eq!(machine["edges"], serde_json::json!([]));
    assert_eq!(machine["outcomes"], serde_json::json!([]));
    assert_eq!(
        machine["frontiers"],
        serde_json::json!([{
            "from": null,
            "target": null,
            "pc": null,
            "reason": {"Relations": "ExpressionLimit"},
            "range": null
        }])
    );
    let ir = &partial["partial_ssa"];
    assert_eq!(ir["complete"], false);
    assert_eq!(ir["blocks"], serde_json::json!([]));
    assert_eq!(ir["frontiers"], machine["frontiers"]);
    assert_eq!(ir["value_count"], 0);
    assert_eq!(ir["mappings"], serde_json::json!([]));
}

#[test]
fn raw_work_budget_flags_stop_execution_instead_of_only_changing_displayed_arguments() {
    for command in ["cfg", "ssa", "explain"] {
        let output = run_concrete(&[command, "--hex", "600160020100", "--max-work", "1"]);
        assert_eq!(output.status.code(), Some(2), "{command}");
        let text = String::from_utf8(output.stdout).unwrap();
        if command == "ssa" {
            assert!(text.is_empty());
        } else {
            assert!(
                text.contains("Incomplete") && text.contains("Work"),
                "{command}: {text}"
            );
        }
    }
}

#[test]
fn raw_and_world_verbose_explanations_report_the_same_web_precision_policy() {
    let expected = AnalysisLimits::default();
    let path = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for args in [
        vec!["explain", "--hex", "00"],
        vec![
            "explain",
            "--world",
            path.as_str(),
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
        ],
        vec![
            "explain",
            "--world",
            path.as_str(),
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
            "--verbose",
        ],
    ] {
        let output = run_concrete(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        for expected in [
            format!("reduction rounds={}", expected.reduction_rounds),
            format!("fact atoms={}", expected.max_facts),
            format!("expression nodes={}", expected.max_expression_nodes),
            format!("depth={}", expected.max_expression_depth),
            format!("constraints={}", expected.max_constraints),
        ] {
            assert!(text.contains(&expected), "missing {expected}: {text}");
        }
    }
}

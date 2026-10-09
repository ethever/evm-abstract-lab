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
    serde_json::from_slice(&output.stdout).unwrap()
}

fn policy(report: &Value, world: bool, expected: &AnalysisLimits) {
    let config = if world {
        &report["config"]["analysis"]
    } else {
        &report["config"]
    };
    for (name, value) in [
        ("max_constants", expected.max_constants),
        ("reduction_rounds", expected.reduction_rounds),
        ("max_facts", expected.max_facts),
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
        ("rlimit", u64::from(expected.smt_rlimit)),
    ] {
        assert_eq!(config["relations"][name], value, "{name}: {config}");
    }
    assert_eq!(config["relations"]["enabled"], expected.relations_enabled);
    assert_eq!(config["relations"]["provider"], "z3");
    assert_eq!(config["domain_profile"], "product");
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
    }
}

#[test]
fn cfg_ssa_and_world_json_use_the_protocol_web_defaults_in_the_real_engine() {
    let expected = AnalysisLimits::default();
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
        max_call_depth: 32,
        max_memory_bytes: 65_536,
        max_expression_nodes: 1024,
        max_expression_depth: 64,
        max_constraints: 128,
        smt_rlimit: 100_000,
        ..AnalysisLimits::default()
    };
    let world = report(analyze(
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
            "--smt.rlimit",
            "100000",
        ],
    ));
    policy(&world, true, &expected);
}

#[test]
fn raw_commands_use_the_larger_memory_default_but_preserve_explicit_memory_frontiers() {
    // MSTORE touches bytes 65536..65568, just beyond the former implicit budget.
    let code = "6001620100005200";
    for command in ["cfg", "ssa", "explain"] {
        let default = run_concrete(&[command, "--hex", code]);
        assert!(
            default.status.success(),
            "{command}: {}",
            String::from_utf8_lossy(&default.stderr)
        );
        let bounded = run_concrete(&[command, "--hex", code, "--max-memory-bytes", "65536"]);
        assert_eq!(bounded.status.code(), Some(2), "{command}");
        let text = String::from_utf8(bounded.stdout).unwrap();
        if command == "ssa" {
            assert!(
                text.is_empty(),
                "closed SSA must still refuse incomplete execution"
            );
        } else {
            assert!(
                text.contains("Incomplete") && text.contains("Memory"),
                "{command}: {text}"
            );
        }
    }
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
            format!("rlimit={}", expected.smt_rlimit),
            format!("expression nodes={}", expected.max_expression_nodes),
            format!("depth={}", expected.max_expression_depth),
            format!("constraints={}", expected.max_constraints),
        ] {
            assert!(text.contains(&expected), "missing {expected}: {text}");
        }
    }
}

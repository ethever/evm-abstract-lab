//! Root terminal coverage is independent of detail completion and CLI exit status.

use super::{analyze, native, run_concrete};
use serde_json::{Value as Json, json};
use std::process::Output;

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const JUMP: &str = "6003565b00";
const NOTE: &str = "Root terminal coverage (conservative)";

fn report(output: &Output, exit: i32) -> Json {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn assert_certificate(analysis: &Json) {
    assert_eq!(analysis["schema_version"], 11);
    assert_eq!(analysis["status"], "Incomplete");
    assert!(!analysis["frontiers"].as_array().unwrap().is_empty());
    let coverage = &analysis["root_terminal_coverage"];
    assert_eq!(coverage["scope"], "AllInputStates");
    assert_eq!(coverage["trigger"], "Transfers");
    assert_eq!(coverage["input"]["config"]["root_terminal_coverage"], true);
    let outcomes = coverage["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 3);
    for (outcome, kind) in outcomes.iter().zip(["Return", "Revert", "Failure"]) {
        assert_eq!(outcome["kind"], kind);
        assert!(outcome["data"].is_object() && outcome["store"].is_object());
        assert!(outcome.get("state").is_none() && outcome.get("pc").is_none());
    }
}

#[test]
fn certificate_keeps_exit_two_and_the_same_joint_observed_detail_and_partial_ssa() {
    // Unknown CALLER forks to two STOP blocks. Two transfers observe one actual
    // terminal result while leaving the other block unprocessed.
    let arguments = [
        "ssa",
        "--hex",
        "33600557005b00",
        "--max-transfers",
        "2",
        "--allow-partial-ssa",
        "--format",
        "json",
    ];
    let mut disabled = arguments.to_vec();
    disabled.push("--no-root-terminal-coverage");
    let ordinary = report(&run_concrete(&disabled), 2);
    assert!(ordinary["analysis"].get("root_terminal_coverage").is_none());
    assert!(
        ordinary["machine_analysis"]
            .get("root_terminal_coverage")
            .is_none()
    );
    assert!(
        !ordinary["machine_analysis"]["outcomes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let mut enabled = arguments.to_vec();
    enabled.push("--root-terminal-coverage");
    let covered = report(&run_concrete(&arguments), 2);
    assert_eq!(
        native::canonical(&covered),
        native::canonical(&report(&run_concrete(&enabled), 2))
    );
    for analysis in [&covered["analysis"], &covered["machine_analysis"]] {
        assert_certificate(analysis);
    }
    assert_eq!(
        covered["analysis"]["root_terminal_coverage"],
        covered["machine_analysis"]["root_terminal_coverage"]
    );
    assert!(covered.get("root_terminal_coverage").is_none());
    assert!(covered.get("ssa").is_none());
    assert_eq!(covered["partial_ssa"]["status"], "Incomplete");
    assert_eq!(
        covered["partial_ssa"]["frontiers"],
        covered["machine_analysis"]["frontiers"]
    );
    // One renaming for the whole tuple preserves aliases across data, Stores,
    // states and SSA; preparation's extra paid work is intentionally separate.
    let detail = |report: &Json| {
        json!({
            "states": report["machine_analysis"]["states"],
            "edges": report["machine_analysis"]["edges"],
            "frontiers": report["machine_analysis"]["frontiers"],
            "outcomes": report["machine_analysis"]["outcomes"],
            "partial_ssa": report["partial_ssa"],
        })
    };
    assert_eq!(
        native::canonical(&detail(&covered)),
        native::canonical(&detail(&ordinary))
    );
}

#[test]
fn plain_native_and_world_ssa_json_forward_only_core_owned_coverage() {
    let raw = report(
        &run_concrete(&[
            "cfg",
            "--hex",
            JUMP,
            "--max-transfers",
            "1",
            "--format",
            "json",
        ]),
        2,
    );
    assert_certificate(&raw);
    for extra in [
        vec!["--format", "json"],
        vec!["--format", "json", "--ssa", "--allow-partial-ssa"],
    ] {
        let mut arguments = vec!["--max-transfers", "1"];
        arguments.extend(extra);
        let report = report(&analyze("call-return-branch", &arguments), 2);
        let analysis = report.get("analysis").unwrap_or(&report);
        assert_certificate(analysis);
        assert_eq!(analysis["config"]["root_terminal_coverage"], true);
        if let Some(partial) = report.get("partial_ssa") {
            assert_eq!(partial["frontiers"], analysis["frontiers"]);
            assert!(report.get("root_terminal_coverage").is_none());
            assert!(report.get("ssa").is_none());
        }
    }
}

#[test]
fn text_labels_one_certificate_and_keeps_detail_frontiers_and_partial_ssa() {
    let fixture = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for mut arguments in [
        vec!["cfg", "--hex", JUMP],
        vec!["ssa", "--hex", JUMP, "--allow-partial-ssa"],
        vec!["explain", "--hex", JUMP, "--allow-partial-ssa"],
        vec![
            "analyze",
            "--world",
            &fixture,
            "--evm.to",
            ENTRY,
            "--ssa",
            "--allow-partial-ssa",
        ],
        vec![
            "explain",
            "--world",
            &fixture,
            "--evm.to",
            ENTRY,
            "--allow-partial-ssa",
        ],
        vec![
            "explain",
            "--world",
            &fixture,
            "--evm.to",
            ENTRY,
            "--allow-partial-ssa",
            "--verbose",
        ],
    ] {
        arguments.extend(["--max-transfers", "1"]);
        let output = run_concrete(&arguments);
        assert_eq!(output.status.code(), Some(2), "{arguments:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("Incomplete") && text.contains("Transfers"),
            "{text}"
        );
        assert_eq!(text.matches(NOTE).count(), 1, "{text}");
        assert!(text.contains("scope=AllInputStates"));
        assert!(text.contains("alternatives=Return, Revert, Failure"));
        if arguments.contains(&"--allow-partial-ssa") {
            assert!(text.contains("Partial SSA (machine state IDs):"), "{text}");
        }
    }
    let strict = run_concrete(&[
        "ssa",
        "--hex",
        JUMP,
        "--max-transfers",
        "1",
        "--root-terminal-coverage",
    ]);
    assert_eq!(strict.status.code(), Some(2));
    assert!(strict.stdout.is_empty());
    let error = String::from_utf8(strict.stderr).unwrap();
    assert!(error.contains("SSA unavailable") && error.contains("Incomplete"));
    assert_eq!(error.matches(NOTE).count(), 1);
}

#[test]
fn default_and_explicit_switches_without_a_certificate_keep_the_existing_output_contract() {
    for extra in [vec![], vec!["--max-work", "1"]] {
        let exit = if extra.is_empty() { 0 } else { 2 };
        let mut arguments = vec!["cfg", "--hex", "00", "--format", "json"];
        arguments.extend_from_slice(&extra);
        for flag in [
            None,
            Some("--root-terminal-coverage"),
            Some("--no-root-terminal-coverage"),
        ] {
            let mut arguments = arguments.clone();
            if let Some(flag) = flag {
                arguments.push(flag);
            }
            let report = report(&run_concrete(&arguments), exit);
            assert_eq!(report["schema_version"], 11);
            assert!(report.get("root_terminal_coverage").is_none());
        }
        let mut arguments = vec!["explain", "--hex", "00"];
        arguments.extend_from_slice(&extra);
        let enabled = run_concrete(&arguments);
        arguments.push("--no-root-terminal-coverage");
        let ordinary = run_concrete(&arguments);
        assert_eq!(ordinary.status.code(), Some(exit));
        assert_eq!(enabled.status.code(), Some(exit));
        assert_eq!(enabled.stdout, ordinary.stdout);
        assert_eq!(enabled.stderr, ordinary.stderr);
        let text = String::from_utf8(enabled.stdout).unwrap();
        assert!(!text.contains(NOTE));
        assert!(!text.contains("not attached"));
    }
}

#[test]
fn fixed_rpc_forwards_default_and_explicit_coverage_with_and_without_discovery() {
    use super::explain::{RpcFixture, RpcServer};
    const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
    std::thread::scope(|scope| {
        for discovery in [false, true] {
            for command in ["analyze", "explain"] {
                for flag in [
                    None,
                    Some("--root-terminal-coverage"),
                    Some("--no-root-terminal-coverage"),
                ] {
                    let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
                    let mut arguments = vec![
                        command,
                        "--rpc",
                        &server.endpoint,
                        "--block-hash",
                        BLOCK,
                        "--evm.to",
                        ENTRY,
                        "--max-transfers",
                        "1",
                    ];
                    if let Some(flag) = flag {
                        arguments.push(flag);
                    }
                    if !discovery {
                        arguments.push("--no-rpc-discovery");
                    }
                    let enabled = flag != Some("--no-root-terminal-coverage");
                    if command == "analyze" {
                        arguments.extend(["--format", "json", "--ssa", "--allow-partial-ssa"]);
                        let report = report(&run_concrete(&arguments), 2);
                        if enabled {
                            assert_certificate(&report["analysis"]);
                        } else {
                            assert!(report["analysis"].get("root_terminal_coverage").is_none());
                        }
                        assert_eq!(report["analysis"]["status"], "Incomplete");
                        assert!(
                            !report["analysis"]["frontiers"]
                                .as_array()
                                .unwrap()
                                .is_empty()
                        );
                        assert_eq!(
                            report["analysis"]["config"]["root_terminal_coverage"],
                            enabled
                        );
                        assert_eq!(report["analysis"]["config"]["max_work"], 2_000_000_000);
                        assert_eq!(
                            report["partial_ssa"]["frontiers"],
                            report["analysis"]["frontiers"]
                        );
                    } else {
                        let output = run_concrete(&arguments);
                        assert_eq!(output.status.code(), Some(2));
                        let text = String::from_utf8(output.stdout).unwrap();
                        assert!(text.contains("Incomplete") && text.contains("Transfers"));
                        assert!(text.contains("SSA unavailable"));
                        assert_eq!(text.matches(NOTE).count(), usize::from(enabled), "{text}");
                    }
                    let requests = server.finish();
                    assert!(
                        requests
                            .iter()
                            .any(|request| request["method"] == "eth_getCode")
                    );
                }
            }
        }
    });
}

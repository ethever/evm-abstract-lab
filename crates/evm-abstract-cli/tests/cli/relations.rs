//! Local path reasoning remains useful without a solver dependency or policy.

use super::{analyze, run_concrete};
use serde_json::{Value, json};
use std::process::{Command, Output};

const GUARDS: &str = "3480600114600957005b80600214601257005b00";

fn report(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn default_relation_policy_matches_web_for_program_and_world() {
    let raw = report(run_concrete(&["cfg", "--hex", "00", "--format", "json"]));
    let world = report(analyze("call-return-branch", &["--format", "json"]));
    let defaults = evm_abstract_protocol::AnalysisLimits::default();
    let expected = json!({
        "enabled": defaults.relations_enabled,
        "max_nodes": defaults.max_expression_nodes,
        "max_depth": defaults.max_expression_depth,
        "max_constraints": defaults.max_constraints,
    });
    for policy in [
        &raw["domain_spec"]["relations"],
        &world["domain_spec"]["relations"],
    ] {
        assert_eq!(policy, &expected);
    }
}

#[test]
fn local_facts_preserve_feasible_paths_and_exclude_a_contradictory_branch() {
    // Empty PATH also guards against accidentally introducing external tools.
    let output = Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .env("PATH", "")
        .args([
            "cfg",
            "--hex",
            GUARDS,
            "--format",
            "json",
            "--context-depth",
            "0",
        ])
        .output()
        .unwrap();
    let analysis = report(output);
    assert_eq!(analysis["status"], "Converged");
    assert_eq!(analysis["domain_spec"]["relations"]["enabled"], true);
    assert!(analysis["frontiers"].as_array().unwrap().is_empty());
    let blocks = analysis["program"]["blocks"].as_array().unwrap();
    let reachable: Vec<_> = analysis["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|state| {
            blocks[state["key"]["basic_block_index"].as_u64().unwrap() as usize]["start_pc"]
                .as_u64()
                .unwrap()
        })
        .collect();
    // value=1 can reach the second guard, but never its value=2 branch.
    assert!(reachable.contains(&9), "{reachable:?}");
    assert!(reachable.contains(&8), "{reachable:?}");
    assert!(reachable.contains(&17), "{reachable:?}");
    assert!(!reachable.contains(&18), "{reachable:?}");
}

#[test]
fn world_ssa_and_text_report_the_complete_retained_relation_policy() {
    let world_path = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let policy = [
        "--max-symbolic-nodes",
        "4096",
        "--max-symbolic-depth",
        "128",
        "--max-relations",
        "1024",
    ];
    let expected =
        json!({"enabled": true, "max_nodes": 4096, "max_depth": 128, "max_constraints": 1024});
    let mut world_args = vec!["--format", "json"];
    world_args.extend_from_slice(&policy);
    let world = report(analyze("call-return-branch", &world_args));
    assert_eq!(world["domain_spec"]["relations"], expected);
    let mut ssa_args = vec!["ssa", "--hex", GUARDS, "--format", "json"];
    ssa_args.extend_from_slice(&policy);
    let ssa = report(run_concrete(&ssa_args));
    assert_eq!(ssa["analysis"]["domain_spec"]["relations"], expected);
    for mut args in [
        vec!["cfg", "--hex", "00"],
        vec!["explain", "--hex", "00"],
        vec![
            "analyze",
            "--world",
            world_path.as_str(),
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
        ],
        vec![
            "explain",
            "--world",
            world_path.as_str(),
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
        ],
        vec![
            "explain",
            "--world",
            world_path.as_str(),
            "--evm.to",
            "0x0000000000000000000000000000000000000101",
            "--verbose",
        ],
    ] {
        args.extend_from_slice(&policy);
        let output = run_concrete(&args);
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.contains("relations=true | local propagation | expression nodes=4096 | depth=128 | constraints=1024"),
            "{text}"
        );
        for obsolete in ["SMT=", "rlimit=", "resource unit="] {
            assert!(!text.contains(obsolete), "{obsolete}: {text}");
        }
    }
}

#[test]
fn obsolete_solver_flags_are_rejected_and_absent_from_help() {
    for command in ["cfg", "ssa", "explain", "analyze"] {
        for (flag, value) in [
            ("--smt.provider", "z3"),
            ("--smt.rlimit", "200000"),
            ("--smt-rlimit", "100000"),
        ] {
            let output = run_concrete(&[command, flag, value]);
            assert_eq!(output.status.code(), Some(2), "{command} {flag}");
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("unexpected argument"));
        }
        let output = run_concrete(&[command, "--help"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        for obsolete in ["--smt.provider", "--smt.rlimit", "--smt-rlimit"] {
            assert!(!text.contains(obsolete));
        }
        for retained in [
            "--max-symbolic-nodes",
            "--max-symbolic-depth",
            "--max-relations",
        ] {
            assert!(text.contains(retained));
        }
    }
}

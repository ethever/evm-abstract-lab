//! The installed command selects native providers and records the resource policy.

use super::{analyze, run_concrete};
use serde_json::Value;
use std::process::{Command, Output};

const GUARDS: &str = "3480600114600957005b80600214601257005b00";

fn json(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn default_smt_policy_is_z3_with_a_hundred_thousand_resource_units() {
    let raw = json(run_concrete(&["cfg", "--hex", "00", "--format", "json"]));
    let world = json(analyze("call-return-branch", &["--format", "json"]));
    for policy in [
        &raw["config"]["relations"],
        &world["config"]["analysis"]["relations"],
    ] {
        assert_eq!(policy["provider"], "z3");
        assert_eq!(policy["rlimit"], 100_000);
    }
}

#[test]
fn each_provider_runs_without_a_solver_executable_and_preserves_feasible_paths() {
    for provider in ["z3", "bitwuzla", "cvc5"] {
        // Empty PATH makes an accidental dependency on a solver executable fail.
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
                "--smt.provider",
                provider,
                "--smt.rlimit",
                "200000",
            ])
            .output()
            .unwrap();
        let analysis = json(output);
        assert_eq!(analysis["status"], "Converged", "{provider}");
        assert_eq!(analysis["config"]["relations"]["provider"], provider);
        assert_eq!(analysis["config"]["relations"]["rlimit"], 200_000);
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
        assert!(reachable.contains(&9), "{provider}: {reachable:?}");
        assert!(reachable.contains(&8), "{provider}: {reachable:?}");
        assert!(reachable.contains(&17), "{provider}: {reachable:?}");
        assert!(!reachable.contains(&18), "{provider}: {reachable:?}");
    }
}

#[test]
fn world_json_ssa_json_and_analysis_text_report_the_selected_provider() {
    let world_path = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    for provider in ["z3", "bitwuzla", "cvc5"] {
        let policy = ["--smt.provider", provider, "--smt.rlimit", "200000"];
        let mut world_args = vec!["--format", "json"];
        world_args.extend_from_slice(&policy);
        let world = json(analyze("call-return-branch", &world_args));
        assert_eq!(
            world["config"]["analysis"]["relations"]["provider"],
            provider
        );
        assert_eq!(world["config"]["analysis"]["relations"]["rlimit"], 200_000);

        let mut ssa_args = vec!["ssa", "--hex", GUARDS, "--format", "json"];
        ssa_args.extend_from_slice(&policy);
        let ssa = json(run_concrete(&ssa_args));
        assert_eq!(ssa["analysis"]["config"]["relations"]["provider"], provider);
        assert_eq!(ssa["analysis"]["config"]["relations"]["rlimit"], 200_000);

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
            assert!(text.contains(&format!("SMT=in-process {provider} | rlimit=200000")));
            let unit = match provider {
                "z3" => "z3 resource units",
                "bitwuzla" => "termination checks (cooperative)",
                "cvc5" => "cvc5 resource units",
                _ => unreachable!(),
            };
            assert!(text.contains(&format!("resource unit={unit}")));
        }
    }
}

#[test]
fn dotted_smt_flags_validate_provider_and_nonzero_resource_limit() {
    let invalid_provider = run_concrete(&["cfg", "--hex", "00", "--smt.provider", "unknown"]);
    assert_eq!(invalid_provider.status.code(), Some(2));
    let zero = run_concrete(&["cfg", "--hex", "00", "--smt.rlimit", "0"]);
    assert_eq!(zero.status.code(), Some(1));
    assert!(zero.stdout.is_empty());
    assert!(String::from_utf8_lossy(&zero.stderr).contains("must be positive"));
    let old_flag = run_concrete(&["cfg", "--hex", "00", "--smt-rlimit", "100000"]);
    assert_eq!(old_flag.status.code(), Some(2));
    for command in ["cfg", "ssa", "explain", "analyze"] {
        let output = run_concrete(&[command, "--help"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("--smt.provider"));
        assert!(text.contains("--smt.rlimit"));
        assert!(!text.contains("--smt-rlimit"));
    }
}

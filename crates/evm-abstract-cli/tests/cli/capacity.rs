//! 大容量必须传入真实引擎；完整候选、默认策略和资源中断分别核对。

use super::{analyze, run};
use serde_json::Value;

#[test]
fn program_entrypoints_accept_every_positive_usize_capacity() {
    for capacity in [65, 1000, usize::MAX] {
        let capacity = capacity.to_string();
        for profile in ["product", "constants-only"] {
            for command in ["cfg", "ssa", "explain"] {
                let output = run(&[
                    command,
                    "--hex",
                    "00",
                    "--domain",
                    profile,
                    "--max-constants",
                    &capacity,
                ]);
                assert!(
                    output.status.success(),
                    "{command} {profile} {capacity}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
    }
}

#[test]
fn constants_only_clz_keeps_the_complete_set_at_capacity_257() {
    for capacity in [256, 257] {
        let capacity = capacity.to_string();
        let output = run(&[
            "cfg",
            "--hex",
            "5f351e00",
            "--domain",
            "constants-only",
            "--max-constants",
            &capacity,
            "--format",
            "json",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["status"], "Converged");
        assert_eq!(json["config"]["max_constants"].to_string(), capacity);
        let result = &json["states"][0]["exit_stack"][0];
        if capacity == "256" {
            assert_eq!(result, "Top");
        } else {
            let candidates = result["Constants"].as_array().unwrap();
            assert_eq!(candidates.len(), 257);
            for value in 0..=256 {
                assert!(candidates.contains(&Value::String(format!("0x{value:x}"))));
            }
        }
    }
}

#[test]
fn requested_explain_capacity_is_admitted_and_retains_work_frontiers() {
    let output = run(&[
        "explain",
        "--hex",
        "5f351e00",
        "--domain",
        "constants-only",
        "--max-constants",
        "1000",
    ]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("status=Incomplete"), "{text}");
    assert!(text.contains("Work"), "{text}");
    assert!(text.contains("SSA unavailable"), "{text}");
}

#[test]
fn world_entrypoint_preserves_large_capacity_and_frozen_policy() {
    let output = analyze(
        "call-return-branch",
        &[
            "--domain",
            "constants-only",
            "--max-constants",
            "1000",
            "--max-work",
            "2000000000",
            "--format",
            "json",
        ],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "Converged");
    assert_eq!(json["config"]["analysis"]["max_constants"], 1000);
    assert_eq!(json["domain_spec"]["capacity"], 1000);
    assert_eq!(json["domain_spec"]["profile"], "constants-only");
}

#[test]
fn default_capacity_stays_eight_for_program_and_world() {
    for output in [
        run(&["cfg", "--hex", "00", "--format", "json"]),
        analyze("call-return-branch", &["--format", "json"]),
    ] {
        assert!(output.status.success());
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(json["domain_spec"]["capacity"], 8);
    }
}

#[test]
fn zero_capacity_is_still_rejected_before_execution() {
    let mut outputs = ["cfg", "ssa", "explain"]
        .map(|command| run(&[command, "--hex", "00", "--max-constants", "0"]))
        .to_vec();
    outputs.push(analyze("call-return-branch", &["--max-constants", "0"]));
    for output in outputs {
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("max_constants must be positive"));
    }
}

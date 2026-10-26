//! Real raw and staged commands retain the same World and cumulative payer.
use serde_json::Value;
use std::process::{Command, Output};

const ROOT: &str = "0x1111111111111111111111111111111111111111";

fn run(bound: Option<usize>, staged: bool) -> Output {
    let path = format!(
        "{}/../../examples/worlds/nested-detail-regions.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut command = Command::new(super::native::binary());
    command.args(["analyze", "--world", &path, "--evm.to", ROOT]);
    // Preserve the original Unknown caller/value/calldata and default budgets.
    if let Some(bound) = bound {
        command.args(["--max-detail-regions", &bound.to_string()]);
    }
    if staged {
        command.arg("--staged-json");
    } else {
        command.args(["--format", "json", "--ssa"]);
    }
    command.output().unwrap()
}

fn actual_private_regions(coverage: &Value, cfg: &[Value], bound: usize) {
    assert_eq!(coverage["requested_detail_regions"], bound);
    assert_eq!(coverage["detail_regions"].as_array().unwrap().len(), bound);
    assert!(coverage["work"]["total"].as_u64().unwrap() <= 2_000_000_000);
    for index in 1..=3 {
        let address = format!("0x{}", format!("{:02x}", 0x11 * index).repeat(20));
        let frames: Vec<_> = cfg
            .iter()
            .filter(|state| {
                state["key"]["frames"].as_array().unwrap().last().unwrap()["code_address"]
                    == address
            })
            .collect();
        if index <= bound {
            assert!(
                frames.iter().any(|state| state["executed_pcs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|pc| pc == 0)),
                "bound={bound} source={address}"
            );
        } else {
            assert!(
                frames
                    .iter()
                    .all(|state| state["executed_pcs"].as_array().unwrap().is_empty())
            );
        }
    }
}

#[test]
fn detail_regions_reach_raw_and_staged_world_commands_with_unchanged_budgets() {
    for staged in [false, true] {
        for bound in [None, Some(0), Some(1), Some(2), Some(3)] {
            let output = run(bound, staged);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let expected = bound.unwrap_or(1);
            if staged {
                let lines: Vec<Value> = output
                    .stdout
                    .split(|byte| *byte == b'\n')
                    .filter(|line| !line.is_empty())
                    .map(|line| serde_json::from_slice(line).unwrap())
                    .collect();
                assert_eq!(lines.len(), 2);
                let first = &lines[0];
                assert_eq!(first["lifecycle"], "Running");
                assert_eq!(first["model_closed"], true);
                assert_eq!(
                    first["report"]["coverage"]["requested_detail_regions"],
                    expected
                );
                assert!(first["report"]["detail"].is_null());
                let final_line = &lines[1];
                assert_eq!(final_line["lifecycle"], "Final");
                for key in ["snapshot", "revision", "global_version"] {
                    assert_eq!(first[key], final_line[key]);
                }
                let report = &final_line["report"];
                assert_eq!(first["report"]["baseline"], report["baseline"]);
                assert!(report["baseline"]["transfers"].as_u64().unwrap() > 0);
                if expected == 0 {
                    assert_eq!(report["coverage"]["detail"], "Disabled");
                    assert!(report["detail"].is_null());
                } else {
                    assert!(
                        report["detail"]["transfers"].as_u64().unwrap()
                            > report["baseline"]["transfers"].as_u64().unwrap()
                    );
                    actual_private_regions(
                        &report["coverage"],
                        report["detail"]["states"].as_array().unwrap(),
                        expected,
                    );
                }
            } else {
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(report["postprocessing_max_work"], 2_000_000_000_u64);
                assert_eq!(
                    report["global"]["analysis"]["config"]["max_work"],
                    2_000_000_000_u64
                );
                assert_eq!(
                    report["global"]["analysis"]["config"]["analysis"]["max_transfers"],
                    10_000_000
                );
                let coverage = &report["hybrid"]["coverage"];
                let global_transfers = report["global"]["analysis"]["transfers"].as_u64().unwrap();
                assert!(global_transfers > 0);
                if expected == 0 {
                    assert_eq!(report["transfers"], global_transfers);
                    assert_eq!(coverage["requested_detail_regions"], 0);
                    assert_eq!(coverage["detail"], "Disabled");
                    assert!(report["native"].is_null());
                } else {
                    let native_transfers =
                        report["native"]["analysis"]["transfers"].as_u64().unwrap();
                    assert!(native_transfers > global_transfers);
                    assert_eq!(report["transfers"], native_transfers);
                    actual_private_regions(
                        coverage,
                        report["native"]["analysis"]["states"].as_array().unwrap(),
                        expected,
                    );
                    assert!(
                        !report["native"]["ssa"]["blocks"]
                            .as_array()
                            .unwrap()
                            .is_empty()
                    );
                }
            }
        }
    }
}

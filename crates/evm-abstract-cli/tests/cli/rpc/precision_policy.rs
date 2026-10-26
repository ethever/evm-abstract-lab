//! Both fixed RPC paths carry the public bound into actual callee execution.
use super::{BLOCK, CALLEE, ENTRY, Fixture, LEAF, RpcServer, assert_pinned, call};
use serde_json::Value;
use std::{process::Command, thread};

#[test]
fn detail_regions_reach_mocked_pinned_rpc_discovery_and_preloaded_execution() {
    thread::scope(|scope| {
        let root = format!("{}5000", call(0xf1, 0x200, 0));
        let child = format!("{}5000", call(0xf1, 0x300, 0));
        for discovery in [false, true] {
            for bound in [0, 1, 2, 3] {
                let fixture =
                    Fixture::new(&[(ENTRY, &root), (CALLEE, &child), (LEAF, "60ab5f5360015ff3")]);
                let server = RpcServer::new(scope, move |request| fixture.reply(request));
                let mut command = Command::new(crate::native::binary());
                command.args([
                    "analyze",
                    "--rpc",
                    &server.endpoint,
                    "--block-hash",
                    BLOCK,
                    "--evm.to",
                    ENTRY,
                    "--account",
                    CALLEE,
                    "--account",
                    LEAF,
                    "--format",
                    "json",
                    "--max-detail-regions",
                    &bound.to_string(),
                ]);
                if !discovery {
                    command.arg("--no-rpc-discovery");
                }
                let output = command.output().unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let report: Value = serde_json::from_slice(&output.stdout).unwrap();
                if discovery {
                    assert_eq!(report["transfers"], report["hybrid"]["rpc"]["transfers"]);
                } else {
                    assert!(report["hybrid"]["rpc"].is_null());
                    let latest = if report["native"].is_null() {
                        &report["global"]
                    } else {
                        &report["native"]
                    };
                    assert_eq!(report["transfers"], latest["analysis"]["transfers"]);
                }
                let original = &report["hybrid"]["coverage"];
                assert_eq!(original["requested_detail_regions"], bound);
                assert_eq!(
                    report["global"]["analysis"]["config"]["max_work"],
                    2_000_000_000_u64
                );
                let precision = if report["hybrid"]["precision"].is_null() {
                    original
                } else {
                    &report["hybrid"]["precision"]
                };
                assert_eq!(precision["requested_detail_regions"], bound);
                if bound == 0 {
                    assert!(report["native"].is_null());
                    assert_eq!(original["detail"], "Disabled");
                } else {
                    assert_eq!(precision["detail_regions"].as_array().unwrap().len(), bound);
                    let states = report["native"]["analysis"]["states"].as_array().unwrap();
                    for (index, address) in [ENTRY, CALLEE, LEAF].into_iter().enumerate() {
                        let frames: Vec<_> = states.iter().filter(|state| state["key"]["frames"].as_array().unwrap().last().unwrap()["code_address"] == address).collect();
                        if index < bound {
                            assert!(
                                frames.iter().any(|state| !state["executed_pcs"]
                                    .as_array()
                                    .unwrap()
                                    .is_empty()),
                                "discovery={discovery} bound={bound} source={address}"
                            );
                        } else {
                            assert!(
                                frames.iter().all(|state| state["executed_pcs"]
                                    .as_array()
                                    .unwrap()
                                    .is_empty())
                            );
                        }
                    }
                }
                assert_pinned(&server.finish());
            }
        }
    });
}

#[test]
fn staged_mocked_rpc_publishes_the_requested_bound_before_precision() {
    thread::scope(|scope| {
        for bound in [0, 3] {
            let root = format!("{}5000", call(0xf1, 0x200, 0));
            let child = format!("{}5000", call(0xf1, 0x300, 0));
            let fixture =
                Fixture::new(&[(ENTRY, &root), (CALLEE, &child), (LEAF, "60ab5f5360015ff3")]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let output = Command::new(crate::native::binary())
                .args([
                    "analyze",
                    "--rpc",
                    &server.endpoint,
                    "--block-hash",
                    BLOCK,
                    "--evm.to",
                    ENTRY,
                    "--account",
                    CALLEE,
                    "--account",
                    LEAF,
                    "--staged-json",
                    "--max-detail-regions",
                    &bound.to_string(),
                ])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let lines: Vec<Value> = output
                .stdout
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_slice(line).unwrap())
                .collect();
            assert_eq!(lines.len(), 2);
            for line in &lines {
                assert_eq!(
                    line["report"]["coverage"]["requested_detail_regions"],
                    bound
                );
                assert_eq!(line["model_closed"], true);
            }
            assert_eq!(lines[0]["lifecycle"], "Running");
            assert!(lines[0]["report"]["detail"].is_null());
            assert_eq!(
                lines[0]["report"]["coverage"]["detail"],
                if bound == 0 { "Disabled" } else { "Pending" }
            );
            assert_eq!(lines[1]["lifecycle"], "Final");
            if bound == 0 {
                assert!(lines[1]["precision"].is_null());
                assert!(lines[1]["native_version"].is_null());
            } else {
                assert_eq!(
                    lines[1]["precision"]["coverage"]["requested_detail_regions"],
                    3
                );
                assert_eq!(
                    lines[1]["precision"]["coverage"]["detail_regions"]
                        .as_array()
                        .unwrap()
                        .len(),
                    3
                );
                assert!(lines[1]["native_version"].is_number());
            }
            assert_pinned(&server.finish());
        }
    });
}

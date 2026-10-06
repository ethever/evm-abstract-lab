//! Pin a moving RPC head once across execution and discovered-call restarts.

use super::{BLOCK, CALLEE, ENTRY, Fixture, LEAF, Reply, RpcServer, call, requests_for};
use crate::run_concrete;
use serde_json::{Value as Json, json};
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

const MOVED_BLOCK: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";

#[test]
fn latest_number_and_hash_pin_once_for_analyze_and_explain_with_recursive_discovery() {
    thread::scope(|scope| {
        for command in ["analyze", "explain"] {
            for selector in [
                vec![],
                vec!["--block-number", "16"],
                vec!["--block-hash", BLOCK],
            ] {
                let root_code = format!("{}50{}5000", call(0xf1, 0x200, 0), call(0xf1, 0x200, 0));
                let callee_code = format!("{}5000", call(0xf1, 0x300, 0));
                let fixture = Fixture::new(&[
                    (ENTRY, &root_code),
                    (CALLEE, &callee_code),
                    (LEAF, "602a00"),
                ]);
                let resolutions = AtomicUsize::new(0);
                let server = RpcServer::new(scope, move |request| {
                    let Reply::Json(mut reply) = fixture.reply(request) else {
                        unreachable!()
                    };
                    match request["method"].as_str().unwrap() {
                        "eth_chainId" => reply["result"] = json!("0x38"),
                        "eth_getBlockByNumber" => {
                            // A second moving-tag/height lookup sees another head, including
                            // during discovery. Every state read must keep the first hash.
                            let first = resolutions.fetch_add(1, Ordering::Relaxed) == 0;
                            reply["result"] = json!({
                                "hash": if first { BLOCK } else { MOVED_BLOCK },
                                "number":"0x10",
                            });
                        }
                        "eth_getBlockByHash" => {
                            assert_eq!(request["params"], json!([BLOCK, false]))
                        }
                        _ => assert_eq!(
                            request["params"].as_array().unwrap().last().unwrap(),
                            &json!({"blockHash":BLOCK,"requireCanonical":true}),
                        ),
                    }
                    Reply::Json(reply)
                });
                let mut args = vec![command, "--rpc", &server.endpoint, "--evm.to", ENTRY];
                args.extend_from_slice(&selector);
                if command == "analyze" {
                    args.extend(["--format", "json"]);
                } else {
                    args.push("--verbose");
                }
                let output = run_concrete(&args);
                assert!(
                    output.status.success(),
                    "{args:?}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
                if command == "analyze" {
                    let analysis: Json = serde_json::from_slice(&output.stdout).unwrap();
                    assert_eq!(analysis["status"], "Converged");
                    assert_eq!(analysis["world"]["identity"]["chain_id"], "0x38");
                    assert_eq!(analysis["world"]["identity"]["block_hash"], BLOCK);
                    assert_eq!(
                        analysis["rpc_acquisition"]["fetched_accounts"],
                        json!([CALLEE, LEAF])
                    );
                    assert_eq!(analysis["rpc_acquisition"]["rounds"], 3);
                } else {
                    let explanation = String::from_utf8(output.stdout).unwrap();
                    assert!(
                        explanation.contains(BLOCK)
                            && explanation.contains(CALLEE)
                            && explanation.contains(LEAF)
                    );
                    assert!(
                        explanation.contains("Converged")
                            && explanation.contains("Verified cross-contract SSA:")
                    );
                }
                let requests = server.finish();
                let numbered: Vec<_> = requests
                    .iter()
                    .filter(|r| r["method"] == "eth_getBlockByNumber")
                    .collect();
                if selector.first() == Some(&"--block-hash") {
                    assert!(numbered.is_empty());
                } else {
                    assert_eq!(
                        numbered.len(),
                        1,
                        "head must be resolved only once: {requests:?}"
                    );
                    assert_eq!(
                        numbered[0]["params"],
                        json!([
                            if selector.is_empty() {
                                "latest"
                            } else {
                                "0x10"
                            },
                            false
                        ])
                    );
                }
                assert_eq!(
                    requests
                        .iter()
                        .filter(|r| r["method"] == "eth_getProof")
                        .count(),
                    0
                );
                for address in [ENTRY, CALLEE, LEAF] {
                    for method in ["eth_getCode", "eth_getBalance", "eth_getTransactionCount"] {
                        assert_eq!(requests_for(&requests, method, address).len(), 1);
                    }
                }
                for request in requests.iter().filter(|r| {
                    !matches!(
                        r["method"].as_str(),
                        Some("eth_chainId" | "eth_getBlockByNumber" | "eth_getBlockByHash")
                    )
                }) {
                    assert_eq!(
                        request["params"].as_array().unwrap().last().unwrap(),
                        &json!({"blockHash":BLOCK,"requireCanonical":true})
                    );
                }
            }
        }
    });
}

#[test]
fn rpc_identity_and_execution_chain_id_are_independent_with_symbolic_call_defaults() {
    thread::scope(|scope| {
        for no_discovery in [false, true] {
            for execution_chain in [None, Some("1")] {
                let fixture = Fixture::new(&[(ENTRY, "4633321400")]);
                let server = RpcServer::new(scope, move |request| {
                    let Reply::Json(mut reply) = fixture.reply(request) else {
                        unreachable!()
                    };
                    if request["method"] == "eth_chainId" {
                        reply["result"] = json!("0x38");
                    }
                    Reply::Json(reply)
                });
                let mut args = vec![
                    "analyze",
                    "--rpc",
                    &server.endpoint,
                    "--evm.to",
                    ENTRY,
                    "--format",
                    "json",
                ];
                if let Some(chain) = execution_chain {
                    args.extend(["--evm.chain-id", chain]);
                }
                if no_discovery {
                    args.push("--no-rpc-discovery");
                }
                let output = std::process::Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
                    .args(&args)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let analysis: Json = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(analysis["world"]["identity"]["chain_id"], "0x38");
                assert_eq!(
                    analysis["states"][0]["exit_stack"][0]["Constants"],
                    json!([if execution_chain.is_some() {
                        "0x1"
                    } else {
                        "0x38"
                    }])
                );
                assert_eq!(
                    analysis["states"][0]["exit_stack"][1]["Constants"],
                    json!(["0x1"])
                );
                assert_eq!(
                    analysis["entry"]["environment"]["caller"],
                    json!({"Symbolic":"Caller"})
                );
                assert!(analysis["entry"]["environment"]["value"]["Constants"].is_null());
                assert!(
                    analysis["entry"]["environment"]["calldata"]["length"]["Constants"].is_null()
                );
                let requests = server.finish();
                assert_eq!(
                    requests
                        .iter()
                        .filter(|r| r["method"] == "eth_getBlockByNumber")
                        .count(),
                    1
                );
                assert!(requests.iter().all(|r| r["method"] != "eth_getProof"));
            }
        }
    });
}

//! The real CLI refines finite SLOAD keys without an explicit --slot flag.

use super::{
    BLOCK, CALLEE, CALLER, ENTRY, Fixture, LEAF, Reply, RpcServer,
    assert_pinned as assert_account_pinned, byte_contains, call, contains, execute, requests_for,
    result, verified_export,
};
use alloy_primitives::U256;
use serde_json::{Value as Json, json};
use std::{
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

fn storage_reply(request: &Json, value: U256) -> Reply {
    Reply::Json(json!({
        "jsonrpc":"2.0",
        "id":request["id"],
        "result":format!("{value:#066x}"),
    }))
}

fn assert_pinned(requests: &[Json]) {
    let mut account_requests = Vec::new();
    for request in requests {
        if request["method"] == "eth_getBlockByNumber" {
            assert_eq!(request["params"], json!(["0x10", false]));
        } else {
            account_requests.push(request.clone());
        }
    }
    assert_account_pinned(&account_requests);
}

fn storage_failure(analysis: &Json, address: &str, slot: U256) -> Json {
    assert_eq!(analysis["status"], "Incomplete");
    let failure = analysis["frontiers"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|frontier| {
            let acquisition = &frontier["reason"]["RpcAcquisition"];
            (acquisition["address"] == address
                && acquisition["failure"]["context"]["slot"] == json!(slot))
            .then(|| acquisition["failure"].clone())
        })
        .unwrap_or_else(|| panic!("missing storage acquisition failure: {analysis}"));
    assert_eq!(failure["context"]["chain_id"], "0x1");
    assert_eq!(failure["context"]["block_hash"], BLOCK);
    assert_eq!(failure["context"]["account"], address);
    assert!(!failure["message"].as_str().unwrap().is_empty());
    assert_eq!(
        analysis["rpc_acquisition"]["failed_storage"],
        json!([{"address":address,"slot":slot}])
    );
    assert!(
        analysis["rpc_acquisition"]["failures"]
            .as_array()
            .unwrap()
            .iter()
            .any(|evidence| evidence["address"] == address
                && evidence["slot"] == json!(slot)
                && evidence["failure"] == failure)
    );
    assert!(
        analysis["world"]["accounts"][address]["storage"]
            .get(format!("{slot:#x}"))
            .is_none()
    );
    failure
}

#[test]
fn default_rpc_sload_fetches_slot_zero_and_generates_verified_ssa() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" {
                storage_reply(request, U256::from(42))
            } else {
                fixture.reply(request)
            }
        });
        let export = result(execute(&server, &["--ssa"]), 0);
        let analysis = verified_export(&export);
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            state["exit_stack"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| contains(value, U256::from(42)))
        }));
        assert_eq!(
            analysis["rpc_acquisition"]["fetched_storage"],
            json!([{"address":ENTRY,"slot":"0x0"}])
        );
        let requests = server.finish();
        let reads = requests_for(&requests, "eth_getStorageAt", ENTRY);
        assert_eq!(reads.len(), 1);
        assert_eq!(
            reads[0]["params"],
            json!([ENTRY,"0x0",{"blockHash":BLOCK,"requireCanonical":true}])
        );
        assert_pinned(&requests);
    });
}

#[test]
fn disabled_discovery_keeps_unselected_storage_unknown_without_requests() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &["--no-rpc-discovery"]), 0);
        assert_eq!(analysis["status"], "Converged");
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            state["exit_stack"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.get("Constants").is_none())
        }));
        let requests = server.finish();
        assert!(requests_for(&requests, "eth_getStorageAt", ENTRY).is_empty());
        assert_pinned(&requests);
    });
}

#[test]
fn explicit_slot_and_fetched_zero_are_cached_across_repeated_sloads() {
    thread::scope(|scope| {
        let slot = format!("{ENTRY}:0");
        for (preselected, value) in [
            (false, U256::ZERO),
            (true, U256::ZERO),
            (true, U256::from(42)),
        ] {
            let fixture = Fixture::new(&[(ENTRY, "5f54505f5400")]);
            let server = RpcServer::new(scope, move |request| {
                if request["method"] == "eth_getStorageAt" {
                    storage_reply(request, value)
                } else {
                    fixture.reply(request)
                }
            });
            let extra = if preselected {
                vec!["--slot", slot.as_str()]
            } else {
                vec![]
            };
            let analysis = result(execute(&server, &extra), 0);
            assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
                state["exit_stack"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|observed| contains(observed, value))
            }));
            assert_eq!(
                analysis["rpc_acquisition"]["fetched_storage"],
                if preselected {
                    json!([])
                } else {
                    json!([{"address":ENTRY,"slot":"0x0"}])
                }
            );
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
            assert_pinned(&requests);
        }
    });
}

#[test]
fn nested_delegatecall_fetches_proxy_storage_instead_of_implementation_storage() {
    thread::scope(|scope| {
        let entry_code = format!("{}5000", call(0xf4, 0x200, 0));
        let implementation_code = format!("{}5000", call(0xf4, 0x300, 0));
        let fixture = Fixture::new(&[
            (ENTRY, &entry_code),
            (CALLEE, &implementation_code),
            (LEAF, "5f5400"),
        ]);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" {
                assert_eq!(request["params"][0], ENTRY);
                storage_reply(request, U256::from(42))
            } else {
                fixture.reply(request)
            }
        });
        let export = result(execute(&server, &["--ssa"]), 0);
        let analysis = verified_export(&export);
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
            frame["code_address"] == LEAF
                && frame["address"] == ENTRY
                && state["exit_stack"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|value| contains(value, U256::from(42)))
        }));
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
        for implementation in [CALLEE, LEAF] {
            assert!(requests_for(&requests, "eth_getStorageAt", implementation).is_empty());
        }
        assert_pinned(&requests);
    });
}

#[test]
fn child_revert_preserves_fetched_snapshot_and_caller_strong_write() {
    thread::scope(|scope| {
        for written in [false, true] {
            let prefix = if written { "60075f55" } else { "" };
            let entry_code = format!("{prefix}{}505f545f5260205ff3", call(0xf4, 0x200, 0));
            let fixture = Fixture::new(&[(ENTRY, &entry_code), (CALLEE, "5f545060015f555f5ffd")]);
            let server = RpcServer::new(scope, move |request| {
                if request["method"] == "eth_getStorageAt" {
                    storage_reply(request, U256::from(42))
                } else {
                    fixture.reply(request)
                }
            });
            let export = result(execute(&server, &["--ssa"]), 0);
            let analysis = verified_export(&export);
            assert!(
                analysis["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|edge| edge["kind"] == "Revert")
            );
            assert!(
                analysis["outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|outcome| outcome["kind"] == "Return"
                        && byte_contains(&outcome["data"], 31, if written { 7 } else { 42 }))
            );
            let requests = server.finish();
            assert_eq!(
                requests_for(&requests, "eth_getStorageAt", ENTRY).len(),
                usize::from(!written)
            );
            assert_pinned(&requests);
        }
    });
}

fn symbolic_calldata(server: &RpcServer, extra: &[&str]) -> std::process::Output {
    let mut args = vec![
        "analyze",
        "--rpc",
        &server.endpoint,
        "--block-hash",
        BLOCK,
        "--evm.to",
        ENTRY,
        "--evm.value",
        "0",
        "--evm.caller",
        CALLER,
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn finite_keys_are_fetched_once_and_unbounded_keys_are_not_enumerated() {
    thread::scope(|scope| {
        for (code, expected_slots) in [("5f356001165400", vec!["0x0", "0x1"]), ("5f355400", vec![])]
        {
            let fixture = Fixture::new(&[(ENTRY, code)]);
            let server = RpcServer::new(scope, move |request| {
                if request["method"] == "eth_getStorageAt" {
                    let slot: U256 = request["params"][1].as_str().unwrap().parse().unwrap();
                    storage_reply(request, slot + U256::from(42))
                } else {
                    fixture.reply(request)
                }
            });
            let analysis = result(symbolic_calldata(&server, &[]), 0);
            assert_eq!(analysis["status"], "Converged");
            if !expected_slots.is_empty() {
                assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
                    state["exit_stack"].as_array().unwrap().iter().any(|value| {
                        contains(value, U256::from(42)) && contains(value, U256::from(43))
                    })
                }));
            }
            let requests = server.finish();
            let reads = requests_for(&requests, "eth_getStorageAt", ENTRY);
            let slots: Vec<_> = reads
                .iter()
                .map(|request| request["params"][1].as_str().unwrap())
                .collect();
            assert_eq!(slots, expected_slots);
            assert!(requests.len() <= 14);
            assert_pinned(&requests);
        }
    });
}

#[test]
fn late_storage_failures_are_typed_and_cannot_export_complete_ssa() {
    thread::scope(|scope| {
        for case in ["remote", "short", "long", "null", "http", "json"] {
            let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
            let server = RpcServer::new(scope, move |request| {
                if request["method"] != "eth_getStorageAt" {
                    return fixture.reply(request);
                }
                match case {
                    "remote" => Reply::Json(
                        json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32000,"message":"historical storage unavailable"}}),
                    ),
                    "short" => {
                        Reply::Json(json!({"jsonrpc":"2.0","id":request["id"],"result":"0x2a"}))
                    }
                    "long" => Reply::Json(
                        json!({"jsonrpc":"2.0","id":request["id"],"result":format!("0x{}", "00".repeat(33))}),
                    ),
                    "null" => {
                        Reply::Json(json!({"jsonrpc":"2.0","id":request["id"],"result":null}))
                    }
                    "http" => Reply::Http(503),
                    "json" => Reply::Bytes(b"not JSON".to_vec()),
                    _ => unreachable!(),
                }
            });
            let analysis = result(execute(&server, &["--ssa"]), 2);
            assert!(analysis.get("ssa").is_none());
            let failure = storage_failure(&analysis, ENTRY, U256::ZERO);
            assert_eq!(failure["context"]["method"], "eth_getStorageAt");
            assert_eq!(analysis["rpc_acquisition"]["fetched_storage"], json!([]));
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
            assert_pinned(&requests);
        }
    });
}

#[test]
fn failed_finite_slot_batch_keeps_affected_keys_and_actual_failure_slot() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f356001165400")]);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] != "eth_getStorageAt" {
                return fixture.reply(request);
            }
            if request["params"][1] == "0x0" {
                return storage_reply(request, U256::from(42));
            }
            assert_eq!(request["params"][1], "0x1");
            Reply::Json(json!({
                "jsonrpc":"2.0",
                "id":request["id"],
                "error":{"code":-32000,"message":"slot one unavailable"},
            }))
        });
        let analysis = result(symbolic_calldata(&server, &["--ssa"]), 2);
        assert_eq!(analysis["status"], "Incomplete");
        assert!(analysis.get("ssa").is_none());
        assert_eq!(analysis["rpc_acquisition"]["fetched_storage"], json!([]));
        assert_eq!(
            analysis["rpc_acquisition"]["failed_storage"],
            json!([
                {"address":ENTRY,"slot":"0x0"},
                {"address":ENTRY,"slot":"0x1"},
            ])
        );
        let failures = analysis["rpc_acquisition"]["failures"].as_array().unwrap();
        assert_eq!(failures.len(), 2);
        for (failure, affected_slot) in failures.iter().zip(["0x0", "0x1"]) {
            assert_eq!(failure["address"], ENTRY);
            assert_eq!(failure["slot"], affected_slot);
            assert_eq!(failure["failure"]["context"]["slot"], "0x1");
            assert_eq!(failure["failure"]["context"]["method"], "eth_getStorageAt");
        }
        assert!(
            analysis["world"]["accounts"][ENTRY]["storage"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 2);
        assert_pinned(&requests);
    });
}

#[test]
fn changed_block_during_storage_fetch_leaves_slot_context_and_no_committed_value() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
        let block_checks = AtomicUsize::new(0);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" {
                return storage_reply(request, U256::from(42));
            }
            let Reply::Json(mut reply) = fixture.reply(request) else {
                unreachable!()
            };
            // The provider can still serve the old hash after a reorg. Only
            // the fixed height's canonical hash changes before installation.
            if request["method"] == "eth_getBlockByNumber"
                && block_checks.fetch_add(1, Ordering::Relaxed) == 1
            {
                reply["result"]["hash"] =
                    json!("0x2222222222222222222222222222222222222222222222222222222222222222");
            }
            Reply::Json(reply)
        });
        let analysis = result(execute(&server, &["--ssa"]), 2);
        let failure = storage_failure(&analysis, ENTRY, U256::ZERO);
        assert_eq!(failure["kind"], "block_mismatch");
        assert_eq!(failure["context"]["method"], "eth_getBlockByNumber");
        assert_eq!(analysis["rpc_acquisition"]["fetched_storage"], json!([]));
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["method"] == "eth_getBlockByNumber")
                .count(),
            2
        );
        assert!(
            requests
                .iter()
                .any(|request| request["method"] == "eth_getBlockByHash")
        );
        assert_pinned(&requests);
    });
}

#[test]
fn request_budget_remains_cumulative_when_late_storage_is_needed() {
    thread::scope(|scope| {
        for (limit, storage_reads) in [("7", 0), ("10", 1)] {
            let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
            let server = RpcServer::new(scope, move |request| {
                if request["method"] == "eth_getStorageAt" {
                    storage_reply(request, U256::from(42))
                } else {
                    fixture.reply(request)
                }
            });
            let analysis = result(execute(&server, &["--max-rpc-requests", limit]), 2);
            let failure = storage_failure(&analysis, ENTRY, U256::ZERO);
            assert_eq!(failure["kind"], "acquisition_limit");
            assert_eq!(failure["resource"], "requests");
            let requests = server.finish();
            assert_eq!(requests.len(), limit.parse::<usize>().unwrap());
            assert_eq!(
                requests_for(&requests, "eth_getStorageAt", ENTRY).len(),
                storage_reads
            );
            assert_eq!(
                analysis["rpc_acquisition"]["requests"].as_u64().unwrap() as usize,
                requests.len()
            );
            assert_pinned(&requests);
        }
    });
}

#[test]
fn storage_refinement_does_not_reset_execution_transfer_budget() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f5460015400")]);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" {
                storage_reply(request, U256::from(42))
            } else {
                fixture.reply(request)
            }
        });
        let analysis = result(execute(&server, &["--max-transfers", "2", "--ssa"]), 2);
        assert_eq!(analysis["status"], "Incomplete");
        assert!(analysis.get("ssa").is_none());
        assert!(
            analysis["frontiers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|frontier| { frontier["reason"]["Budget"] == "Transfers" })
        );
        assert_eq!(analysis["transfers"], 2);
        assert_eq!(analysis["rpc_acquisition"]["rounds"], 2);
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
        assert_pinned(&requests);
    });
}

#[test]
fn rpc_explain_uses_the_same_default_slot_discovery_and_verified_ssa() {
    thread::scope(|scope| {
        let fixture = Fixture::new(&[(ENTRY, "5f5400")]);
        let server = RpcServer::new(scope, move |request| {
            if request["method"] == "eth_getStorageAt" {
                storage_reply(request, U256::from(42))
            } else {
                fixture.reply(request)
            }
        });
        let output = crate::run_concrete(&[
            "explain",
            "--rpc",
            &server.endpoint,
            "--block-hash",
            BLOCK,
            "--evm.to",
            ENTRY,
            "--verbose",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let explanation = String::from_utf8(output.stdout).unwrap();
        assert!(explanation.contains("Converged"));
        assert!(explanation.contains("Verified cross-contract SSA:"));
        assert!(explanation.contains("0x2a"));
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getStorageAt", ENTRY).len(), 1);
        assert_pinned(&requests);
    });
}

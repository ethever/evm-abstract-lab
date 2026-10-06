//! Actual binary acceptance for reusable summaries, lifecycle, native calls and
//! explicit RPC failure. Assertions correlate each observable terminal bundle.

use alloy_primitives::{Address, U256, keccak256};
use serde_json::Value as Json;
use std::{
    collections::BTreeSet,
    net::TcpListener,
    process::{Command, Output},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const CREATED: &str = "0xea53a153a9a04fd632b2486d84732feb3b71afb7";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

fn run_concrete(args: &[&str]) -> Output {
    let mut scoped = args.to_vec();
    if args.iter().any(|arg| matches!(*arg, "--world" | "--rpc")) {
        for (flag, value) in [
            ("--evm.value", "0"),
            ("--evm.calldata", "0x"),
            ("--evm.caller", "0x0000000000000000000000000000000000001000"),
        ] {
            if !args.iter().any(|arg| arg.split('=').next() == Some(flag)) {
                scoped.extend([flag, value]);
            }
        }
    }
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .args(scoped)
        .output()
        .unwrap()
}

fn fixture(name: &str, extra: &[&str]) -> Json {
    let path = format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut args = vec![
        "analyze",
        "--world",
        path.as_str(),
        "--evm.to",
        ENTRY,
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    let output = run_concrete(&args);
    assert!(
        output.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn contains(value: &Json, expected: U256) -> bool {
    value["Constants"].as_array().is_some_and(|values| {
        values.iter().any(|value| {
            value.as_str().and_then(|value| value.parse::<U256>().ok()) == Some(expected)
        })
    })
}

fn byte_contains(data: &Json, offset: usize, expected: u8) -> bool {
    let value = data["bytes"]
        .get(offset.to_string())
        .unwrap_or(&data["default"]);
    contains(value, U256::from(expected))
}

fn account<'a>(store: &'a Json, address: &str) -> Option<&'a Json> {
    store["account_observations"]
        .as_array()?
        .iter()
        .find(|account| account["address"] == address)
}

fn slot_contains(store: &Json, address: &str, slot: u64, expected: U256) -> bool {
    store["persistent"]["slots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|observation| {
            observation["address"] == address
                && observation["slot"]
                    .as_str()
                    .and_then(|value| value.parse::<U256>().ok())
                    == Some(U256::from(slot))
                && contains(&observation["value"], expected)
        })
}

fn relations(analysis: &Json) -> BTreeSet<String> {
    analysis["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|outcome| {
            serde_json::to_string(&serde_json::json!([
                outcome["kind"],
                outcome["data"],
                outcome["store"]
            ]))
            .unwrap()
        })
        .collect()
}

fn verify_export(json: &Json) -> &Json {
    let analysis = &json["analysis"];
    assert_eq!(analysis["status"], "Converged");
    assert_eq!(
        json["ssa"]["blocks"].as_array().unwrap().len(),
        analysis["states"].as_array().unwrap().len()
    );
    assert_eq!(
        json["ssa"]["transitions"].as_array().unwrap().len(),
        analysis["edges"].as_array().unwrap().len()
    );
    assert!(!analysis["edges"].as_array().unwrap().is_empty());
    analysis
}

#[test]
fn summary_switch_preserves_real_graph_ssa_identity_and_joint_outcomes() {
    let enabled = fixture("summary-reuse", &["--ssa"]);
    let analysis = verify_export(&enabled);
    assert_eq!(analysis["world"]["identity"]["kind"], "offline");
    assert_eq!(analysis["world"]["identity"]["label"], "summary-reuse:v1");
    let fingerprint = analysis["world"]["fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 66);
    assert!(analysis["summary_stats"]["hits"].as_u64().unwrap() > 0);
    let records = analysis["summaries"].as_array().unwrap();
    assert!(records.iter().any(|record| {
        !record["reused_at"].as_array().unwrap().is_empty()
            && record["input"]["world_fingerprint"] == fingerprint
            && record["outputs"].as_array().unwrap().iter().any(|output| {
                output["kind"] == "Return" && contains(&output["data"]["length"], U256::from(32))
            })
    }));
    assert!(
        analysis["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|edge| edge["kind"] == "Call")
            .count()
            >= 2
    );
    assert!(
        analysis["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| outcome["kind"] == "Return"
                && contains(&outcome["data"]["length"], U256::from(32))
                && byte_contains(&outcome["data"], 31, 7))
    );
    let disabled = fixture("summary-reuse", &["--ssa", "--no-summaries"]);
    let baseline = verify_export(&disabled);
    assert_eq!(baseline["config"]["use_summaries"], false);
    assert_eq!(baseline["summary_stats"]["hits"], 0);
    assert_eq!(baseline["summary_stats"]["published"], 0);
    assert!(baseline["summaries"].as_array().unwrap().is_empty());
    assert_eq!(relations(analysis), relations(baseline));
}

#[test]
fn creation_exports_initcode_captured_runtime_and_one_correlated_commit() {
    let result = fixture("create-runtime", &["--ssa"]);
    let analysis = verify_export(&result);
    let runtime = alloy_primitives::hex::decode("602a5f5260205ff3").unwrap();
    let runtime_hash = serde_json::to_value(keccak256(&runtime)).unwrap();
    let states = analysis["states"].as_array().unwrap();
    assert!(states.iter().any(
        |state| state["key"]["frames"].as_array().unwrap().last().unwrap()["mode"] == "InitCode"
    ));
    assert!(states.iter().any(|state| {
        let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
        frame["address"] == CREATED
            && frame["mode"] == "Runtime"
            && frame["code_hash"] == runtime_hash
            && !state["executed_pcs"].as_array().unwrap().is_empty()
    }));
    assert!(
        analysis["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| {
                if outcome["kind"] != "Return" {
                    return false;
                }
                let store = &outcome["store"];
                let Some(created) = account(store, CREATED) else {
                    return false;
                };
                let Some(creator) = account(store, ENTRY) else {
                    return false;
                };
                created["existence"] == "present"
                    && created["code_hash"] == runtime_hash
                    && created["code_size"] == 8
                    && contains(&created["nonce"], U256::from(1))
                    && contains(&creator["nonce"], U256::from(1))
                    && slot_contains(
                        store,
                        ENTRY,
                        0,
                        U256::from_be_slice(CREATED.parse::<Address>().unwrap().as_slice()),
                    )
                    && slot_contains(store, ENTRY, 1, U256::from(1))
                    && contains(&outcome["data"]["length"], U256::from(32))
                    && byte_contains(&outcome["data"], 31, 42)
            }),
        "no correlated created runtime/nonce/storage/return commit: {}",
        analysis["outcomes"]
    );
}

#[test]
fn native_identity_call_exports_its_frame_return_data_and_effect_ssa() {
    let result = fixture("identity-precompile", &["--ssa"]);
    let analysis = verify_export(&result);
    assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
        state["key"]["frames"].as_array().unwrap().last().unwrap()["mode"]["Precompile"]
            == "0x0000000000000000000000000000000000000004"
    }));
    assert!(
        analysis["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| {
                outcome["kind"] == "Return"
                    && contains(&outcome["data"]["length"], U256::from(32))
                    && byte_contains(&outcome["data"], 31, 42)
            })
    );
}

#[test]
fn explicit_rpc_connection_error_keeps_selected_hash_without_inventing_chain_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let output = run_concrete(&[
        "analyze",
        "--rpc",
        &endpoint,
        "--block-hash",
        BLOCK,
        "--evm.to",
        ENTRY,
        "--ssa",
        "--format",
        "json",
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("RPC transport failure")
            && error.contains("eth_chainId")
            && !error.contains("chain 0x1")
            && error.contains(BLOCK),
        "{error}"
    );
    assert!(!error.contains("Converged"));
    let output = run_concrete(&["analyze", "--rpc", &endpoint]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

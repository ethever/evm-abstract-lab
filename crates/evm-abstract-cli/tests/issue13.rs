//! Actual binary acceptance for reusable summaries, lifecycle, native calls and
//! explicit RPC failure. Assertions correlate each observable terminal bundle.

use alloy_primitives::{Address, U256, keccak256};
use evm_abstract::domain::{EvmValue, NumericValue};
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
    Command::new(crate::native::binary())
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
    native::select(&serde_json::from_slice(&output.stdout).unwrap())
}

fn contains(value: &Json, expected: U256) -> bool {
    value["numeric"]["constants"]
        .as_array()
        .is_some_and(|values| {
            values.iter().any(|value| {
                value.as_str().and_then(|value| value.parse::<U256>().ok()) == Some(expected)
            })
        })
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

#[path = "support/native.rs"]
mod native;
#[path = "support/native_bytes.rs"]
mod native_bytes;

fn relations(analysis: &Json) -> BTreeSet<String> {
    // One source bijection must cover the input and every joint output/store.
    let joint = native::canonical(&serde_json::json!([
        analysis["world"],
        analysis["entry"],
        analysis["outcomes"]
    ]));
    joint[2]
        .as_array()
        .unwrap()
        .iter()
        .map(|outcome| {
            serde_json::to_string(&serde_json::json!([
                joint[0],
                joint[1],
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
    assert!(native::root(&analysis["world"]["storage_source"]).is_object());
    assert_eq!(json["global_status"], "Converged");
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
    assert_eq!(json["ssa"]["complete"], true);
    let states: BTreeSet<_> = analysis["states"]
        .as_array()
        .unwrap()
        .iter()
        .map(|state| state["id"].as_u64().unwrap())
        .collect();
    let blocks: BTreeSet<_> = json["ssa"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| block["state"].as_u64().unwrap())
        .collect();
    assert_eq!(states.len(), analysis["states"].as_array().unwrap().len());
    assert_eq!(
        blocks.len(),
        json["ssa"]["blocks"].as_array().unwrap().len()
    );
    assert_eq!(blocks, states);
    for block in json["ssa"]["blocks"].as_array().unwrap() {
        let state = analysis["states"]
            .as_array()
            .unwrap()
            .iter()
            .find(|state| state["id"] == block["state"])
            .expect("SSA block belongs to this Native graph");
        assert_eq!(
            block["effect"]["inputs"].as_array().unwrap().len(),
            analysis["edges"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|edge| edge["to"] == block["state"])
                .count(),
            "SSA effect inputs match incoming Native edges"
        );
        let instructions = block["instructions"].as_array().unwrap();
        assert_eq!(
            instructions
                .iter()
                .map(|instruction| &instruction["pc"])
                .collect::<Vec<_>>(),
            state["executed_pcs"]
                .as_array()
                .unwrap()
                .iter()
                .collect::<Vec<_>>()
        );
        let mut effect = block["effect"]["result"].as_u64().unwrap();
        for instruction in instructions {
            assert_eq!(instruction["effect_input"], effect);
            let next = instruction["effect_result"].as_u64().unwrap();
            assert_ne!(next, effect);
            effect = next;
        }
        assert_eq!(block["exit_effect"], effect);
    }
    let mut edges = BTreeSet::new();
    for transition in json["ssa"]["transitions"].as_array().unwrap() {
        let index = transition["edge"].as_u64().unwrap();
        assert!(edges.insert(index), "SSA transitions cover each edge once");
        let edge = &analysis["edges"][index as usize];
        assert_eq!(transition["kind"], edge["kind"]);
        let source = json["ssa"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["state"] == edge["from"])
            .unwrap();
        let target = json["ssa"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["state"] == edge["to"])
            .unwrap();
        assert_eq!(transition["effect_input"], source["exit_effect"]);
        let effect = transition["effect_result"].as_u64().unwrap();
        assert!(
            target["effect"]["inputs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|input| { input["transition"] == index && input["effect"] == effect })
        );
    }
    analysis
}

fn assert_policy(report: &Json, regions: u64) {
    let entry = report["analysis"]["entry"]
        .as_object()
        .filter(|entry| !entry.is_empty())
        .expect("nonempty original physical entry exists");
    assert_eq!(
        entry["binding"],
        serde_json::json!({"Physical": {"address": ENTRY}})
    );
    assert_eq!(entry["address"], ENTRY);
    let environment = entry["environment"]
        .as_object()
        .filter(|environment| !environment.is_empty())
        .expect("nonempty original entry environment exists");
    assert_eq!(environment["to"], serde_json::json!({"Concrete": ENTRY}));
    assert_eq!(
        environment["caller"],
        serde_json::json!({
            "Concrete": "0x0000000000000000000000000000000000001000"
        })
    );
    assert_eq!(environment["is_static"], false);
    let zero = serde_json::to_value(EvmValue::<NumericValue>::constant(U256::ZERO)).unwrap();
    assert_eq!(environment["value"], zero);
    let calldata = &environment["calldata"];
    assert_eq!(calldata["length"], zero);
    let root = &calldata["root"];
    assert!(!root["identity_scope"].as_str().unwrap().is_empty());
    assert_eq!(native::root(root), &serde_json::json!({"BytesExact": []}));
    let coverage = &report["hybrid"]["coverage"];
    assert_eq!(coverage["requested_detail_regions"], regions);
    assert_eq!(coverage["baseline_current"], true);
    assert_eq!(coverage["detail_current"], true);
    assert_eq!(report["analysis"]["config"]["max_work"], 2_000_000_000_u64);
    let work = &coverage["work"];
    let total: u64 = ["prior", "baseline", "preparation", "detail"]
        .iter()
        .map(|field| work[field].as_u64().unwrap())
        .sum();
    assert_eq!(work["total"], total);
    assert_eq!(report["analysis"]["work"], total);
    assert!(total <= 2_000_000_000);
}

fn same_frame(left: &Json, right: &Json) -> bool {
    [
        "code_address",
        "code_hash",
        "mode",
        "address",
        "address_value",
        "caller",
        "is_static",
    ]
    .iter()
    .all(|field| !left[field].is_null() && left[field] == right[field])
}

// Relative Native execution has real visited-block receipts, not private PC or
// SSA inventories. Its body input revision is independent of the entry clock.
fn relative_runtime<'a>(analysis: &'a Json, address: &str, hash: &Json) -> Option<&'a Json> {
    analysis["procedure_summaries"]
        .as_array()?
        .iter()
        .find_map(|record| {
            if record["kind"] != "KnownCall"
                || record["closed"] != true
                || record["source_current"] != true
            {
                return None;
            }
            let procedures = record["procedures"].as_array()?;
            let index = record["root_procedure"].as_u64()?;
            let procedure = procedures.get(index as usize)?;
            let namespace = record["version"]["namespace"].as_u64()?;
            if record["version"]["generation"].as_u64()? == 0
                || procedure["version"]["namespace"].as_u64() != Some(namespace)
            {
                return None;
            }
            let source_id = record["source_state"].as_u64()?;
            let source = analysis["states"]
                .as_array()?
                .iter()
                .find(|state| state["id"].as_u64() == Some(source_id))?;
            if source["key"]["frames"].as_array()?.last()? != &procedure["frame"] {
                return None;
            }
            let frame = &procedure["frame"];
            if procedure["semantics"] != "Native"
                || !procedure["process_input"].is_null()
                || frame["mode"] != "Runtime"
                || frame["code_address"] != address
                || frame["address"] != address
                || frame["address_value"]["Concrete"] != address
                || frame["code_hash"] != *hash
                || frame["caller"]["Concrete"] != ENTRY
                || frame["is_static"] != false
            {
                return None;
            }
            let covered = procedure["covered_blocks"].as_array()?;
            record["regional_cfg"]["nodes"]
                .as_array()?
                .iter()
                .any(|node| {
                    let transfer = &node["transfer"];
                    let body_revision = node["input_revision"].as_u64();
                    node["procedure"] == index
                        && node["excluded"] == false
                        && same_frame(&node["frame"], frame)
                        && node["frame"]["basic_block_index"].as_u64().is_some()
                        && covered.contains(&node["frame"]["basic_block_index"])
                        && transfer["semantics"] == "Native"
                        && transfer["process"].is_null()
                        && body_revision.is_some()
                        && body_revision == transfer["input_revision"].as_u64()
                        && transfer["current"] == true
                        && transfer["complete"] == true
                        && transfer["visited_block"] == true
                })
                .then_some(procedure)
        })
}

fn verify_summary_switch(regions: u64) {
    let region_arg = regions.to_string();
    let mut extra = vec!["--ssa"];
    if regions == 2 {
        extra.extend(["--max-detail-regions", region_arg.as_str()]);
    }
    let enabled = fixture("summary-reuse", &extra);
    assert_policy(&enabled, regions);
    let analysis = verify_export(&enabled);
    assert_eq!(analysis["world"]["identity"]["kind"], "offline");
    assert_eq!(analysis["world"]["identity"]["label"], "summary-reuse:v1");
    let fingerprint = analysis["world"]["fingerprint"].as_str().unwrap();
    assert_eq!(fingerprint.len(), 66);
    if regions == 2 {
        assert!(analysis["summary_stats"]["hits"].as_u64().unwrap() > 0);
        let records = analysis["summaries"].as_array().unwrap();
        assert!(records.iter().any(|record| {
            !record["reused_at"].as_array().unwrap().is_empty()
                && record["input"]["world_fingerprint"] == fingerprint
                && record["outputs"].as_array().unwrap().iter().any(|output| {
                    output["kind"] == "Return"
                        && contains(&output["data"]["length"], U256::from(32))
                })
        }));
    } else {
        assert_eq!(analysis["summary_stats"]["hits"], 0);
    }
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
                && native_bytes::word_matches(&outcome["data"], U256::from(7)))
    );
    extra.push("--no-summaries");
    let disabled = fixture("summary-reuse", &extra);
    assert_policy(&disabled, regions);
    let baseline = verify_export(&disabled);
    assert_eq!(baseline["config"]["use_summaries"], false);
    assert_eq!(baseline["summary_stats"]["hits"], 0);
    assert_eq!(baseline["summary_stats"]["published"], 0);
    assert!(baseline["summaries"].as_array().unwrap().is_empty());
    if regions == 1 {
        let hash = serde_json::json!(keccak256(
            alloy_primitives::hex::decode("5f545f5260205ff3").unwrap()
        ));
        for current in [analysis, baseline] {
            let callee =
                relative_runtime(current, "0x0000000000000000000000000000000000000200", &hash)
                    .expect("default Native callee has current, executed relative evidence");
            assert!(callee["outputs"].as_array().unwrap().iter().any(|output| {
                output["kind"] == "Return"
                    && contains(&output["data"]["length"], U256::from(32))
                    && output["data"]["content"]["Exact"]
                        == serde_json::json!(U256::from(7).to_be_bytes::<32>())
            }));
        }
    }
    assert_eq!(relations(analysis), relations(baseline));
    let mut foreign = baseline.clone();
    foreign["world"]["storage_source"]["identity_scope"] = serde_json::json!("foreign-world");
    assert_ne!(
        relations(analysis),
        relations(&foreign),
        "input and output storage sources must share one bijection"
    );
}

#[test]
fn summary_switch_preserves_real_graph_ssa_identity_and_joint_outcomes() {
    verify_summary_switch(1);
}

#[test]
fn summary_switch_two_region_full_cache_preserves_joint_outcomes() {
    // Same fixture, environment and shared total work; only this additional
    // profile requests private states for the captured callee as well.
    verify_summary_switch(2);
}

#[test]
fn creation_exports_initcode_captured_runtime_and_one_correlated_commit() {
    let result = fixture("create-runtime", &["--ssa"]);
    assert_policy(&result, 1);
    let analysis = verify_export(&result);
    let runtime = alloy_primitives::hex::decode("602a5f5260205ff3").unwrap();
    let runtime_hash = serde_json::to_value(keccak256(&runtime)).unwrap();
    let states = analysis["states"].as_array().unwrap();
    assert!(states.iter().any(|state| {
        let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
        frame["mode"] == "InitCode"
            && frame["address"] == CREATED
            && frame["code_address"] == CREATED
            && frame["code_hash"]
                == serde_json::json!(keccak256(
                    alloy_primitives::hex::decode("61000861000d5f396100085ff3602a5f5260205ff3")
                        .unwrap()
                ))
            && state["entry"]["store"]["created"][CREATED] == true
            && !state["executed_pcs"].as_array().unwrap().is_empty()
    }));
    let runtime_state = states
        .iter()
        .find(|state| {
            let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
            frame["address"] == CREATED
                && frame["code_address"] == CREATED
                && frame["mode"] == "Runtime"
                && frame["code_hash"] == runtime_hash
                && state["entry"]["store"]["created"][CREATED] == true
        })
        .expect("created runtime retains its actual code hash and creation state");
    // CREATED had empty code when optional private regions were selected. The
    // deployed overlay executes through the Native relative body instead.
    assert!(runtime_state["executed_pcs"].as_array().unwrap().is_empty());
    let procedure = relative_runtime(analysis, CREATED, &runtime_hash)
        .expect("created runtime has a current, executed Native relative body");
    assert_eq!(procedure["input"]["created"][CREATED], true);
    assert!(
        procedure["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|output| {
                output["kind"] == "Return"
                    && output["store"]["created"][CREATED] == true
                    && contains(&output["data"]["length"], U256::from(32))
                    && output["data"]["content"]["Exact"]
                        == serde_json::json!(U256::from(42).to_be_bytes::<32>())
            })
    );
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
                    && native_bytes::word_matches(&outcome["data"], U256::from(42))
            }),
        "no correlated created runtime/nonce/storage/return commit: {}",
        analysis["outcomes"]
    );
}

#[test]
fn native_runtime_execution_requires_current_source_and_typed_body_receipts() {
    let result = fixture("create-runtime", &["--ssa"]);
    let analysis = verify_export(&result);
    let hash = serde_json::json!(keccak256(
        alloy_primitives::hex::decode("602a5f5260205ff3").unwrap()
    ));
    assert!(relative_runtime(analysis, CREATED, &hash).is_some());
    let mut changed = analysis.clone();
    for record in changed["procedure_summaries"].as_array_mut().unwrap() {
        let unrelated = record["procedures"][0].clone();
        record["procedures"].as_array_mut().unwrap().push(unrelated);
        let mut node = record["regional_cfg"]["nodes"][0].clone();
        node["procedure"] = serde_json::json!(1);
        record["regional_cfg"]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(node);
        record["procedures"][0]["semantics"] = serde_json::json!("Process");
    }
    assert!(
        relative_runtime(&changed, CREATED, &hash).is_none(),
        "a separate Native procedure cannot replace the declared root"
    );
    for (path, value) in [
        ("/source_current", serde_json::json!(false)),
        ("/closed", serde_json::json!(false)),
        ("/version/generation", serde_json::json!(0)),
        ("/version/namespace", Json::Null),
        ("/procedures/0/version/namespace", serde_json::json!(99)),
        ("/source_state", serde_json::json!(u64::MAX)),
        ("/procedures/0/semantics", serde_json::json!("Process")),
        (
            "/procedures/0/frame/code_hash",
            serde_json::json!(keccak256([])),
        ),
        (
            "/procedures/0/frame/caller/Concrete",
            serde_json::json!(CREATED),
        ),
        ("/procedures/0/frame/is_static", serde_json::json!(true)),
        ("/regional_cfg/nodes/0/excluded", serde_json::json!(true)),
        (
            "/regional_cfg/nodes/0/frame/address",
            serde_json::json!(ENTRY),
        ),
        (
            "/regional_cfg/nodes/0/transfer/semantics",
            serde_json::json!("Process"),
        ),
        (
            "/regional_cfg/nodes/0/transfer/current",
            serde_json::json!(false),
        ),
        (
            "/regional_cfg/nodes/0/transfer/complete",
            serde_json::json!(false),
        ),
        (
            "/regional_cfg/nodes/0/transfer/visited_block",
            serde_json::json!(false),
        ),
        (
            "/regional_cfg/nodes/0/transfer/input_revision",
            serde_json::json!(99),
        ),
    ] {
        let mut changed = analysis.clone();
        for record in changed["procedure_summaries"].as_array_mut().unwrap() {
            *record.pointer_mut(path).unwrap() = value.clone();
        }
        assert!(
            relative_runtime(&changed, CREATED, &hash).is_none(),
            "{path}"
        );
    }
    // Equality alone would accept two absent/null/noninteger clocks. Conversely,
    // matching body clocks remain valid when the procedure entry clock differs.
    for revision in [Json::Null, serde_json::json!("0"), serde_json::json!(99)] {
        let mut changed = analysis.clone();
        for record in changed["procedure_summaries"].as_array_mut().unwrap() {
            let node = &mut record["regional_cfg"]["nodes"][0];
            node["input_revision"] = revision.clone();
            node["transfer"]["input_revision"] = revision.clone();
        }
        assert_eq!(
            relative_runtime(&changed, CREATED, &hash).is_some(),
            revision.as_u64().is_some()
        );
    }
    let mut changed = analysis.clone();
    for record in changed["procedure_summaries"].as_array_mut().unwrap() {
        let node = &mut record["regional_cfg"]["nodes"][0];
        node.as_object_mut().unwrap().remove("input_revision");
        node["transfer"]
            .as_object_mut()
            .unwrap()
            .remove("input_revision");
    }
    assert!(relative_runtime(&changed, CREATED, &hash).is_none());
}

#[test]
fn native_identity_call_exports_its_frame_return_data_and_effect_ssa() {
    let result = fixture("identity-precompile", &["--ssa"]);
    assert_policy(&result, 1);
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
                    && native_bytes::word_matches(&outcome["data"], U256::from(42))
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

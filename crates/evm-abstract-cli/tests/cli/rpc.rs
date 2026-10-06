//! The actual CLI discovers callees through bounded, exact-hash HTTP requests.

use super::run_concrete;
use alloy_primitives::{Address, U256, hex, keccak256};
use serde_json::{Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    net::TcpListener,
    process::Output,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

#[path = "rpc/pinning.rs"]
mod pinning;

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const CALLEE: &str = "0x0000000000000000000000000000000000000200";
const LEAF: &str = "0x0000000000000000000000000000000000000300";
const CALLER: &str = "0x0000000000000000000000000000000000001000";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

#[derive(Clone)]
struct AccountFixture {
    code: Vec<u8>,
    zero_facts: bool,
}

#[derive(Clone)]
struct Fixture(BTreeMap<String, AccountFixture>);

impl Fixture {
    fn new(accounts: &[(&str, &str)]) -> Self {
        Self(
            accounts
                .iter()
                .map(|(address, code)| {
                    (
                        (*address).to_owned(),
                        AccountFixture {
                            code: hex::decode(code).unwrap(),
                            zero_facts: false,
                        },
                    )
                })
                .collect(),
        )
    }

    fn reply(&self, request: &Json) -> Reply {
        let method = request["method"].as_str().unwrap();
        let result = match method {
            "eth_chainId" => json!("0x1"),
            "eth_getBlockByHash" | "eth_getBlockByNumber" => json!({"hash":BLOCK,"number":"0x10"}),
            _ => {
                let address = request["params"][0].as_str().unwrap();
                let account = self
                    .0
                    .get(address)
                    .unwrap_or_else(|| panic!("unconfigured RPC account {address}: {method}"));
                let balance = if account.zero_facts {
                    "0x0"
                } else {
                    "0x1000000"
                };
                match method {
                    "eth_getCode" => json!(format!("0x{}", hex::encode(&account.code))),
                    "eth_getBalance" => json!(balance),
                    "eth_getTransactionCount" => json!("0x0"),
                    _ => panic!("unexpected RPC method {method}"),
                }
            }
        };
        Reply::Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
    }
}

enum Reply {
    Json(Json),
    Bytes(Vec<u8>),
    Http(u16),
}

struct RpcServer {
    endpoint: String,
    stop: Option<mpsc::Sender<()>>,
    done: Option<mpsc::Receiver<Vec<Json>>>,
}

impl RpcServer {
    fn new<'scope, 'env, H>(scope: &'scope thread::Scope<'scope, 'env>, handler: H) -> Self
    where
        H: Fn(&Json) -> Reply + Send + 'scope,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = mpsc::channel();
        let (completed, done) = mpsc::channel();
        scope.spawn(move || {
            let mut requests = Vec::new();
            while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("discovery RPC accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                    assert!(header.len() <= 8192);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap();
                assert!(length <= 8192);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: Json = serde_json::from_slice(&body).unwrap();
                let (status, bytes) = match handler(&request) {
                    Reply::Json(reply) => (200, serde_json::to_vec(&reply).unwrap()),
                    Reply::Bytes(bytes) => (200, bytes),
                    Reply::Http(status) => (status, Vec::new()),
                };
                write!(
                    stream,
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                )
                .unwrap();
                stream.write_all(&bytes).unwrap();
                requests.push(request);
            }
            completed.send(requests).unwrap();
        });
        Self {
            endpoint,
            stop: Some(stop),
            done: Some(done),
        }
    }

    fn finish(mut self) -> Vec<Json> {
        drop(self.stop.take());
        self.done.take().unwrap().recv().unwrap()
    }
}

impl Drop for RpcServer {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(done) = self.done.take() {
            let _ = done.recv();
        }
    }
}

fn execute(server: &RpcServer, extra: &[&str]) -> Output {
    let mut args = vec![
        "analyze",
        "--rpc",
        &server.endpoint,
        "--block-hash",
        BLOCK,
        "--evm.to",
        ENTRY,
        "--format",
        "json",
    ];
    args.extend_from_slice(extra);
    run_concrete(&args)
}

fn result(output: Output, exit: i32) -> Json {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn call(op: u8, target: u16, value: u8) -> String {
    let value = if matches!(op, 0xf1 | 0xf2) {
        format!("60{value:02x}")
    } else {
        String::new()
    };
    format!("5f5f5f5f{value}61{target:04x}6207a120{op:02x}")
}

fn requests_for<'a>(requests: &'a [Json], method: &str, address: &str) -> Vec<&'a Json> {
    requests
        .iter()
        .filter(|request| request["method"] == method && request["params"][0] == address)
        .collect()
}

fn assert_pinned(requests: &[Json]) {
    for request in requests {
        match request["method"].as_str().unwrap() {
            "eth_chainId" => assert_eq!(request["params"], json!([])),
            "eth_getBlockByHash" => assert_eq!(request["params"], json!([BLOCK, false])),
            _ => assert_eq!(
                request["params"].as_array().unwrap().last().unwrap(),
                &json!({"blockHash":BLOCK,"requireCanonical":true})
            ),
        }
        assert_ne!(request["method"], "eth_getProof");
        assert!(!request.to_string().contains("latest"));
    }
}

#[test]
fn recursive_rpc_callees_are_discovered_without_account_flags_and_cached() {
    thread::scope(|scope| {
        let a = format!("{}50{}5000", call(0xf1, 0x200, 0), call(0xf1, 0x200, 0));
        let b = format!("{}5000", call(0xf1, 0x300, 0));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &b), (LEAF, "602a00")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &[]), 0);
        assert_eq!(analysis["status"], "Converged");
        assert!(analysis["frontiers"].as_array().unwrap().is_empty());
        for address in [ENTRY, CALLEE, LEAF] {
            assert!(analysis["world"]["accounts"].get(address).is_some());
        }
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            let frames = state["key"]["frames"].as_array().unwrap();
            frames.len() == 3 && frames[2]["code_address"] == LEAF
        }));
        let requests = server.finish();
        assert_eq!(
            analysis["rpc_acquisition"]["fetched_accounts"],
            json!([CALLEE, LEAF])
        );
        assert_eq!(analysis["rpc_acquisition"]["rounds"], 3);
        assert_eq!(
            analysis["rpc_acquisition"]["requests"].as_u64().unwrap() as usize,
            requests.len()
        );
        assert!(
            analysis["rpc_acquisition"]["states_created"]
                .as_u64()
                .unwrap()
                >= analysis["states"].as_array().unwrap().len() as u64
        );
        for address in [ENTRY, CALLEE, LEAF] {
            for method in ["eth_getCode", "eth_getBalance", "eth_getTransactionCount"] {
                assert_eq!(requests_for(&requests, method, address).len(), 1);
            }
        }
        assert_pinned(&requests);
    });
}

#[test]
fn rpc_discovery_can_be_disabled_without_guessing_empty_code() {
    thread::scope(|scope| {
        let a = format!("{}00", call(0xf1, 0x200, 0));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "00")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &["--no-rpc-discovery"]), 2);
        assert_eq!(analysis["status"], "Incomplete");
        assert!(analysis["world"]["accounts"].get(CALLEE).is_none());
        assert!(
            analysis["frontiers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|frontier| { frontier["reason"]["MissingCode"] == CALLEE })
        );
        let requests = server.finish();
        assert!(requests_for(&requests, "eth_getCode", CALLEE).is_empty());
        assert_pinned(&requests);
    });
}

fn contains(value: &Json, expected: U256) -> bool {
    value["Constants"].as_array().is_some_and(|values| {
        values.iter().any(|value| {
            value.as_str().and_then(|value| value.parse::<U256>().ok()) == Some(expected)
        })
    })
}

fn address_word(address: &str) -> U256 {
    U256::from_be_slice(address.parse::<Address>().unwrap().as_slice())
}

#[test]
fn discovered_code_preserves_each_call_kind_execution_context() {
    thread::scope(|scope| {
        for op in [0xf1, 0xf2, 0xf4, 0xfa] {
            let a = format!("{}00", call(op, 0x200, 9));
            let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "30333400")]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let analysis = result(execute(&server, &["--evm.value", "42"]), 0);
            assert_eq!(analysis["status"], "Converged");
            let child = analysis["states"]
                .as_array()
                .unwrap()
                .iter()
                .find(|state| state["key"]["frames"].as_array().unwrap().len() == 2)
                .unwrap();
            let frame = &child["key"]["frames"][1];
            let address = if matches!(op, 0xf2 | 0xf4) {
                ENTRY
            } else {
                CALLEE
            };
            let caller = if op == 0xf4 { CALLER } else { ENTRY };
            let value = if op == 0xf4 {
                42
            } else if op == 0xfa {
                0
            } else {
                9
            };
            assert_eq!(frame["code_address"], CALLEE);
            assert_eq!(frame["address"], address);
            assert_eq!(frame["caller"]["Concrete"], caller);
            assert_eq!(frame["is_static"], op == 0xfa);
            assert!(contains(&child["exit_stack"][0], address_word(address)));
            assert!(contains(&child["exit_stack"][1], address_word(caller)));
            assert!(contains(&child["exit_stack"][2], U256::from(value)));
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getCode", CALLEE).len(), 1);
            assert_pinned(&requests);
        }
    });
}

fn delegation(address: &str) -> String {
    format!("ef0100{}", address.strip_prefix("0x").unwrap())
}

#[test]
fn rpc_discovers_entry_and_callee_delegations_and_follows_only_one_pointer() {
    thread::scope(|scope| {
        for delegated_entry in [true, false] {
            let marker = delegation(LEAF);
            let a = if delegated_entry {
                marker.clone()
            } else {
                format!("{}00", call(0xf1, 0x200, 0))
            };
            let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &marker), (LEAF, "303300")]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let analysis = result(execute(&server, &[]), 0);
            let state = analysis["states"]
                .as_array()
                .unwrap()
                .iter()
                .find(|state| {
                    state["key"]["frames"].as_array().unwrap().last().unwrap()["code_address"]
                        == LEAF
                })
                .unwrap();
            let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
            assert_eq!(
                frame["address"],
                if delegated_entry { ENTRY } else { CALLEE }
            );
            assert_eq!(frame["mode"], "Runtime");
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getCode", LEAF).len(), 1);
            assert_eq!(
                requests_for(&requests, "eth_getCode", CALLEE).len(),
                usize::from(!delegated_entry)
            );
            assert_pinned(&requests);
        }
        let a = format!("{}00", call(0xf1, 0x200, 0));
        let b = delegation(LEAF);
        let c = delegation("0x0000000000000000000000000000000000000400");
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &b), (LEAF, &c)]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &[]), 0);
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
            frame["code_address"] == LEAF && frame["mode"] == "InvalidDelegation"
        }));
        let requests = server.finish();
        assert!(!requests.iter().any(|request| request["params"][0] == "0x0000000000000000000000000000000000000400"));
        assert_pinned(&requests);
    });
}

#[test]
fn native_and_delegated_precompile_execution_never_fetches_precompile_code() {
    thread::scope(|scope| {
        let native = "0x0000000000000000000000000000000000000001";
        for delegated in [false, true] {
            let a = format!("{}00", call(0xf1, if delegated { 0x200 } else { 1 }, 0));
            let b = delegation(native);
            let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &b)]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let analysis = result(execute(&server, &[]), 0);
            assert_eq!(analysis["status"], "Converged");
            assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
                let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
                frame["code_address"] == native
                    && if delegated {
                        frame["mode"] == "Empty"
                    } else {
                        frame["mode"]["Precompile"] == native
                    }
            }));
            let requests = server.finish();
            assert!(requests_for(&requests, "eth_getCode", native).is_empty());
            assert_pinned(&requests);
        }
    });
}

#[test]
fn unknown_rpc_call_target_keeps_a_frontier_without_guessing_remote_addresses() {
    thread::scope(|scope| {
        // Unrequested storage is unknown; the concrete low-160 target is unproven.
        let fixture = Fixture::new(&[(ENTRY, "5f5f5f5f5f5f545af100")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &["--max-call-depth", "2"]), 2);
        assert_eq!(analysis["status"], "Incomplete");
        assert!(
            analysis["frontiers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|frontier| { frontier["reason"] == "UnknownTarget" })
        );
        assert!(
            analysis["rpc_acquisition"]["fetched_accounts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["method"] == "eth_getCode")
                .count(),
            1
        );
        assert_pinned(&requests);
    });
}

fn acquisition_failure(analysis: &Json, address: &str) -> Json {
    assert_eq!(analysis["status"], "Incomplete");
    let failure = analysis["frontiers"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|frontier| {
            let frontier = &frontier["reason"]["RpcAcquisition"];
            (frontier["address"] == address).then(|| frontier["failure"].clone())
        })
        .unwrap_or_else(|| {
            panic!(
                "missing typed acquisition frontier: {}",
                analysis["frontiers"]
            )
        });
    assert_eq!(failure["context"]["chain_id"], "0x1");
    assert_eq!(failure["context"]["block_hash"], BLOCK);
    assert!(!failure["kind"].is_null());
    assert!(!failure["message"].as_str().unwrap().is_empty());
    assert!(analysis["world"]["accounts"].get(address).is_none());
    assert_eq!(
        analysis["rpc_acquisition"]["failed_accounts"],
        json!([address])
    );
    failure
}

#[test]
fn discovered_rpc_failures_are_typed_atomic_and_never_become_empty_success() {
    thread::scope(|scope| {
        for case in [
            "null-code",
            "remote",
            "wrong-id",
            "http",
            "json",
            "invalid-balance",
            "invalid-nonce",
        ] {
            let a = format!("{}50{}5000", call(0xf1, 0x200, 0), call(0xf1, 0x200, 0));
            let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "602a00")]);
            let server = RpcServer::new(scope, move |request| {
                let Reply::Json(mut reply) = fixture.reply(request) else {
                    unreachable!()
                };
                if request["params"][0] != CALLEE {
                    return Reply::Json(reply);
                }
                match (case, request["method"].as_str().unwrap()) {
                    ("null-code", "eth_getCode") => reply["result"] = Json::Null,
                    ("remote", "eth_getCode") => {
                        return Reply::Json(
                            json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32602,"message":"blockHash unsupported"}}),
                        );
                    }
                    ("wrong-id", "eth_getCode") => reply["id"] = json!(999_999),
                    ("http", "eth_getCode") => return Reply::Http(503),
                    ("json", "eth_getCode") => return Reply::Bytes(b"not JSON".to_vec()),
                    ("invalid-balance", "eth_getBalance") => reply["result"] = json!("0x00"),
                    ("invalid-nonce", "eth_getTransactionCount") => reply["result"] = json!("0xzz"),
                    _ => {}
                }
                Reply::Json(reply)
            });
            let analysis = result(execute(&server, &["--ssa"]), 2);
            assert!(analysis.get("ssa").is_none());
            let failure = acquisition_failure(&analysis, CALLEE);
            assert_eq!(failure["context"]["account"], CALLEE);
            assert_eq!(
                failure["context"]["method"],
                match case {
                    "invalid-balance" => "eth_getBalance",
                    "invalid-nonce" => "eth_getTransactionCount",
                    _ => "eth_getCode",
                }
            );
            assert!(
                analysis["rpc_acquisition"]["fetched_accounts"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert!(!analysis["states"].as_array().unwrap().iter().any(|state| {
                state["key"]["frames"].as_array().unwrap().last().unwrap()["code_address"] == CALLEE
            }));
            let requests = server.finish();
            assert_eq!(
                requests_for(&requests, "eth_getCode", CALLEE).len(),
                1,
                "{case}"
            );
            assert_pinned(&requests);
        }
    });
}

#[test]
fn discovered_account_is_not_committed_when_final_chain_verification_fails() {
    thread::scope(|scope| {
        let a = format!("{}00", call(0xf1, 0x200, 0));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "602a00")]);
        let chain_checks = AtomicUsize::new(0);
        let server = RpcServer::new(scope, move |request| {
            let Reply::Json(mut reply) = fixture.reply(request) else {
                unreachable!()
            };
            if request["method"] == "eth_chainId"
                && chain_checks.fetch_add(1, Ordering::Relaxed) == 3
            {
                reply["result"] = json!("0x2");
            }
            Reply::Json(reply)
        });
        let analysis = result(execute(&server, &[]), 2);
        let failure = acquisition_failure(&analysis, CALLEE);
        assert_eq!(failure["context"]["method"], "eth_chainId");
        let requests = server.finish();
        assert_eq!(
            requests_for(&requests, "eth_getTransactionCount", CALLEE).len(),
            1
        );
        assert_pinned(&requests);
    });
}

#[test]
fn rpc_account_and_request_caps_stop_discovery_with_explicit_partial_results() {
    thread::scope(|scope| {
        for (flag, limit, fetched, callee_requests, max_requests) in [
            ("--max-rpc-accounts", "1", false, 0, 7),
            ("--max-rpc-requests", "7", false, 0, 7),
            // All three account observations arrive, but no final check fits.
            ("--max-rpc-requests", "12", false, 1, 12),
        ] {
            let a = format!("{}00", call(0xf1, 0x200, 0));
            let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "00")]);
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let analysis = result(execute(&server, &[flag, limit]), 2);
            acquisition_failure(&analysis, CALLEE);
            assert_eq!(
                analysis["rpc_acquisition"]["fetched_accounts"],
                if fetched { json!([CALLEE]) } else { json!([]) }
            );
            let requests = server.finish();
            assert_eq!(
                requests_for(&requests, "eth_getCode", CALLEE).len(),
                callee_requests
            );
            assert!(requests.len() <= max_requests);
            assert_eq!(
                analysis["rpc_acquisition"]["requests"].as_u64().unwrap() as usize,
                requests.len()
            );
            assert_pinned(&requests);
        }
        let a = format!("{}00", call(0xf1, 0x200, 0));
        let b = format!("{}00", call(0xf1, 0x300, 0));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &b), (LEAF, "00")]);
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let analysis = result(execute(&server, &["--max-rpc-accounts", "2"]), 2);
        acquisition_failure(&analysis, LEAF);
        assert_eq!(
            analysis["rpc_acquisition"]["fetched_accounts"],
            json!([CALLEE])
        );
        assert!(analysis["world"]["accounts"].get(CALLEE).is_some());
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getCode", CALLEE).len(), 1);
        assert!(requests_for(&requests, "eth_getCode", LEAF).is_empty());
        assert_pinned(&requests);
    });
}

fn relations(analysis: &Json) -> BTreeSet<String> {
    analysis["outcomes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|outcome| {
            serde_json::to_string(&json!([outcome["kind"], outcome["data"], outcome["store"]]))
                .unwrap()
        })
        .collect()
}

fn verified_export(export: &Json) -> &Json {
    let analysis = &export["analysis"];
    assert_eq!(analysis["status"], "Converged");
    assert!(analysis["frontiers"].as_array().unwrap().is_empty());
    assert_eq!(
        export["ssa"]["blocks"].as_array().unwrap().len(),
        analysis["states"].as_array().unwrap().len()
    );
    assert_eq!(
        export["ssa"]["transitions"].as_array().unwrap().len(),
        analysis["edges"].as_array().unwrap().len()
    );
    analysis
}

#[test]
fn discovered_facts_survive_ancestor_revert_and_match_preloaded_summary_oracles() {
    thread::scope(|scope| {
        let a = format!(
            "60035f55{}50{}50{}5000",
            call(0xf1, 0x200, 5),
            call(0xf1, 0x300, 2),
            call(0xf1, 0x300, 2)
        );
        let b = format!("60075f555f5fa0{}505f5ffd", call(0xf1, 0x300, 1));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, &b), (LEAF, "602a5f5260205ff3")]);
        let mut analyses = Vec::new();
        for extra in [
            vec!["--ssa"],
            vec![
                "--ssa",
                "--no-rpc-discovery",
                "--account",
                CALLEE,
                "--account",
                LEAF,
            ],
            vec!["--ssa", "--no-summaries"],
            vec![
                "--ssa",
                "--no-summaries",
                "--no-rpc-discovery",
                "--account",
                CALLEE,
                "--account",
                LEAF,
            ],
        ] {
            let fixture = fixture.clone();
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let export = result(execute(&server, &extra), 0);
            let analysis = verified_export(&export).clone();
            assert!(
                analysis["edges"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|edge| edge["kind"] == "Revert")
            );
            for outcome in analysis["outcomes"].as_array().unwrap() {
                assert!(
                    outcome["store"]["possible_logs"]
                        .as_array()
                        .unwrap()
                        .is_empty()
                );
                assert!(
                    !outcome["store"]["persistent"]["slots"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|slot| slot["address"] == CALLEE)
                );
            }
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getCode", LEAF).len(), 1);
            assert_pinned(&requests);
            analyses.push(analysis);
        }
        // Compare discovery with complete initial observations under the same
        // summary policy, including every outcome's data and transaction state.
        assert_eq!(relations(&analyses[0]), relations(&analyses[1]));
        assert_eq!(relations(&analyses[2]), relations(&analyses[3]));
        for analysis in &analyses[2..] {
            assert_eq!(analysis["summary_stats"]["hits"], 0);
            assert!(analysis["summaries"].as_array().unwrap().is_empty());
        }
        assert_eq!(
            analyses[0]["rpc_acquisition"]["fetched_accounts"],
            json!([CALLEE, LEAF])
        );
        assert_eq!(
            analyses[2]["rpc_acquisition"]["fetched_accounts"],
            json!([CALLEE, LEAF])
        );
    });
}

#[test]
fn discovered_repeated_read_only_callee_reuses_a_summary_with_identical_outcomes() {
    thread::scope(|scope| {
        let a = format!("{}50{}5000", call(0xfa, 0x200, 0), call(0xfa, 0x200, 0));
        let fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "602a5f5260205ff3")]);
        let mut analyses = Vec::new();
        for extra in [vec!["--ssa"], vec!["--ssa", "--no-summaries"]] {
            let fixture = fixture.clone();
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let export = result(execute(&server, &extra), 0);
            analyses.push(verified_export(&export).clone());
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getCode", CALLEE).len(), 1);
            assert_pinned(&requests);
        }
        let enabled = &analyses[0];
        let disabled = &analyses[1];
        assert!(enabled["summary_stats"]["hits"].as_u64().unwrap() > 0);
        assert!(
            enabled["summaries"]
                .as_array()
                .unwrap()
                .iter()
                .any(|summary| {
                    !summary["reused_at"].as_array().unwrap().is_empty()
                        && summary["outputs"].as_array().unwrap().iter().any(|output| {
                            output["kind"] == "Return"
                                && contains(&output["data"]["length"], U256::from(32))
                                && byte_contains(&output["data"], 31, 42)
                        })
                })
        );
        assert_eq!(disabled["summary_stats"]["hits"], 0);
        assert!(disabled["summaries"].as_array().unwrap().is_empty());
        assert_eq!(relations(enabled), relations(disabled));
    });
}

fn byte_contains(data: &Json, offset: usize, expected: u8) -> bool {
    let byte = data["bytes"]
        .get(offset.to_string())
        .unwrap_or(&data["default"]);
    contains(byte, U256::from(expected))
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

fn account<'a>(store: &'a Json, address: &str) -> Option<&'a Json> {
    store["account_observations"]
        .as_array()?
        .iter()
        .find(|account| account["address"] == address)
}

#[test]
fn discovered_and_preloaded_empty_accounts_preserve_unknown_existence() {
    thread::scope(|scope| {
        for (preloaded, zero_facts) in [(false, false), (false, true), (true, false), (true, true)]
        {
            let a = format!("{}50{}5000", call(0xf1, 0x200, 0), call(0xf1, 0x200, 0));
            let mut fixture = Fixture::new(&[(ENTRY, &a), (CALLEE, "")]);
            fixture.0.get_mut(CALLEE).unwrap().zero_facts = zero_facts;
            let server = RpcServer::new(scope, move |request| fixture.reply(request));
            let extra = if preloaded {
                vec!["--account", CALLEE]
            } else {
                vec![]
            };
            let analysis = result(execute(&server, &extra), 0);
            assert_eq!(
                analysis["world"]["accounts"][CALLEE]["existence"],
                if zero_facts { "unknown" } else { "present" }
            );
            assert_eq!(
                analysis["world"]["code_hashes"][CALLEE],
                json!(keccak256([]))
            );
            assert_eq!(
                analysis["world"]["accounts"][CALLEE]["storage_unknown"],
                true
            );
            assert_eq!(
                analysis["rpc_acquisition"]["fetched_accounts"],
                if preloaded {
                    json!([])
                } else {
                    json!([CALLEE])
                }
            );
            let requests = server.finish();
            assert_eq!(requests_for(&requests, "eth_getCode", CALLEE).len(), 1);
            assert_pinned(&requests);
        }
    });
}

#[test]
fn deployed_runtime_overlay_is_used_without_refetching_snapshot_empty_code() {
    thread::scope(|scope| {
        const CREATED: &str = "0xea53a153a9a04fd632b2486d84732feb3b71afb7";
        const ZERO: &str = "0x0000000000000000000000000000000000000000";
        // Existing creation lesson: deploy a runtime returning 42, then call it.
        let factory = "6100156100275f396100155f6000f0805f555060205f5f5f5f5f5462fffffff160015560205ff361000861000d5f396100085ff3602a5f5260205ff3";
        let mut fixture = Fixture::new(&[(ENTRY, factory), (CREATED, ""), (ZERO, "")]);
        fixture.0.get_mut(CREATED).unwrap().zero_facts = true;
        fixture.0.get_mut(ZERO).unwrap().zero_facts = true;
        let server = RpcServer::new(scope, move |request| fixture.reply(request));
        let export = result(
            execute(&server, &["--ssa", "--account", CREATED, "--account", ZERO]),
            0,
        );
        let analysis = verified_export(&export);
        let runtime_hash = json!(keccak256(hex::decode("602a5f5260205ff3").unwrap()));
        assert!(analysis["states"].as_array().unwrap().iter().any(|state| {
            let frame = state["key"]["frames"].as_array().unwrap().last().unwrap();
            frame["code_address"] == CREATED
                && frame["mode"] == "Runtime"
                && frame["code_hash"] == runtime_hash
                && state["entry"]["store"]["created"][CREATED] == true
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
                        && slot_contains(store, ENTRY, 0, address_word(CREATED))
                        && slot_contains(store, ENTRY, 1, U256::from(1))
                        && contains(&outcome["data"]["length"], U256::from(32))
                        && byte_contains(&outcome["data"], 31, 42)
                })
        );
        assert!(
            analysis["rpc_acquisition"]["fetched_accounts"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getCode", CREATED).len(), 1);
        assert_pinned(&requests);
    });
}

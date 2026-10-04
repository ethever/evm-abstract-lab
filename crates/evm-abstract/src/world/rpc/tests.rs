//! Local JSON-RPC regressions exercise the real HTTP and input boundaries.

use super::{AccountRequest, RpcError, RpcInput, load};
use crate::{
    Address, Fork, U256,
    domain::Value,
    world::{Existence, SnapshotIdentity},
};
use alloy_primitives::{B256, keccak256};
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

type Handler = Box<dyn Fn(&Json) -> Reply + Send>;

enum Reply {
    Json(Json),
    Bytes(Vec<u8>),
    Http(u16),
    Delay,
}

struct Server {
    endpoint: String,
    requests: Arc<Mutex<Vec<Json>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    fn new(handler: Handler) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fake RPC accept failed: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
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
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: Json = serde_json::from_slice(&body).unwrap();
                observed_requests.lock().unwrap().push(request.clone());
                let (status, bytes) = match handler(&request) {
                    Reply::Json(value) => (200, serde_json::to_vec(&value).unwrap()),
                    Reply::Bytes(bytes) => (200, bytes),
                    Reply::Http(status) => (status, Vec::new()),
                    Reply::Delay => {
                        thread::sleep(Duration::from_millis(150));
                        continue;
                    }
                };
                let header = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                // Timeout tests intentionally close the client connection first.
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&bytes);
            }
        });
        Self {
            endpoint,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    fn input(&self) -> RpcInput {
        let mut input = RpcInput::new(
            &self.endpoint,
            Fork::Osaka,
            U256::from(1),
            B256::repeat_byte(0x11),
        );
        input.accounts.push(AccountRequest {
            address: Address::repeat_byte(0x22),
            slots: BTreeSet::from([U256::ZERO]),
        });
        input
    }

    fn requests(&self) -> Vec<Json> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn healthy(request: &Json) -> Json {
    let result = match request["method"].as_str().unwrap() {
        "eth_chainId" => json!("0x1"),
        "eth_getBlockByHash" => json!({"hash": B256::repeat_byte(0x11)}),
        "eth_getCode" => json!("0x00"),
        "eth_getBalance" => json!("0x9"),
        "eth_getTransactionCount" => json!("0x1"),
        "eth_getProof" => json!({
            "address": Address::repeat_byte(0x22), "balance":"0x9", "nonce":"0x1",
            "codeHash":keccak256([0]), "storageHash":B256::repeat_byte(0x33),
            "accountProof":[], "storageProof":[{"key":"0x0", "value":"0x7", "proof":[]}],
        }),
        "eth_getStorageAt" => json!(format!("0x{:064x}", U256::from(7))),
        method => panic!("unexpected RPC method {method}"),
    };
    json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
}

#[test]
fn rpc_world_pins_every_fact_and_keeps_unrequested_facts_unknown() {
    let server = Server::new(Box::new(|request| Reply::Json(healthy(request))));
    let input = server.input();
    let world = load(&input).unwrap();
    assert_eq!(
        world.identity(),
        &SnapshotIdentity::Chain {
            chain_id: input.chain_id,
            block_hash: input.block_hash
        }
    );
    let account = world.account(Address::repeat_byte(0x22)).unwrap();
    assert_eq!(account.balance, Value::constant(U256::from(9)));
    assert_eq!(account.nonce, Value::constant(U256::from(1)));
    assert_eq!(account.existence, Existence::Present);
    assert!(account.storage_unknown);
    assert_eq!(account.storage[&U256::ZERO], Value::constant(U256::from(7)));
    assert!(world.account(Address::repeat_byte(0x44)).is_none());
    assert_eq!(
        world.code_hash(Address::repeat_byte(0x22)),
        Some(keccak256([0]))
    );
    let requests = server.requests();
    assert_eq!(requests.len(), 9);
    for request in requests {
        match request["method"].as_str().unwrap() {
            "eth_chainId" => assert_eq!(request["params"], json!([])),
            "eth_getBlockByHash" => assert_eq!(request["params"], json!([input.block_hash, false])),
            _ => assert_eq!(
                request["params"].as_array().unwrap().last().unwrap(),
                &json!({"blockHash": input.block_hash, "requireCanonical": true})
            ),
        }
        assert!(!request.to_string().contains("latest"));
    }
}

#[test]
fn absence_is_explicit_and_distinct_from_present_empty_code() {
    for present in [true, false] {
        let server = Server::new(Box::new(move |request| {
            let mut response = healthy(request);
            match request["method"].as_str().unwrap() {
                "eth_getCode" => response["result"] = json!("0x"),
                "eth_getBalance" | "eth_getTransactionCount" => response["result"] = json!("0x0"),
                "eth_getStorageAt" => response["result"] = json!(format!("0x{:064x}", U256::ZERO)),
                "eth_getProof" => {
                    response["result"]["balance"] = json!("0x0");
                    response["result"]["nonce"] = json!("0x0");
                    response["result"]["codeHash"] =
                        json!(if present { keccak256([]) } else { B256::ZERO });
                    response["result"]["storageProof"][0]["value"] = json!("0x0");
                }
                _ => {}
            }
            Reply::Json(response)
        }));
        let world = load(&server.input()).unwrap();
        let account = world.account(Address::repeat_byte(0x22)).unwrap();
        assert_eq!(
            account.existence,
            if present {
                Existence::Present
            } else {
                Existence::Absent
            }
        );
        assert_eq!(account.storage_unknown, present);
        assert_eq!(
            world.code_hash(Address::repeat_byte(0x22)),
            Some(if present { keccak256([]) } else { B256::ZERO })
        );
    }
}

#[test]
fn missing_or_mismatched_chain_and_block_fail_before_state_requests() {
    for case in ["chain", "missing-block", "wrong-block"] {
        let server = Server::new(Box::new(move |request| {
            let mut response = healthy(request);
            match (case, request["method"].as_str().unwrap()) {
                ("chain", "eth_chainId") => response["result"] = json!("0x2"),
                ("missing-block", "eth_getBlockByHash") => response["result"] = Json::Null,
                ("wrong-block", "eth_getBlockByHash") => {
                    response["result"]["hash"] = json!(B256::repeat_byte(0x12))
                }
                _ => {}
            }
            Reply::Json(response)
        }));
        let error = load(&server.input()).unwrap_err();
        match case {
            "chain" => assert!(matches!(error, RpcError::ChainMismatch { .. })),
            "missing-block" => assert!(matches!(error, RpcError::MissingResult { .. })),
            _ => assert!(matches!(error, RpcError::BlockMismatch { .. })),
        }
        assert!(server.requests().iter().all(|request| {
            !request["method"]
                .as_str()
                .unwrap()
                .starts_with("eth_getCode")
        }));
    }
}

#[test]
fn state_failures_never_retry_at_latest_or_manufacture_empty_code() {
    for case in [
        "unsupported-selector",
        "null-code",
        "wrong-id",
        "http",
        "json",
        "limit",
        "timeout",
    ] {
        let server = Server::new(Box::new(move |request| {
            if request["method"] != "eth_getCode" {
                return Reply::Json(healthy(request));
            }
            match case {
                "unsupported-selector" => Reply::Json(
                    json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32602,"message":"blockHash unsupported"}}),
                ),
                "null-code" => {
                    let mut response = healthy(request);
                    response["result"] = Json::Null;
                    Reply::Json(response)
                }
                "wrong-id" => {
                    let mut response = healthy(request);
                    response["id"] = json!(999);
                    Reply::Json(response)
                }
                "http" => Reply::Http(503),
                "json" => Reply::Bytes(b"not json".to_vec()),
                "limit" => Reply::Bytes(vec![b' '; 1025]),
                "timeout" => Reply::Delay,
                _ => unreachable!(),
            }
        }));
        let mut input = server.input();
        input.max_response_bytes = 1024;
        if case == "timeout" {
            input.timeout = Duration::from_millis(50);
        }
        let error = load(&input).unwrap_err();
        let context = error.context();
        assert_eq!(context.method, "eth_getCode");
        assert_eq!(context.account, Some(Address::repeat_byte(0x22)));
        assert_eq!(context.chain_id, input.chain_id);
        assert_eq!(context.block_hash, input.block_hash);
        match case {
            "unsupported-selector" => {
                assert!(matches!(error, RpcError::Remote { code: -32602, .. }))
            }
            "null-code" => assert!(matches!(error, RpcError::MissingResult { .. })),
            "wrong-id" => assert!(matches!(error, RpcError::Response { .. })),
            "http" => assert!(matches!(error, RpcError::Http { status: 503, .. })),
            "json" => assert!(matches!(error, RpcError::Json { .. })),
            "limit" => assert!(matches!(error, RpcError::ResponseLimit { .. })),
            "timeout" => assert!(matches!(
                error,
                RpcError::Transport { .. } | RpcError::Read { .. }
            )),
            _ => unreachable!(),
        }
        let requests = server.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests.last().unwrap()["method"], "eth_getCode");
        assert!(
            requests
                .iter()
                .all(|request| !request.to_string().contains("latest"))
        );
    }
}

#[test]
fn conflicting_account_hash_nonce_balance_address_and_slots_are_rejected() {
    for field in [
        "codeHash",
        "nonce",
        "balance",
        "address",
        "storage",
        "missing-slot",
        "duplicate-slot",
    ] {
        let server = Server::new(Box::new(move |request| {
            let mut response = healthy(request);
            if request["method"] == "eth_getProof" {
                match field {
                    "codeHash" => response["result"][field] = json!(B256::repeat_byte(0x99)),
                    "nonce" | "balance" => response["result"][field] = json!("0x8"),
                    "address" => response["result"][field] = json!(Address::repeat_byte(0x99)),
                    "storage" => response["result"]["storageProof"][0]["value"] = json!("0x8"),
                    "missing-slot" => response["result"]["storageProof"] = json!([]),
                    "duplicate-slot" => {
                        response["result"]["storageProof"] =
                            json!([{"key":"0x1","value":"0x7","proof":[]}])
                    }
                    _ => unreachable!(),
                }
            }
            Reply::Json(response)
        }));
        let error = load(&server.input()).unwrap_err();
        assert_eq!(error.context().account, Some(Address::repeat_byte(0x22)));
        if field == "codeHash" {
            assert!(matches!(error, RpcError::World { .. }));
        } else {
            assert!(matches!(error, RpcError::Response { .. }));
        }
    }
}

#[test]
fn pinned_snapshot_acquisition_rejects_endpoint_chain_switch() {
    let chains = Arc::new(Mutex::new(0));
    let server = Server::new(Box::new(move |request| {
        let mut response = healthy(request);
        if request["method"] == "eth_chainId" {
            let mut chains = chains.lock().unwrap();
            *chains += 1;
            if *chains == 2 {
                response["result"] = json!("0x2");
            }
        }
        Reply::Json(response)
    }));
    assert!(matches!(
        load(&server.input()),
        Err(RpcError::ChainMismatch { .. })
    ));
}

#[test]
fn connection_failures_are_typed_and_carry_the_expected_snapshot() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let input = RpcInput::new(
        endpoint,
        Fork::Osaka,
        U256::from(1),
        B256::repeat_byte(0x11),
    );
    let error = load(&input).unwrap_err();
    assert!(matches!(error, RpcError::Transport { .. }));
    assert_eq!(error.context().method, "eth_chainId");
    assert_eq!(error.context().block_hash, input.block_hash);
}

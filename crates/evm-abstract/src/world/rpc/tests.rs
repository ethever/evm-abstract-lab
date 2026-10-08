//! Local JSON-RPC regressions exercise the real HTTP and input boundaries.

mod storage;

use super::{AccountRequest, RpcBlock, RpcError, RpcFailureKind, RpcInput, Session, load};
use crate::{
    Address, Fork, U256,
    domain::{AbstractValue, Domain},
    world::{Existence, SnapshotIdentity, Store},
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
    thread,
    time::Duration,
};

enum Reply {
    Json(Json),
    Bytes(Vec<u8>),
    Trickle(Json),
    Http(u16),
    Delay(Duration),
}

struct Server {
    endpoint: String,
    requests: Arc<Mutex<Vec<Json>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    fn new<'scope, 'env, H>(scope: &'scope thread::Scope<'scope, 'env>, handler: H) -> Self
    where
        H: Fn(&Json) -> Reply + Send + 'scope,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let observed_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);
        scope.spawn(move || {
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
                let (status, bytes, trickle) = match handler(&request) {
                    Reply::Json(value) => (200, serde_json::to_vec(&value).unwrap(), false),
                    Reply::Bytes(bytes) => (200, bytes, false),
                    Reply::Trickle(value) => (200, serde_json::to_vec(&value).unwrap(), true),
                    Reply::Http(status) => (status, Vec::new(), false),
                    Reply::Delay(delay) => {
                        thread::sleep(delay);
                        continue;
                    }
                };
                let header = format!(
                    "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                // Timeout tests intentionally close the client connection first.
                let _ = stream.write_all(header.as_bytes());
                if trickle {
                    for byte in bytes {
                        if stream.write_all(&[byte]).is_err() {
                            break;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                } else {
                    let _ = stream.write_all(&bytes);
                }
            }
        });
        Self {
            endpoint,
            requests,
            stop,
        }
    }

    fn input(&self) -> RpcInput {
        let mut input = RpcInput::new(&self.endpoint, Fork::Osaka);
        input.block = RpcBlock::Hash(B256::repeat_byte(0x11));
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
    }
}

fn healthy(request: &Json) -> Json {
    let result = match request["method"].as_str().unwrap() {
        "eth_chainId" => json!("0x1"),
        "eth_getBlockByHash" | "eth_getBlockByNumber" => {
            json!({"hash": B256::repeat_byte(0x11), "number":"0x2a"})
        }
        "eth_getCode" => json!("0x00"),
        "eth_getBalance" => json!("0x9"),
        "eth_getTransactionCount" => json!("0x1"),
        "eth_getStorageAt" => json!(format!("0x{:064x}", U256::from(7))),
        method => panic!("unexpected RPC method {method}"),
    };
    json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
}

#[test]
fn rpc_world_pins_every_fact_and_keeps_unrequested_facts_unknown() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let input = server.input();
        let world = load(&input).unwrap();
        assert_eq!(
            world.identity(),
            &SnapshotIdentity::Chain {
                chain_id: U256::from(1),
                block_hash: B256::repeat_byte(0x11)
            }
        );
        let account = world.account(Address::repeat_byte(0x22)).unwrap();
        assert_eq!(account.balance, AbstractValue::constant(U256::from(9)));
        assert_eq!(account.nonce, AbstractValue::constant(U256::from(1)));
        assert_eq!(account.existence, Existence::Present);
        assert!(account.storage_unknown);
        assert_eq!(
            account.storage[&U256::ZERO],
            AbstractValue::constant(U256::from(7))
        );
        assert!(world.account(Address::repeat_byte(0x44)).is_none());
        assert_eq!(
            world.code_hash(Address::repeat_byte(0x22)),
            Some(keccak256([0]))
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 8);
        for request in requests {
            match request["method"].as_str().unwrap() {
                "eth_chainId" => assert_eq!(request["params"], json!([])),
                "eth_getBlockByHash" => {
                    assert_eq!(request["params"], json!([B256::repeat_byte(0x11), false]))
                }
                _ => assert_eq!(
                    request["params"].as_array().unwrap().last().unwrap(),
                    &json!({"blockHash": B256::repeat_byte(0x11), "requireCanonical": true})
                ),
            }
            assert!(!request.to_string().contains("latest"));
        }
    });
}

#[test]
fn latest_number_and_hash_pin_once_across_incremental_acquisition() {
    thread::scope(|scope| {
        for block in [
            RpcBlock::Latest,
            RpcBlock::Number(42),
            RpcBlock::Hash(B256::repeat_byte(0x11)),
        ] {
            let resolutions = Arc::new(Mutex::new(0));
            let observed_resolutions = Arc::clone(&resolutions);
            let server = Server::new(scope, move |request| {
                let mut response = healthy(request);
                if request["method"] == "eth_chainId" {
                    response["result"] = json!("0x38");
                }
                if request["method"] == "eth_getBlockByNumber" {
                    let mut resolutions = observed_resolutions.lock().unwrap();
                    *resolutions += 1;
                    // Resolving the selector again would observe a different
                    // head and mix account facts from two snapshots.
                    if *resolutions > 1 {
                        response["result"]["hash"] = json!(B256::repeat_byte(0x55));
                        response["result"]["number"] = json!("0x2b");
                    }
                }
                Reply::Json(response)
            });
            let mut input = server.input();
            input.block = block;
            let mut session = Session::load(&input).unwrap();
            assert!(session.fetch_account(Address::repeat_byte(0x44)).unwrap());
            assert_eq!(
                session.world().identity(),
                &SnapshotIdentity::Chain {
                    chain_id: U256::from(56),
                    block_hash: B256::repeat_byte(0x11)
                }
            );
            let requests = server.requests();
            assert_eq!(requests.len(), 15);
            assert_eq!(
                *resolutions.lock().unwrap(),
                usize::from(!matches!(block, RpcBlock::Hash(_)))
            );
            for request in requests {
                match request["method"].as_str().unwrap() {
                    "eth_chainId" => assert_eq!(request["params"], json!([])),
                    "eth_getBlockByNumber" => assert_eq!(
                        request["params"],
                        match block {
                            RpcBlock::Latest => json!(["latest", false]),
                            RpcBlock::Number(number) => json!([format!("{number:#x}"), false]),
                            RpcBlock::Hash(_) => unreachable!(),
                        }
                    ),
                    "eth_getBlockByHash" => {
                        assert_eq!(request["params"], json!([B256::repeat_byte(0x11), false]))
                    }
                    method => {
                        assert!(matches!(
                            method,
                            "eth_getCode"
                                | "eth_getBalance"
                                | "eth_getTransactionCount"
                                | "eth_getStorageAt"
                        ));
                        assert_eq!(
                            request["params"].as_array().unwrap().last().unwrap(),
                            &json!({"blockHash": B256::repeat_byte(0x11), "requireCanonical": true})
                        );
                    }
                }
            }
        }
    });
}

#[test]
fn latest_is_the_default_and_bootstrap_failures_retain_only_known_identity() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let mut input = RpcInput::new(&server.endpoint, Fork::Osaka);
        assert_eq!(input.block, RpcBlock::Latest);
        input.max_requests = 1;
        let error = load(&input).unwrap_err();
        assert!(matches!(error, RpcError::AcquisitionLimit { limit: 1, .. }));
        assert_eq!(error.context().method, "eth_getBlockByNumber");
        assert_eq!(error.context().chain_id, Some(U256::from(1)));
        assert_eq!(error.context().block_hash, None);
        assert_eq!(server.requests().len(), 1);
    });
}

#[test]
fn explicit_number_rejects_a_different_returned_height_before_state_requests() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let mut input = server.input();
        input.block = RpcBlock::Number(43);
        let error = load(&input).unwrap_err();
        assert!(matches!(error, RpcError::Response { .. }));
        assert_eq!(error.context().method, "eth_getBlockByNumber");
        assert_eq!(server.requests().len(), 2);
    });
}

#[test]
fn request_bound_includes_bootstrap_and_never_publishes_a_partial_account() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(healthy(request)));
        let mut input = server.input();
        input.block = RpcBlock::Latest;
        input.max_requests = 14;
        let mut session = Session::load(&input).unwrap();
        assert_eq!(session.requests(), 8);
        let error = session
            .fetch_account(Address::repeat_byte(0x44))
            .unwrap_err();
        assert!(matches!(
            error,
            RpcError::AcquisitionLimit { limit: 14, .. }
        ));
        assert_eq!(error.context().method, "eth_getBlockByHash");
        assert_eq!(error.context().block_hash, Some(B256::repeat_byte(0x11)));
        assert!(
            session
                .world()
                .account(Address::repeat_byte(0x44))
                .is_none()
        );
        assert_eq!(session.requests(), 14);
        assert_eq!(
            server
                .requests()
                .iter()
                .filter(|request| request["method"] == "eth_getBlockByNumber")
                .count(),
            1
        );
    });
}

#[test]
fn canonical_state_rejection_never_repins_latest_or_publishes_an_account() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            if request["method"] == "eth_getCode"
                && request["params"][0] == json!(Address::repeat_byte(0x44))
            {
                return Reply::Json(
                    json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32000,"message":"block is no longer canonical"}}),
                );
            }
            Reply::Json(healthy(request))
        });
        let mut input = server.input();
        input.block = RpcBlock::Latest;
        let mut session = Session::load(&input).unwrap();
        let error = session
            .fetch_account(Address::repeat_byte(0x44))
            .unwrap_err();
        assert!(matches!(error, RpcError::Remote { .. }));
        assert_eq!(error.context().block_hash, Some(B256::repeat_byte(0x11)));
        assert!(
            session
                .world()
                .account(Address::repeat_byte(0x44))
                .is_none()
        );
        let requests = server.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request["method"] == "eth_getBlockByNumber")
                .count(),
            1
        );
        assert_eq!(
            requests.last().unwrap()["params"][1],
            json!({"blockHash": B256::repeat_byte(0x11), "requireCanonical": true})
        );
    });
}

#[test]
fn zero_account_keeps_presence_and_unrequested_storage_unknown() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut response = healthy(request);
            match request["method"].as_str().unwrap() {
                "eth_getCode" => response["result"] = json!("0x"),
                "eth_getBalance" | "eth_getTransactionCount" => response["result"] = json!("0x0"),
                "eth_getStorageAt" => response["result"] = json!(format!("0x{:064x}", U256::ZERO)),
                _ => {}
            }
            Reply::Json(response)
        });
        let world = load(&server.input()).unwrap();
        let account = world.account(Address::repeat_byte(0x22)).unwrap();
        assert_eq!(account.existence, Existence::Unknown);
        assert!(account.storage_unknown);
        assert_eq!(
            account.storage[&U256::ZERO],
            AbstractValue::constant(U256::ZERO)
        );
        assert!(!account.storage.contains_key(&U256::from(1)));
        assert_eq!(
            Store::new(&world).read(
                Address::repeat_byte(0x22),
                &AbstractValue::constant(U256::from(1)),
                Domain::default(),
            ),
            AbstractValue::top()
        );
    });
}

#[test]
fn any_nonzero_ordinary_account_observation_establishes_presence() {
    thread::scope(|scope| {
        for nonzero in [
            "eth_getCode",
            "eth_getBalance",
            "eth_getTransactionCount",
            "eth_getStorageAt",
        ] {
            let server = Server::new(scope, move |request| {
                let mut response = healthy(request);
                match request["method"].as_str().unwrap() {
                    "eth_getCode" => {
                        response["result"] = json!(if nonzero == "eth_getCode" {
                            "0x00"
                        } else {
                            "0x"
                        })
                    }
                    "eth_getBalance" | "eth_getTransactionCount" => {
                        response["result"] = json!(if request["method"] == nonzero {
                            "0x1"
                        } else {
                            "0x0"
                        })
                    }
                    "eth_getStorageAt" => {
                        response["result"] = json!(format!(
                            "0x{:064x}",
                            U256::from(u8::from(nonzero == "eth_getStorageAt"))
                        ))
                    }
                    _ => {}
                }
                Reply::Json(response)
            });
            let world = load(&server.input()).unwrap();
            assert_eq!(
                world.account(Address::repeat_byte(0x22)).unwrap().existence,
                Existence::Present
            );
        }
    });
}

#[test]
fn missing_or_mismatched_chain_and_block_fail_before_state_requests() {
    thread::scope(|scope| {
        for case in ["malformed-chain", "missing-block", "wrong-block"] {
            let server = Server::new(scope, move |request| {
                let mut response = healthy(request);
                match (case, request["method"].as_str().unwrap()) {
                    ("malformed-chain", "eth_chainId") => response["result"] = json!("0x01"),
                    ("missing-block", "eth_getBlockByHash") => response["result"] = Json::Null,
                    ("wrong-block", "eth_getBlockByHash") => {
                        response["result"]["hash"] = json!(B256::repeat_byte(0x12))
                    }
                    _ => {}
                }
                Reply::Json(response)
            });
            let error = load(&server.input()).unwrap_err();
            match case {
                "malformed-chain" => assert!(matches!(error, RpcError::Response { .. })),
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
    });
}

#[test]
fn state_failures_never_retry_at_latest_or_manufacture_empty_code() {
    thread::scope(|scope| {
        let request_timeout = Duration::from_millis(500);
        for case in [
            "unsupported-selector",
            "null-code",
            "wrong-id",
            "http",
            "json",
            "limit",
            "timeout",
        ] {
            let server = Server::new(scope, move |request| {
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
                    "timeout" => Reply::Delay(request_timeout.saturating_mul(2)),
                    _ => unreachable!(),
                }
            });
            let mut input = server.input();
            input.max_response_bytes = 1024;
            if case == "timeout" {
                input.timeout = request_timeout;
            }
            let error = load(&input).unwrap_err();
            let context = error.context();
            assert_eq!(context.method, "eth_getCode");
            assert_eq!(context.account, Some(Address::repeat_byte(0x22)));
            assert_eq!(context.chain_id, Some(U256::from(1)));
            assert_eq!(context.block_hash, Some(B256::repeat_byte(0x11)));
            match case {
                "unsupported-selector" => {
                    assert!(matches!(error, RpcError::Remote { code: -32602, .. }))
                }
                "null-code" => assert!(matches!(error, RpcError::MissingResult { .. })),
                "wrong-id" => assert!(matches!(error, RpcError::Response { .. })),
                "http" => assert!(matches!(error, RpcError::Http { status: 503, .. })),
                "json" => assert!(matches!(error, RpcError::Json { .. })),
                "limit" => assert!(matches!(error, RpcError::ResponseLimit { .. })),
                "timeout" => match &error {
                    RpcError::Transport { source, .. } => assert!(source.is_timeout()),
                    other => panic!("expected account acquisition timeout, got {other}"),
                },
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
    });
}

#[test]
fn malformed_ordinary_state_is_rejected_without_partial_account() {
    thread::scope(|scope| {
        for method in [
            "eth_getBalance",
            "eth_getTransactionCount",
            "eth_getStorageAt",
        ] {
            let server = Server::new(scope, move |request| {
                let mut response = healthy(request);
                if request["method"] == method {
                    response["result"] = json!("0x01");
                }
                Reply::Json(response)
            });
            let error = load(&server.input()).unwrap_err();
            assert_eq!(error.context().method, method);
            assert_eq!(error.context().account, Some(Address::repeat_byte(0x22)));
            assert!(matches!(error, RpcError::Response { .. }));
        }
    });
}

#[test]
fn pinned_snapshot_acquisition_rejects_endpoint_chain_switch() {
    thread::scope(|scope| {
        let chains = Arc::new(Mutex::new(0));
        let server = Server::new(scope, move |request| {
            let mut response = healthy(request);
            if request["method"] == "eth_chainId" {
                let mut chains = chains.lock().unwrap();
                *chains += 1;
                if *chains == 2 {
                    response["result"] = json!("0x2");
                }
            }
            Reply::Json(response)
        });
        assert!(matches!(
            load(&server.input()),
            Err(RpcError::ChainMismatch { .. })
        ));
    });
}

#[test]
fn bootstrap_connection_failure_does_not_fabricate_snapshot_identity() {
    // 目的端口 0 不会有监听者，避免释放临时端口后被并行 mock 重新占用。
    let input = RpcInput::new("http://127.0.0.1:0", Fork::Osaka);
    let error = load(&input).unwrap_err();
    assert!(matches!(error, RpcError::Transport { .. }));
    assert_eq!(error.context().method, "eth_chainId");
    assert_eq!(error.context().chain_id, None);
    assert_eq!(error.context().block_hash, None);
}

fn incremental_healthy(request: &Json) -> Json {
    healthy(request)
}

#[test]
fn incremental_accounts_are_cached_and_keep_unrequested_storage_unknown() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| Reply::Json(incremental_healthy(request)));
        let input = server.input();
        let mut session = Session::load(&input).unwrap();
        assert_eq!(session.requests(), 8);
        assert!(!session.fetch_account(Address::repeat_byte(0x22)).unwrap());
        assert_eq!(session.requests(), 8);
        assert!(session.fetch_account(Address::repeat_byte(0x44)).unwrap());
        assert_eq!(session.requests(), 15);
        let account = session.world().account(Address::repeat_byte(0x44)).unwrap();
        assert_eq!(account.balance.singleton(), Some(U256::from(9)));
        assert_eq!(account.nonce.singleton(), Some(U256::from(1)));
        assert!(account.storage_unknown);
        assert!(account.storage.is_empty());
        assert!(!session.fetch_account(Address::repeat_byte(0x44)).unwrap());
        assert_eq!(session.requests(), 15);
        assert_eq!(server.requests().len(), 15);
        for request in server.requests() {
            if !matches!(
                request["method"].as_str(),
                Some("eth_chainId" | "eth_getBlockByHash")
            ) {
                assert_eq!(
                    request["params"].as_array().unwrap().last().unwrap(),
                    &json!({"blockHash": B256::repeat_byte(0x11), "requireCanonical": true})
                );
            }
        }
    });
}

#[test]
fn incremental_final_identity_failure_does_not_install_an_account() {
    thread::scope(|scope| {
        for failing_method in ["eth_chainId", "eth_getBlockByHash"] {
            let checks = Arc::new(Mutex::new(0));
            let server = Server::new(scope, move |request| {
                let mut response = incremental_healthy(request);
                if request["method"] == failing_method {
                    let mut checks = checks.lock().unwrap();
                    *checks += 1;
                    if *checks == 4 {
                        if failing_method == "eth_chainId" {
                            response["result"] = json!("0x2");
                        } else {
                            response["result"]["hash"] = json!(B256::repeat_byte(0x55));
                        }
                    }
                }
                Reply::Json(response)
            });
            let mut session = Session::load(&server.input()).unwrap();
            let error = session
                .fetch_account(Address::repeat_byte(0x44))
                .unwrap_err();
            assert_eq!(error.context().method, failing_method);
            assert!(
                session
                    .world()
                    .account(Address::repeat_byte(0x44))
                    .is_none()
            );
            assert!(
                session
                    .world()
                    .account(Address::repeat_byte(0x22))
                    .is_some()
            );
            assert_eq!(session.requests(), server.requests().len());
            assert_eq!(
                error.kind(),
                if failing_method == "eth_chainId" {
                    RpcFailureKind::ChainMismatch
                } else {
                    RpcFailureKind::BlockMismatch
                }
            );
        }
    });
}

#[test]
fn remote_message_credentials_are_not_copied_to_public_failure_evidence() {
    thread::scope(|scope| {
        let message =
            "rejected https://rpc-user:rpc-password@example.invalid/rpc?api-key=private-token";
        let server = Server::new(scope, move |request| {
            if request["method"] == "eth_getCode"
                && request["params"][0] == json!(Address::repeat_byte(0x44))
            {
                return Reply::Json(json!({
                    "jsonrpc":"2.0", "id":request["id"],
                    "error":{"code":-32000, "message":message}
                }));
            }
            Reply::Json(incremental_healthy(request))
        });
        let mut session = Session::load(&server.input()).unwrap();
        let error = session
            .fetch_account(Address::repeat_byte(0x44))
            .unwrap_err();
        assert!(
            matches!(&error, RpcError::Remote { message: observed, .. } if observed == message)
        );
        let displayed = error.to_string();
        let failure = error.failure();
        assert_eq!(failure.kind, RpcFailureKind::Remote);
        assert_eq!(failure.context.account, Some(Address::repeat_byte(0x44)));
        assert_eq!(failure.context.block_hash, Some(B256::repeat_byte(0x11)));
        let serialized = serde_json::to_string(&failure).unwrap();
        for secret in [
            "https://",
            "rpc-user",
            "rpc-password",
            "private-token",
            message,
        ] {
            assert!(!displayed.contains(secret));
            assert!(!serialized.contains(secret));
        }
        assert!(displayed.contains("-32000"));
        assert!(
            session
                .world()
                .account(Address::repeat_byte(0x44))
                .is_none()
        );
    });
}

#[test]
fn incremental_timeout_and_response_limit_remain_typed_without_partial_cache() {
    thread::scope(|scope| {
        let request_timeout = Duration::from_millis(500);
        for delayed in [false, true] {
            let server = Server::new(scope, move |request| {
                if request["method"] == "eth_getCode"
                    && request["params"][0] == json!(Address::repeat_byte(0x44))
                {
                    return if delayed {
                        // Healthy initialization needs scheduling headroom;
                        // only this incremental request must exceed its deadline.
                        Reply::Delay(request_timeout.saturating_mul(2))
                    } else {
                        Reply::Bytes(vec![b' '; 1025])
                    };
                }
                Reply::Json(incremental_healthy(request))
            });
            let mut input = server.input();
            input.max_response_bytes = 1024;
            if delayed {
                input.timeout = request_timeout;
            }
            let mut session = Session::load(&input).unwrap();
            let error = session
                .fetch_account(Address::repeat_byte(0x44))
                .unwrap_err();
            if delayed {
                match &error {
                    RpcError::Transport { source, .. } => assert!(source.is_timeout()),
                    other => panic!("expected incremental request timeout, got {other}"),
                }
            } else {
                assert!(matches!(error, RpcError::ResponseLimit { limit: 1024, .. }));
            }
            assert_eq!(error.context().method, "eth_getCode");
            assert_eq!(error.context().account, Some(Address::repeat_byte(0x44)));
            assert!(
                session
                    .world()
                    .account(Address::repeat_byte(0x44))
                    .is_none()
            );
            assert_eq!(session.requests(), 11);
        }
    });
}

#[test]
fn incremental_body_timeout_is_a_total_deadline_despite_continuing_chunks() {
    thread::scope(|scope| {
        let server = Server::new(scope, |request| {
            let mut reply = incremental_healthy(request);
            if request["method"] == "eth_getCode"
                && request["params"][0] == json!(Address::repeat_byte(0x44))
            {
                // 每字节间隔10ms，总响应超过500ms；健康初始化留足并发调度余量。
                // 单次读取仍能持续成功，测试必须验证整条响应的总期限。
                reply["padding"] = json!("x".repeat(128));
                assert!(serde_json::to_vec(&reply).unwrap().len() > 50);
                return Reply::Trickle(reply);
            }
            Reply::Json(reply)
        });
        let mut input = server.input();
        input.timeout = Duration::from_millis(500);
        let mut session = Session::load(&input).unwrap();
        let error = session
            .fetch_account(Address::repeat_byte(0x44))
            .unwrap_err();
        match &error {
            // reqwest maps body deadline failures into a concrete I/O error;
            // its Display need not expose the nested timeout text.
            RpcError::Read { .. } => {}
            RpcError::Transport { source, .. } => assert!(source.is_timeout()),
            other => panic!("expected total response timeout, got {other}"),
        }
        assert_eq!(error.context().method, "eth_getCode");
        assert_eq!(error.context().account, Some(Address::repeat_byte(0x44)));
        assert!(
            session
                .world()
                .account(Address::repeat_byte(0x44))
                .is_none()
        );
        assert!(
            session
                .world()
                .account(Address::repeat_byte(0x22))
                .is_some()
        );
        assert_eq!(session.requests(), 11);
    });
}

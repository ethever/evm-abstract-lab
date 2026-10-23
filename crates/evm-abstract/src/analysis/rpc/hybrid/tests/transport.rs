//! Finite HTTP transport for the actual loader; no alternate acquisition path.

use crate::{
    Address, U256,
    analysis::control::CancelToken,
    world::rpc::{RpcBlock, RpcInput},
};
use alloy_primitives::B256;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

pub(super) fn address(value: u64) -> Address {
    Address::from_word(U256::from(value).into())
}

#[derive(Default)]
pub(super) struct Fixture {
    pub code: BTreeMap<Address, String>,
    pub storage: BTreeMap<(Address, U256), U256>,
    pub fail_code: Option<Address>,
    pub wrong_chain_on_storage: bool,
    pub wrong_hash_on_storage: bool,
    pub cancel_storage: Option<CancelToken>,
}

pub(super) struct Server {
    pub endpoint: String,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
}

impl Server {
    pub fn new<'scope, 'env>(scope: &'scope thread::Scope<'scope, 'env>, fixture: Fixture) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_stop = Arc::clone(&stop);
        let observed = Arc::clone(&requests);
        scope.spawn(move || {
            let mut storage_seen = false;
            while !thread_stop.load(Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(pair) => pair,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                };
                stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                }
                let length: usize = String::from_utf8(header).unwrap().lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length").then(|| value.trim().parse().unwrap())
                }).unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: Value = serde_json::from_slice(&body).unwrap();
                observed.lock().unwrap().push(request.clone());
                let method = request["method"].as_str().unwrap();
                let owner = request["params"][0].as_str().and_then(|s| s.parse::<Address>().ok());
                let result = match method {
                    "eth_chainId" if storage_seen && fixture.wrong_chain_on_storage => json!("0x2"),
                    "eth_chainId" => json!("0x1"),
                    "eth_getBlockByHash" | "eth_getBlockByNumber" => {
                        let hash = if storage_seen && fixture.wrong_hash_on_storage { 0x22 } else { 0x11 };
                        json!({
                            "hash": B256::repeat_byte(hash), "number":"0x2a",
                            "parentHash":B256::repeat_byte(0x10), "timestamp":"0x1234",
                            "miner":Address::repeat_byte(0x33), "mixHash":B256::repeat_byte(0x44),
                            "gasLimit":"0x1c9c380", "baseFeePerGas":"0x7",
                            "sha3Uncles":B256::ZERO, "stateRoot":B256::ZERO,
                            "transactionsRoot":B256::ZERO, "receiptsRoot":B256::ZERO,
                            "logsBloom":format!("0x{}", "00".repeat(256)), "difficulty":"0x0",
                            "gasUsed":"0x0", "extraData":"0x", "nonce":"0x0000000000000000"
                        })
                    }
                    "eth_getCode" => json!(format!("0x{}", fixture.code.get(&owner.unwrap()).expect("fixture code"))),
                    "eth_getBalance" => json!("0x64"),
                    "eth_getTransactionCount" => json!("0x1"),
                    "eth_getStorageAt" => {
                        storage_seen = true;
                        if let Some(cancel) = &fixture.cancel_storage { cancel.cancel(); }
                        let slot: U256 = request["params"][1].as_str().unwrap().parse().unwrap();
                        let value = fixture.storage.get(&(owner.unwrap(), slot)).copied().unwrap_or(U256::ZERO);
                        json!(format!("0x{value:064x}"))
                    }
                    _ => panic!("unexpected fixture request {request}"),
                };
                let response = if method == "eth_getCode" && fixture.fail_code == owner {
                    json!({"jsonrpc":"2.0", "id":request["id"], "error":{"code":-32001,"message":"fixture unavailable"}})
                } else {
                    json!({"jsonrpc":"2.0", "id":request["id"], "result":result})
                };
                let bytes = serde_json::to_vec(&response).unwrap();
                let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", bytes.len());
                let _ = stream.write_all(header.as_bytes());
                let _ = stream.write_all(&bytes);
            }
        });
        Self {
            endpoint,
            requests,
            stop,
        }
    }

    pub fn input(&self) -> RpcInput {
        let mut input = RpcInput::new(&self.endpoint, crate::Fork::Osaka);
        input.block = RpcBlock::Hash(B256::repeat_byte(0x11));
        input
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    pub fn count(&self, method: &str, owner: Address) -> usize {
        self.requests()
            .iter()
            .filter(|r| r["method"] == method && r["params"][0] == json!(owner))
            .count()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

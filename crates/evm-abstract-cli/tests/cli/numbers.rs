//! Real CLI regressions cover quantity normalization and fixed-hash RPC requests.

use super::{analyze, run};
use alloy_primitives::{B256, U256, keccak256};
use serde_json::{Value as Json, json};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn decimal_entry_value_matches_hex_and_reaches_execution() {
    for (decimal, hexadecimal) in [
        ("0", "0x0"),
        ("56", "0x38"),
        ("00056", "0x38"),
        ("18446744073709551616", "0x10000000000000000"),
        (
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ),
    ] {
        let hexadecimal_output = analyze(
            "call-return-branch",
            &["--format", "json", "--value", hexadecimal],
        );
        assert!(hexadecimal_output.status.success());
        let decimal_output = analyze(
            "call-return-branch",
            &["--format", "json", "--value", decimal],
        );
        assert!(
            decimal_output.status.success(),
            "decimal {decimal}: {}",
            String::from_utf8_lossy(&decimal_output.stderr)
        );
        let hexadecimal_json: serde_json::Value =
            serde_json::from_slice(&hexadecimal_output.stdout).unwrap();
        let decimal_json: serde_json::Value =
            serde_json::from_slice(&decimal_output.stdout).unwrap();
        assert_eq!(
            decimal_json["entry"]["value"]["Constants"],
            serde_json::json!([hexadecimal])
        );
        assert_eq!(decimal_json, hexadecimal_json);
    }
}

#[test]
fn invalid_rpc_quantities_exit_before_acquiring_a_world() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    for (flag, argument, diagnostic) in [
        ("--chain-id", "--chain-id=-1", "ASCII decimal digits"),
        ("--value", "--value=1.5", "ASCII decimal digits"),
        (
            "--slot",
            "--slot=0x0000000000000000000000000000000000000101:ff",
            "ASCII decimal digits",
        ),
        (
            "--value",
            "--value=115792089237316195423570985008687907853269984665640564039457584007913129639936",
            "256-bit",
        ),
    ] {
        let mut args = vec![
            "analyze",
            "--rpc",
            &endpoint,
            "--entry",
            ENTRY,
            "--block-hash",
            BLOCK,
        ];
        if flag != "--chain-id" {
            args.extend(["--chain-id", "1"]);
        }
        args.push(argument);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains(flag) && error.contains(diagnostic),
            "{error}"
        );
        assert!(!error.contains("transport"), "{error}");
    }
}

#[test]
fn decimal_rpc_quantities_keep_full_width_and_canonical_storage_keys() {
    thread::scope(|scope| {
        for (decimal_chain, hex_chain) in [
            ("56", "0x38"),
            ("18446744073709551616", "0x10000000000000000"),
        ] {
            let server = RpcServer::new(scope, hex_chain);
            let decimal_slot = format!("{ENTRY}:16");
            let hex_slot = format!("{ENTRY}:0x10");
            let uppercase_slot = format!("{ENTRY}:0X10");
            let mut results = Vec::new();
            for (chain, value) in [(decimal_chain, "1000"), (hex_chain, "0x3e8")] {
                let output = run(&[
                    "analyze",
                    "--rpc",
                    &server.endpoint,
                    "--chain-id",
                    chain,
                    "--value",
                    value,
                    "--slot",
                    &decimal_slot,
                    "--slot",
                    &hex_slot,
                    "--slot",
                    &uppercase_slot,
                    "--block-hash",
                    BLOCK,
                    "--entry",
                    ENTRY,
                    "--format",
                    "json",
                ]);
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                let result: Json = serde_json::from_slice(&output.stdout).unwrap();
                assert_eq!(result["status"], "Converged");
                assert_eq!(result["world"]["identity"]["chain_id"], hex_chain);
                // CALLVALUE; PUSH1 16; SLOAD; STOP exercises both CLI quantities.
                assert_eq!(
                    result["states"][0]["exit_stack"],
                    json!([{"Constants":["0x3e8"]}, {"Constants":["0x2a"]}])
                );
                results.push(result);
            }
            assert_eq!(results[0], results[1]);
            let requests = server.finish();
            let proof_requests: Vec<_> = requests
                .iter()
                .filter(|request| request["method"] == "eth_getProof")
                .collect();
            assert_eq!(proof_requests.len(), 2);
            for request in proof_requests {
                assert_eq!(
                    request["params"][1],
                    json!([format!("0x{:064x}", U256::from(16))])
                );
            }
            let storage_requests: Vec<_> = requests
                .iter()
                .filter(|request| request["method"] == "eth_getStorageAt")
                .collect();
            assert_eq!(storage_requests.len(), 2);
            for request in storage_requests {
                assert_eq!(request["params"][1], "0x10");
            }
            for request in requests.iter().filter(|request| {
                request["method"] != "eth_chainId" && request["method"] != "eth_getBlockByHash"
            }) {
                assert_eq!(
                    request["params"].as_array().unwrap().last().unwrap(),
                    &json!({"blockHash":BLOCK,"requireCanonical":true})
                );
            }
        }
    });
}

struct RpcServer {
    endpoint: String,
    stop: Option<mpsc::Sender<()>>,
    done: Option<mpsc::Receiver<Vec<Json>>>,
}

impl RpcServer {
    fn new<'scope, 'env>(scope: &'scope thread::Scope<'scope, 'env>, chain_id: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = mpsc::channel();
        let (completed, done) = mpsc::channel();
        let chain_id = chain_id.to_owned();
        scope.spawn(move || {
            let mut requests = Vec::new();
            while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("mock RPC accept: {error}"),
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
                let result = rpc_result(&request, &chain_id);
                let response = serde_json::to_vec(
                    &json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                )
                .unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .unwrap();
                stream.write_all(&response).unwrap();
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

fn rpc_result(request: &Json, chain_id: &str) -> Json {
    match request["method"].as_str().unwrap() {
        "eth_chainId" => json!(chain_id),
        "eth_getBlockByHash" => json!({"hash":BLOCK}),
        "eth_getCode" => json!("0x3460105400"),
        "eth_getBalance" => json!("0x1000000"),
        "eth_getTransactionCount" => json!("0x0"),
        "eth_getProof" => json!({
            "address":ENTRY,"balance":"0x1000000","nonce":"0x0",
            "codeHash":keccak256([0x34,0x60,0x10,0x54,0x00]),
            "storageHash":B256::repeat_byte(0x33),"accountProof":[],
            "storageProof":[{"key":"0x10","value":"0x2a","proof":[]}]
        }),
        "eth_getStorageAt" => json!(format!("0x{:064x}", U256::from(42))),
        method => panic!("unexpected mock RPC method: {method}"),
    }
}

//! Exercise RPC CLI overrides at the actual socket/typed-error boundary.

use super::{Cli, CliError, Command, ENTRY};
use clap::Parser;
use evm_abstract::world::rpc::RpcError;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::Duration,
};

fn read_request(stream: &mut TcpStream) {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut header = Vec::new();
    let mut byte = [0];
    while !header.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        assert!(header.len() < 8192);
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
    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(request["method"], "eth_chainId");
}

fn execute(parsed: Cli) -> CliError {
    match parsed.command {
        Command::Analyze { args, .. } => args.analyze().unwrap_err(),
        Command::Explain { args } => args.run().unwrap_err(),
        _ => panic!("expected RPC-capable command"),
    }
}

#[test]
fn analyze_and_explain_apply_response_byte_limits_before_reading_the_body() {
    for command in ["analyze", "explain"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let parsed = Cli::try_parse_from([
            "evm-abstract",
            command,
            "--rpc",
            &endpoint,
            "--evm.to",
            ENTRY,
            "--max-rpc-response-bytes",
            "128",
        ])
        .unwrap();
        thread::scope(|scope| {
            scope.spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n",
                    )
                    .unwrap();
            });
            match execute(parsed) {
                CliError::Rpc(RpcError::ResponseLimit { context, limit }) => {
                    assert_eq!(limit, 128);
                    assert_eq!(context.method, "eth_chainId");
                }
                other => panic!("expected response-byte bound, got {other:?}"),
            }
        });
    }
}

#[test]
fn analyze_and_explain_apply_rpc_timeouts_to_an_actual_pending_response() {
    for command in ["analyze", "explain"] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let parsed = Cli::try_parse_from([
            "evm-abstract",
            command,
            "--rpc",
            &endpoint,
            "--evm.to",
            ENTRY,
            "--rpc-timeout-ms",
            "100",
        ])
        .unwrap();
        thread::scope(|scope| {
            scope.spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                read_request(&mut stream);
                // Wait for the client's timeout to close this request. The read
                // deadline also prevents an unwired CLI flag from hanging tests.
                let mut byte = [0];
                let _ = stream.read(&mut byte);
            });
            match execute(parsed) {
                CliError::Rpc(RpcError::Transport { context, source }) => {
                    assert_eq!(context.method, "eth_chainId");
                    assert!(source.is_timeout(), "{source:?}");
                }
                other => panic!("expected request timeout, got {other:?}"),
            }
        });
    }
}

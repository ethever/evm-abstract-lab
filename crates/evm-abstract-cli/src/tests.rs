//! CLI validates explicit input selection before acquisition and execution.

use super::{Cli, Command};
use clap::Parser;
use std::net::TcpListener;

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn world_and_rpc_are_exclusive_and_rpc_requires_fixed_identity() {
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--entry",
            ENTRY
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--entry",
            ENTRY
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--rpc",
            "http://127.0.0.1:1",
            "--chain-id",
            "0x1",
            "--block-hash",
            BLOCK,
            "--entry",
            ENTRY
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--fork",
            "cancun",
            "--entry",
            ENTRY
        ])
        .is_err()
    );
}

#[test]
fn rpc_storage_flags_are_typed_and_only_accepted_for_rpc_input() {
    let slot = format!("{ENTRY}:0x7");
    let parsed = Cli::try_parse_from([
        "evm-abstract",
        "analyze",
        "--rpc",
        "http://127.0.0.1:1",
        "--chain-id",
        "0x1",
        "--block-hash",
        BLOCK,
        "--entry",
        ENTRY,
        "--slot",
        &slot,
        "--no-summaries",
    ])
    .unwrap();
    let Command::Analyze { args, .. } = parsed.command else {
        panic!("expected analyze")
    };
    assert_eq!(args.slot.len(), 1);
    assert_eq!(args.slot[0].slot, alloy_primitives::U256::from(7));
    assert!(args.no_summaries);
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--chain-id",
            "0x1",
            "--block-hash",
            BLOCK,
            "--entry",
            ENTRY,
            "--slot",
            "invalid:latest"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--entry",
            ENTRY,
            "--slot",
            &slot
        ])
        .is_err()
    );
}

#[test]
fn rpc_failure_reaches_cli_as_input_error_before_an_analysis_exists() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let parsed = Cli::try_parse_from([
        "evm-abstract",
        "analyze",
        "--rpc",
        &endpoint,
        "--chain-id",
        "0x1",
        "--block-hash",
        BLOCK,
        "--entry",
        ENTRY,
    ])
    .unwrap();
    let Command::Analyze { args, .. } = parsed.command else {
        panic!("expected analyze")
    };
    let error = args.analyze().unwrap_err();
    assert!(
        error
            .downcast_ref::<evm_abstract::world::rpc::RpcError>()
            .is_some()
    );
}

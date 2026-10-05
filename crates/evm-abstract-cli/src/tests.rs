//! CLI validates explicit input selection before acquisition and execution.

use super::{Cli, Command, error::CliError};
use alloy_primitives::U256;
use clap::{Parser, error::ErrorKind};
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
        "1",
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
        "1",
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
    assert!(matches!(error, CliError::Rpc(_)));
}

#[test]
fn all_cli_quantities_keep_their_u256_value() {
    for (input, expected) in [
        ("0", U256::ZERO),
        ("000", U256::ZERO),
        ("010", U256::from(10)),
        ("56", U256::from(56)),
        ("0x38", U256::from(56)),
        ("0X38", U256::from(56)),
        ("0xAb", U256::from(171)),
        ("18446744073709551616", U256::from(1) << 64),
        (
            "115792089237316195423570985008687907853269984665640564039457584007913129639935",
            U256::MAX,
        ),
        (
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            U256::MAX,
        ),
    ] {
        let slot = format!("{ENTRY}:{input}");
        let parsed = Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--chain-id",
            input,
            "--value",
            input,
            "--slot",
            &slot,
            "--block-hash",
            BLOCK,
            "--entry",
            ENTRY,
        ])
        .unwrap();
        let Command::Analyze { args, .. } = parsed.command else {
            panic!("expected analyze")
        };
        assert_eq!(args.chain_id, Some(expected), "chain id: {input}");
        assert_eq!(args.value, expected, "value: {input}");
        assert_eq!(args.slot[0].slot, expected, "slot: {input}");
    }
}

#[test]
fn invalid_cli_quantities_are_rejected_during_argument_validation() {
    for invalid in [
        "",
        "0x",
        "0X",
        "-1",
        "+1",
        "1.0",
        "1e3",
        "1_000",
        " 1",
        "1 ",
        "ff",
        "0b10",
        "0o10",
        "１",
        "١",
        "0xgg",
        "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        "0x10000000000000000000000000000000000000000000000000000000000000000",
    ] {
        for flag in ["--chain-id", "--value", "--slot"] {
            let chain_id = if flag == "--chain-id" { invalid } else { "1" };
            let value = if flag == "--value" { invalid } else { "0" };
            let slot = if flag == "--slot" { invalid } else { "0" };
            let chain_arg = format!("--chain-id={chain_id}");
            let value_arg = format!("--value={value}");
            let slot_arg = format!("--slot={ENTRY}:{slot}");
            let error = Cli::try_parse_from([
                "evm-abstract",
                "analyze",
                "--rpc",
                "http://127.0.0.1:1",
                "--entry",
                ENTRY,
                "--block-hash",
                BLOCK,
                &chain_arg,
                &value_arg,
                &slot_arg,
            ])
            .err()
            .expect("invalid quantity must fail before acquisition");
            assert_eq!(error.kind(), ErrorKind::ValueValidation, "{flag}={invalid}");
            assert!(error.to_string().contains(flag), "{error}");
        }
    }
}

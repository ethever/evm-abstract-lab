//! CLI validates explicit input selection before acquisition and execution.

use super::{Cli, Command, error::CliError};
use alloy_primitives::U256;
use clap::{Parser, error::ErrorKind};
use evm_abstract::{
    analysis::control::Control,
    domain::{NumericValue, ProductAlgebra, ValueAlgebra},
};
use std::net::TcpListener;

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";

#[test]
fn world_and_rpc_are_exclusive_and_rpc_can_pin_latest() {
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--evm.to",
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
            "--evm.to",
            ENTRY
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--rpc",
            "http://127.0.0.1:1",
            "--block-hash",
            BLOCK,
            "--evm.to",
            ENTRY
        ])
        .is_err()
    );
    let slot = format!("{ENTRY}:0");
    for flags in [
        vec!["--fork", "cancun"],
        vec!["--block-hash", BLOCK],
        vec!["--block-number", "16"],
        vec!["--account", ENTRY],
        vec!["--slot", &slot],
        vec!["--no-rpc-discovery"],
        vec!["--max-rpc-accounts", "256"],
        vec!["--max-rpc-requests", "16384"],
        vec!["--max-rpc-response-bytes", "4194304"],
        vec!["--rpc-timeout-ms", "15000"],
    ] {
        let mut args = vec![
            "evm-abstract",
            "analyze",
            "--world",
            "fixture.json",
            "--evm.to",
            ENTRY,
        ];
        args.extend_from_slice(&flags);
        assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
    }
}

#[test]
fn rpc_storage_flags_are_typed_and_only_accepted_for_rpc_input() {
    let slot = format!("{ENTRY}:0x7");
    let parsed = Cli::try_parse_from([
        "evm-abstract",
        "analyze",
        "--rpc",
        "http://127.0.0.1:1",
        "--block-hash",
        BLOCK,
        "--evm.to",
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
            "--block-hash",
            BLOCK,
            "--evm.to",
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
            "--evm.to",
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
        "--block-hash",
        BLOCK,
        "--evm.to",
        ENTRY,
    ])
    .unwrap();
    let Command::Analyze { args, .. } = parsed.command else {
        panic!("expected analyze")
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
    let error = args.analyze(&algebra, &Control::default()).unwrap_err();
    assert!(matches!(error, CliError::Rpc(_)));
}

#[test]
fn value_and_storage_quantities_keep_their_u256_value() {
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
            "--evm.value",
            input,
            "--slot",
            &slot,
            "--block-hash",
            BLOCK,
            "--evm.to",
            ENTRY,
        ])
        .unwrap();
        let Command::Analyze { args, .. } = parsed.command else {
            panic!("expected analyze")
        };
        assert_eq!(args.evm.value, Some(expected), "value: {input}");
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
        for flag in ["--evm.value", "--slot"] {
            let value = if flag == "--evm.value" { invalid } else { "0" };
            let slot = if flag == "--slot" { invalid } else { "0" };
            let value_arg = format!("--evm.value={value}");
            let slot_arg = format!("--slot={ENTRY}:{slot}");
            let error = Cli::try_parse_from([
                "evm-abstract",
                "analyze",
                "--rpc",
                "http://127.0.0.1:1",
                "--evm.to",
                ENTRY,
                "--block-hash",
                BLOCK,
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

#[test]
fn explain_accepts_every_world_analysis_option_except_output_selection() {
    use clap::CommandFactory;
    let cli = Cli::command();
    let analyze = cli.find_subcommand("analyze").unwrap();
    let explain = cli.find_subcommand("explain").unwrap();
    let explain_flags = explain
        .get_arguments()
        .filter_map(|arg| arg.get_long())
        .collect::<std::collections::BTreeSet<_>>();
    for flag in analyze.get_arguments().filter_map(|arg| arg.get_long()) {
        if !matches!(flag, "format" | "ssa") {
            assert!(
                explain_flags.contains(flag),
                "world explain is missing --{flag}"
            );
        }
    }
}

#[test]
fn calldata_is_one_typed_input_and_empty_bytes_differ_from_omission() {
    use evm_abstract::{
        domain::{EvmValue, WordElement},
        world::EvmByteArray,
    };
    for input in [None, Some("0x"), Some("0x2a")] {
        let mut arguments = vec!["evm-abstract", "cfg", "--hex", "00"];
        if let Some(input) = input {
            arguments.extend(["--evm.calldata", input]);
        }
        let parsed = Cli::try_parse_from(arguments).unwrap();
        let Command::Cfg { args, .. } = parsed.command else {
            panic!("expected cfg")
        };
        let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
        let environment = args.evm.environment::<NumericValue>().unwrap();
        match input {
            None => {
                let calldata = environment.calldata;
                let unknown = EvmByteArray::<NumericValue>::unknown();
                assert_eq!(calldata.len().numeric(), unknown.len().numeric());
                assert_eq!(
                    calldata.default_byte().numeric(),
                    unknown.default_byte().numeric()
                );
                assert_eq!(calldata.len().provenance(), unknown.len().provenance());
                assert_eq!(
                    calldata.default_byte().provenance(),
                    unknown.default_byte().provenance()
                );
                assert!(calldata.stored_bytes().next().is_none());
                assert_eq!(calldata.exact_bytes(algebra.borrowed()), None);
            }
            Some("0x") => {
                let calldata = environment.calldata;
                let zero = EvmValue::<NumericValue>::constant(U256::ZERO);
                // Constructors may retain different optional length proofs;
                // explicit empty input still has exact content and zero padding.
                assert_eq!(
                    calldata.len().numeric().observation().singleton(),
                    Some(U256::ZERO)
                );
                assert_eq!(calldata.exact_bytes(algebra.borrowed()), Some(Vec::new()));
                assert_eq!(calldata.default_byte().singleton(), Some(U256::ZERO));
                assert_eq!(calldata.len().provenance(), zero.provenance());
                assert_eq!(calldata.default_byte().provenance(), zero.provenance());
                for offset in [0, 1, usize::MAX] {
                    assert_eq!(
                        calldata.byte_at(offset, algebra.borrowed()).singleton(),
                        Some(U256::ZERO)
                    );
                }
            }
            Some(_) => assert_eq!(environment.calldata, EvmByteArray::exact(&[42])),
        }
    }
}

#[test]
fn rpc_block_numbers_accept_decimal_and_hex_with_a_u64_bound() {
    for (input, expected) in [
        ("0", 0),
        ("0016", 16),
        ("0x10", 16),
        ("0X10", 16),
        ("18446744073709551615", u64::MAX),
        ("0xffffffffffffffff", u64::MAX),
    ] {
        let parsed = Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--block-number",
            input,
        ])
        .unwrap();
        let Command::Analyze { args, .. } = parsed.command else {
            panic!("expected analyze")
        };
        assert_eq!(args.block_number, Some(expected), "{input}");
    }
    for input in [
        "",
        "0x",
        "-1",
        "+1",
        "1.0",
        "ff",
        "1_000",
        "latest",
        "18446744073709551616",
        "0x10000000000000000",
    ] {
        let argument = format!("--block-number={input}");
        let error = Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            &argument,
        ])
        .err()
        .unwrap();
        assert_eq!(error.kind(), ErrorKind::ValueValidation, "{input}");
    }
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--block-number",
            "16",
            "--block-hash",
            BLOCK,
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--chain-id",
            "1",
        ])
        .is_err()
    );
}

mod defaults;
mod root_terminal_coverage;
mod rpc_limits;

#[test]
fn every_execution_entry_accepts_only_native_numeric_selectors() {
    for input in [
        vec!["cfg", "--hex", "00"],
        vec!["ssa", "--file", "unused.hex"],
        vec!["analyze", "--world", "unused.json", "--evm.to", ENTRY],
        vec!["analyze", "--rpc", "http://127.0.0.1:1", "--evm.to", ENTRY],
        vec!["explain", "--hex", "00"],
        vec!["explain", "--world", "unused.json", "--evm.to", ENTRY],
        vec!["explain", "--rpc", "http://127.0.0.1:1", "--evm.to", ENTRY],
    ] {
        for model in ["product", "finite-set"] {
            let mut args = vec!["evm-abstract"];
            args.extend_from_slice(&input);
            args.extend(["--numeric-model", model]);
            let command = Cli::try_parse_from(&args).unwrap().command;
            assert_eq!(command.numeric().unwrap().numeric_model.to_string(), model);
        }
        for rejected in [
            ["--domain", "product"],
            ["--domain", "constants-only"],
            ["--numeric-model", "constants-only"],
            ["--numeric-model", "unknown"],
        ] {
            let mut args = vec!["evm-abstract"];
            args.extend_from_slice(&input);
            args.extend(rejected);
            assert!(Cli::try_parse_from(&args).is_err(), "{args:?}");
        }
    }
}

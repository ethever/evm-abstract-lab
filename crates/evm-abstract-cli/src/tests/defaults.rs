//! Parsed CLI inputs must reach the native machine with Web's complete policy.

use super::{Cli, Command, ENTRY};
use clap::Parser;
use evm_abstract::{
    analysis::{ExecutionConfig, control::Control},
    domain::{DomainSpec, ProductAlgebra, ValueAlgebra},
};
use evm_abstract_protocol::{AnalysisLimits, NumericModel};

fn assert_policy(actual: &ExecutionConfig, spec: DomainSpec, expected: &AnalysisLimits) {
    let config = &actual.analysis;
    for (field, actual, expected) in [
        ("constants", spec.capacity(), expected.max_constants),
        ("rounds", spec.reduction_rounds(), expected.reduction_rounds),
        ("facts", spec.fact_limit(), expected.max_facts),
        ("context", config.context_depth, expected.context_depth),
        ("states", config.max_states, expected.max_states),
        ("transfers", config.max_transfers, expected.max_transfers),
        ("work", actual.max_work, expected.max_work),
        ("call depth", actual.max_call_depth, expected.max_call_depth),
        ("memory", actual.max_memory_bytes, expected.max_memory_bytes),
        (
            "expression nodes",
            spec.relations().max_nodes,
            expected.max_expression_nodes,
        ),
        (
            "expression depth",
            spec.relations().max_depth,
            expected.max_expression_depth,
        ),
        (
            "constraints",
            spec.relations().max_constraints,
            expected.max_constraints,
        ),
    ] {
        assert_eq!(actual as u64, expected, "{field}");
    }
    assert_eq!(actual.use_summaries, expected.use_summaries);
    assert_eq!(
        actual.root_terminal_coverage,
        expected.root_terminal_coverage
    );
    assert_eq!(expected.numeric_model, NumericModel::Product);
    assert_eq!(spec.relations().enabled, expected.relations_enabled);
}

#[test]
fn raw_hex_file_and_offline_world_execute_with_web_defaults() {
    assert_eq!(AnalysisLimits::default().max_work, 2_000_000_000);
    assert!(AnalysisLimits::default().root_terminal_coverage);
    let file = format!(
        "{}/../../examples/straight-line.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    for command in ["cfg", "ssa"] {
        for source in [["--hex", "00"], ["--file", file.as_str()]] {
            let parsed =
                Cli::try_parse_from(["evm-abstract", command, source[0], source[1]]).unwrap();
            let args = match parsed.command {
                Command::Cfg { args, .. } | Command::Ssa { args, .. } => args,
                _ => panic!("unexpected raw command"),
            };
            let algebra =
                ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
            let analysis = args.analyze(&algebra, &Control::default()).unwrap();
            assert_policy(
                analysis.config(),
                analysis.domain_spec(),
                &AnalysisLimits::default(),
            );
        }
    }
    let world = format!(
        "{}/../../examples/worlds/call-return-branch.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let parsed = Cli::try_parse_from([
        "evm-abstract",
        "analyze",
        "--world",
        &world,
        "--evm.to",
        ENTRY,
    ])
    .unwrap();
    let Command::Analyze { args, .. } = parsed.command else {
        panic!("expected world analysis")
    };
    let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
    let analysis = args.analyze(&algebra, &Control::default()).unwrap();
    assert_policy(
        analysis.config(),
        analysis.domain_spec(),
        &AnalysisLimits::default(),
    );
}

#[test]
fn explicit_previous_policy_is_not_replaced_by_larger_defaults() {
    let expected = AnalysisLimits {
        max_constants: 8,
        reduction_rounds: 4,
        max_facts: 256,
        max_states: 4096,
        max_transfers: 100_000,
        max_work: 20_000_000,
        root_terminal_coverage: false,
        max_call_depth: 32,
        max_memory_bytes: 65_536,
        max_expression_nodes: 1024,
        max_expression_depth: 64,
        max_constraints: 128,
        ..AnalysisLimits::default()
    };
    for command in ["cfg", "ssa"] {
        let parsed = Cli::try_parse_from([
            "evm-abstract",
            command,
            "--hex",
            "00",
            "--max-constants",
            "8",
            "--reduction-rounds",
            "4",
            "--max-facts",
            "256",
            "--max-states",
            "4096",
            "--max-transfers",
            "100000",
            "--max-work",
            "20000000",
            "--no-root-terminal-coverage",
            "--max-call-depth",
            "32",
            "--max-memory-bytes",
            "65536",
            "--max-symbolic-nodes",
            "1024",
            "--max-symbolic-depth",
            "64",
            "--max-relations",
            "128",
        ])
        .unwrap();
        let args = match parsed.command {
            Command::Cfg { args, .. } | Command::Ssa { args, .. } => args,
            _ => panic!("unexpected raw command"),
        };
        let algebra = ValueAlgebra::new(ProductAlgebra::from_spec(args.numeric.spec().unwrap()));
        let analysis = args.analyze(&algebra, &Control::default()).unwrap();
        assert_policy(analysis.config(), analysis.domain_spec(), &expected);
    }
}

#[test]
fn rpc_cli_defaults_match_the_web_acquisition_policy_and_keep_explicit_values() {
    let defaults = AnalysisLimits::default();
    for explicit in [false, true] {
        let mut input = vec![
            "evm-abstract",
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
        ];
        if explicit {
            input.extend([
                "--max-rpc-accounts",
                "256",
                "--max-rpc-requests",
                "16384",
                "--max-rpc-response-bytes",
                "4194304",
                "--rpc-timeout-ms",
                "15000",
            ]);
        }
        let parsed = Cli::try_parse_from(input).unwrap();
        let Command::Analyze { args, .. } = parsed.command else {
            panic!("expected RPC analysis")
        };
        let expected = if explicit {
            [256, 16_384, 4 * 1024 * 1024, 15_000]
        } else {
            [
                defaults.rpc_max_accounts,
                defaults.rpc_max_requests,
                defaults.rpc_max_response_bytes,
                defaults.rpc_timeout_ms,
            ]
        };
        assert_eq!(
            [
                args.max_rpc_accounts as u64,
                args.max_rpc_requests as u64,
                args.max_rpc_response_bytes as u64,
                args.rpc_timeout_ms
            ],
            expected
        );
    }
}

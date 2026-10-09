//! Parsed CLI inputs must reach the native machine with Web's complete policy.

use super::{Cli, Command, ENTRY};
use clap::Parser;
use evm_abstract::{
    analysis::ExecutionConfig,
    domain::{Profile, relational::SmtProvider},
};
use evm_abstract_protocol::{AnalysisLimits, DomainProfile};

fn assert_policy(actual: &ExecutionConfig, expected: &AnalysisLimits) {
    let config = &actual.analysis;
    for (field, actual, expected) in [
        ("constants", config.max_constants, expected.max_constants),
        ("rounds", config.reduction_rounds, expected.reduction_rounds),
        ("facts", config.max_facts, expected.max_facts),
        ("context", config.context_depth, expected.context_depth),
        ("states", config.max_states, expected.max_states),
        ("transfers", config.max_transfers, expected.max_transfers),
        ("work", actual.max_work, expected.max_work),
        ("call depth", actual.max_call_depth, expected.max_call_depth),
        ("memory", actual.max_memory_bytes, expected.max_memory_bytes),
        (
            "expression nodes",
            config.relations.max_nodes,
            expected.max_expression_nodes,
        ),
        (
            "expression depth",
            config.relations.max_depth,
            expected.max_expression_depth,
        ),
        (
            "constraints",
            config.relations.max_constraints,
            expected.max_constraints,
        ),
    ] {
        assert_eq!(actual as u64, expected, "{field}");
    }
    assert_eq!(actual.use_summaries, expected.use_summaries);
    assert_eq!(
        config.domain_profile,
        match expected.domain_profile {
            DomainProfile::Product => Profile::Product,
            DomainProfile::ConstantsOnly => Profile::ConstantsOnly,
        }
    );
    assert_eq!(config.relations.enabled, expected.relations_enabled);
    assert_eq!(config.relations.rlimit, expected.smt_rlimit);
    assert_eq!(
        config.relations.provider,
        match expected.smt_provider {
            evm_abstract_protocol::SmtProvider::Z3 => SmtProvider::Z3,
            evm_abstract_protocol::SmtProvider::Bitwuzla => SmtProvider::Bitwuzla,
            evm_abstract_protocol::SmtProvider::Cvc5 => SmtProvider::Cvc5,
        }
    );
}

#[test]
fn raw_hex_file_and_offline_world_execute_with_web_defaults() {
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
            let analysis = args.analyze().unwrap();
            assert_policy(analysis.execution().config(), &AnalysisLimits::default());
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
    let analysis = args.analyze().unwrap();
    assert_policy(analysis.config(), &AnalysisLimits::default());
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
        max_call_depth: 32,
        max_memory_bytes: 65_536,
        max_expression_nodes: 1024,
        max_expression_depth: 64,
        max_constraints: 128,
        smt_rlimit: 100_000,
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
            "--smt.rlimit",
            "100000",
        ])
        .unwrap();
        let args = match parsed.command {
            Command::Cfg { args, .. } | Command::Ssa { args, .. } => args,
            _ => panic!("unexpected raw command"),
        };
        assert_policy(args.analyze().unwrap().execution().config(), &expected);
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

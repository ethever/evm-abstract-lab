//! Unified explain is exercised through the real binary, including pinned RPC.

use super::run_concrete;
use alloy_primitives::{U256, hex, keccak256};
use serde_json::{Value as Json, json};
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::Output,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const CALLEE: &str = "0x0000000000000000000000000000000000000200";
const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
const VERIFIED: &str = "Verified cross-contract SSA:";

fn fixture(name: &str) -> String {
    format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn world_command(command: &str, name: &str, extra: &[&str]) -> Output {
    let path = fixture(name);
    let mut args = vec![command, "--world", &path, "--evm.to", ENTRY];
    if command == "explain" {
        args.push("--verbose");
    }
    args.extend_from_slice(extra);
    run_concrete(&args)
}

fn text(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_full_report(explanation: &str, report: &Output) {
    let report = text(report);
    assert!(
        explanation.contains(report.trim_end()),
        "explain must preserve the complete analyze report"
    );
    for section in [
        "Analysis",
        "Snapshot",
        "References",
        "States",
        "State details",
        "Transitions",
        "Outcomes",
        "Call summaries",
        "Diagnostics",
        "Frontiers",
    ] {
        assert!(
            explanation.lines().any(|line| line.trim() == section),
            "missing {section}"
        );
    }
}

fn assert_human_ssa(explanation: &str) {
    let (_, ssa) = explanation.split_once(VERIFIED).unwrap();
    assert!(ssa.contains('%'), "SSA value names are missing");
    assert!(ssa.contains('!'), "SSA effect names are missing");
    assert!(
        ssa.contains("transitions=0")
            || ssa.split_whitespace().any(|word| {
                word.strip_prefix('T').is_some_and(|rest| {
                    rest.starts_with(|character: char| character.is_ascii_digit())
                })
            }),
        "SSA transition names are missing"
    );
    assert!(!ssa.trim_start().starts_with('{'));
    assert!(!ssa.contains("\"blocks\":") && !ssa.contains("\"transitions\":"));
}

#[test]
fn world_explain_combines_actual_disassembly_complete_report_and_readable_ssa() {
    for (name, instruction) in [
        ("call-return-branch", "MLOAD"),
        ("returndata-copy", "RETURNDATACOPY"),
    ] {
        let output = world_command("explain", name, &[]);
        success(&output);
        let explanation = text(&output);
        let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
        assert!(disassembly.contains("PUSH1") && disassembly.contains("RETURN"));
        assert!(disassembly.contains(instruction));
        assert_full_report(&explanation, &world_command("analyze", name, &[]));
        assert!(explanation.contains("Call") && explanation.contains("Return"));
        assert_human_ssa(&explanation);
    }
}

#[test]
fn world_explain_keeps_code_identity_separate_from_state_ownership() {
    let output = world_command("explain", "proxy-storage", &[]);
    success(&output);
    let explanation = text(&output);
    for address in [
        ENTRY,
        "0x0000000000000000000000000000000000000201",
        "0x0000000000000000000000000000000000000202",
        "0x0000000000000000000000000000000000000300",
    ] {
        assert!(explanation.contains(address), "missing {address}");
    }
    let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
    assert!(disassembly.contains("DELEGATECALL") && disassembly.contains("SSTORE"));
    assert_full_report(
        &explanation,
        &world_command("analyze", "proxy-storage", &[]),
    );
    assert_human_ssa(&explanation);
}

#[test]
fn world_explain_disassembles_captured_initcode_and_created_runtime() {
    let output = world_command("explain", "create-runtime", &[]);
    success(&output);
    let explanation = text(&output);
    let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
    assert!(disassembly.contains("InitCode") && disassembly.contains("Runtime"));
    assert!(disassembly.contains("CREATE") && disassembly.contains("MSTORE"));
    assert!(
        disassembly
            .to_ascii_lowercase()
            .contains("0xea53a153a9a04fd632b2486d84732feb3b71afb7")
    );
    assert_full_report(
        &explanation,
        &world_command("analyze", "create-runtime", &[]),
    );
}

#[test]
fn incomplete_world_explain_keeps_disassembly_and_every_report_section() {
    for (name, extra, frontier) in [
        ("missing-code", vec![], "MissingCode"),
        ("call-return-branch", vec!["--max-work", "1"], "Work"),
        (
            "call-return-branch",
            vec!["--max-memory-bytes", "1"],
            "Memory",
        ),
        ("reentry", vec!["--max-call-depth", "2"], "CallDepth"),
        ("call-return-branch", vec!["--max-states", "1"], "States"),
        (
            "call-return-branch",
            vec!["--max-transfers", "1"],
            "Transfers",
        ),
    ] {
        let output = world_command("explain", name, &extra);
        assert_eq!(output.status.code(), Some(2), "{name} {extra:?}");
        let explanation = text(&output);
        assert_full_report(&explanation, &world_command("analyze", name, &extra));
        let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
        assert!(disassembly.contains("PUSH") || disassembly.contains("CALL"));
        if extra == ["--max-work", "1"] {
            assert!(disassembly.contains("Input code observations (not execution evidence)"));
            assert!(!disassembly.contains("Captured instruction list"));
        }
        assert!(explanation.contains("Incomplete") && explanation.contains(frontier));
        assert!(explanation.contains("SSA unavailable"));
        assert!(!explanation.contains(VERIFIED));
    }
}

#[test]
fn legacy_hex_and_file_explain_keep_defaults_and_explicit_fork_behavior() {
    let path = format!(
        "{}/../../examples/straight-line.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    let code = fs::read_to_string(&path).unwrap();
    let file = run_concrete(&["explain", "--file", &path]);
    success(&file);
    let hex = run_concrete(&["explain", "--hex", &code]);
    success(&hex);
    assert_eq!(file.stdout, hex.stdout);
    let explicit = run_concrete(&[
        "explain",
        "--hex",
        &code,
        "--fork",
        "osaka",
        "--context-depth",
        "8",
        "--max-constants",
        "8",
        "--domain",
        "product",
        "--reduction-rounds",
        "4",
        "--max-facts",
        "256",
        "--max-states",
        "4096",
        "--max-transfers",
        "100000",
    ]);
    success(&explicit);
    assert_eq!(hex.stdout, explicit.stdout);
    let explanation = text(&hex);
    assert!(explanation.contains("context_depth=8") && explanation.contains("domain=Product"));
    assert!(explanation.contains("stack SSA:") && !explanation.contains(VERIFIED));
    for fork in ["cancun", "prague", "osaka"] {
        let output = run_concrete(&["explain", "--hex", "00", "--fork", fork]);
        success(&output);
        assert!(text(&output).contains(&format!("fork={fork}")));
    }
}

#[test]
fn legacy_explain_still_reports_resource_frontiers_without_ssa() {
    let output = run_concrete(&["explain", "--hex", "600035565b00", "--max-transfers", "1"]);
    assert_eq!(output.status.code(), Some(2));
    let explanation = text(&output);
    assert!(explanation.contains("PUSH1") && explanation.contains("status=Incomplete"));
    assert!(explanation.contains("SSA unavailable") && !explanation.contains("stack SSA:"));
}

#[test]
fn explain_requires_one_source_and_rejects_every_pair_of_sources() {
    let path = fixture("call-return-branch");
    let sources = [
        ["--hex", "00"],
        ["--file", "unused.hex"],
        ["--world", path.as_str()],
        ["--rpc", "http://127.0.0.1:1"],
    ];
    assert_eq!(run_concrete(&["explain"]).status.code(), Some(2));
    for (left_index, left) in sources.iter().enumerate() {
        for right in &sources[left_index + 1..] {
            let mut args = vec!["explain", "--evm.to", ENTRY];
            args.extend_from_slice(left);
            args.extend_from_slice(right);
            if left[0] == "--rpc" || right[0] == "--rpc" {
                args.extend(["--block-hash", BLOCK]);
            }
            let output = run_concrete(&args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(output.stdout.is_empty());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains(left[0]) && error.contains(right[0]),
                "{error}"
            );
        }
    }
}

#[test]
fn world_and_rpc_explain_require_entry_and_block_selectors_are_exclusive() {
    let path = fixture("call-return-branch");
    for args in [
        vec!["explain", "--world", &path],
        vec!["explain", "--rpc", "http://127.0.0.1:1"],
        vec![
            "explain",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--block-hash",
            BLOCK,
            "--block-number",
            "16",
        ],
    ] {
        let output = run_concrete(&args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains("transport"));
    }
}

#[test]
fn rpc_observation_flags_remain_exclusive_to_rpc_and_world_selects_its_own_fork() {
    let path = fixture("call-return-branch");
    let slot = format!("{ENTRY}:0");
    for source in [
        ["--hex", "00"],
        ["--file", "unused.hex"],
        ["--world", &path],
    ] {
        for flag in [
            vec!["--block-number", "16"],
            vec!["--block-hash", BLOCK],
            vec!["--account", CALLEE],
            vec!["--slot", &slot],
            vec!["--no-rpc-discovery"],
            vec!["--max-rpc-accounts", "256"],
            vec!["--max-rpc-requests", "16384"],
        ] {
            let mut args = vec!["explain"];
            if source[0] == "--world" {
                args.extend(["--evm.to", ENTRY]);
            }
            args.extend(source);
            args.extend_from_slice(&flag);
            let output = run_concrete(&args);
            assert_eq!(output.status.code(), Some(2), "{args:?}");
            assert!(output.stdout.is_empty());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains(flag[0]) && (error.contains("--rpc") || error.contains(source[0])),
                "{error}"
            );
        }
    }
    let output = world_command("explain", "call-return-branch", &["--fork", "osaka"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
}

#[test]
fn invalid_rpc_block_number_and_hash_are_rejected_before_network_access() {
    for number in ["-1", "1.5", "ff", "0xzz", "18446744073709551616"] {
        let block_arg = format!("--block-number={number}");
        let output = run_concrete(&[
            "explain",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            &block_arg,
        ]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--block-number"));
    }
    for hash in ["latest", "0x1", "0xffff", "zz"] {
        let output = run_concrete(&[
            "explain",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            ENTRY,
            "--block-hash",
            hash,
        ]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("32-byte block hash") && !error.contains("transport"));
    }
}

struct TemporaryWorld(PathBuf);

impl TemporaryWorld {
    fn new(contents: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-abstract-explain-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, contents).unwrap();
        Self(path)
    }

    fn run_concrete(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            command,
            "--world",
            self.0.to_str().unwrap(),
            "--evm.to",
            ENTRY,
        ];
        if command == "explain" {
            args.push("--verbose");
        }
        args.extend_from_slice(extra);
        run_concrete(&args)
    }
}

impl Drop for TemporaryWorld {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn invalid_world_json_and_entry_environment_fail_without_partial_output() {
    let malformed = TemporaryWorld::new("{\"fork\":");
    let output = malformed.run_concrete("explain", &[]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid world JSON"));
    for (extra, diagnostic) in [
        (vec!["--evm.to", "0x01"], "EVM environment address"),
        (vec!["--evm.caller", "0x01"], "EVM environment address"),
        (vec!["--evm.calldata", "0xzz"], "calldata hex"),
    ] {
        let path = fixture("call-return-branch");
        let mut args = vec!["explain", "--world", &path];
        if extra[0] != "--evm.to" {
            args.extend(["--evm.to", ENTRY]);
        }
        args.extend_from_slice(&extra);
        let output = run_concrete(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(diagnostic));
    }
}

#[test]
fn world_explain_uses_entry_environment_domain_and_every_execution_budget_flag() {
    // Read calldata length/word, value and caller without changing state.
    let world = TemporaryWorld::new(
        &json!({
            "fork":"osaka", "provenance":"synthetic:explain-environment",
            "accounts":[{"address":ENTRY,"code":"0x365f35343300"}]
        })
        .to_string(),
    );
    let extra = [
        "--evm.caller",
        CALLEE,
        "--evm.calldata",
        "0x2a",
        "--evm.value",
        "7",
        "--evm.static",
        "--no-summaries",
        "--max-call-depth",
        "5",
        "--max-work",
        "20000000",
        "--max-memory-bytes",
        "65536",
        "--max-constants",
        "1",
        "--domain",
        "constants-only",
        "--reduction-rounds",
        "1",
        "--max-facts",
        "1",
        "--context-depth",
        "0",
        "--max-states",
        "4096",
        "--max-transfers",
        "100000",
    ];
    let output = world.run_concrete("explain", &extra);
    success(&output);
    let explanation = text(&output);
    assert_full_report(&explanation, &world.run_concrete("analyze", &extra));
    assert!(explanation.contains("ConstantsOnly") && explanation.contains(CALLEE));
    assert!(explanation.contains("CALLDATASIZE") && explanation.contains("CALLVALUE"));
    assert_human_ssa(&explanation);
    let mut json_extra = extra.to_vec();
    json_extra.extend(["--format", "json"]);
    let analyzed = world.run_concrete("analyze", &json_extra);
    success(&analyzed);
    let analyzed: Json = serde_json::from_slice(&analyzed.stdout).unwrap();
    assert_eq!(analyzed["entry"]["environment"]["is_static"], true);
    assert_eq!(
        analyzed["entry"]["environment"]["caller"]["Concrete"],
        CALLEE
    );
    assert_eq!(
        analyzed["entry"]["environment"]["value"]["Constants"],
        json!(["0x7"])
    );
    assert_eq!(
        analyzed["entry"]["environment"]["calldata"]["length"]["Constants"],
        json!(["0x1"])
    );
    assert_eq!(analyzed["config"]["use_summaries"], false);
    assert_eq!(
        analyzed["config"]["analysis"]["domain_profile"],
        "constants-only"
    );
    assert_eq!(analyzed["config"]["analysis"]["context_depth"], 0);
    assert_eq!(analyzed["config"]["max_call_depth"], 5);
}

#[test]
fn world_explain_static_entry_restriction_is_visible_in_the_result() {
    let world = TemporaryWorld::new(
        &json!({
            "fork":"osaka", "provenance":"synthetic:explain-static", "accounts":[{
                "address":ENTRY,"code":"0x60015f5500",
                "storage_unknown":false
            }]
        })
        .to_string(),
    );
    let output = world.run_concrete("explain", &["--evm.static"]);
    success(&output);
    let explanation = text(&output);
    assert_full_report(
        &explanation,
        &world.run_concrete("analyze", &["--evm.static"]),
    );
    assert!(explanation.contains("Failure"));
    assert_human_ssa(&explanation);
    let analyzed = world.run_concrete("analyze", &["--evm.static", "--format", "json"]);
    success(&analyzed);
    let analyzed: Json = serde_json::from_slice(&analyzed.stdout).unwrap();
    assert_eq!(analyzed["entry"]["environment"]["is_static"], true);
    assert!(
        analyzed["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|outcome| outcome["kind"] == "Failure")
    );
}

#[test]
fn invalid_domain_policy_is_rejected_in_program_and_world_explain() {
    let path = fixture("call-return-branch");
    for source in [["--hex", "00"], ["--world", &path]] {
        for flag in ["--max-facts", "--reduction-rounds"] {
            let mut args = vec!["explain"];
            if source[0] == "--world" {
                args.extend(["--evm.to", ENTRY]);
            }
            args.extend(source);
            args.extend([flag, "0"]);
            let output = run_concrete(&args);
            assert_eq!(output.status.code(), Some(1));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("must be positive"));
        }
    }
}

#[test]
fn rpc_explain_acquires_one_fixed_world_and_uses_the_requested_observations() {
    thread::scope(|scope| {
        let server = RpcServer::new(scope);
        let slot = format!("{ENTRY}:16");
        let output = run_concrete(&[
            "explain",
            "--rpc",
            &server.endpoint,
            "--block-hash",
            BLOCK,
            "--evm.to",
            ENTRY,
            "--account",
            CALLEE,
            "--slot",
            &slot,
            "--fork",
            "cancun",
            "--evm.value",
            "7",
            "--evm.static",
        ]);
        success(&output);
        let explanation = text(&output);
        assert!(explanation.contains(BLOCK));
        assert!(explanation.contains("CALLVALUE") && explanation.contains("SLOAD"));
        assert!(explanation.contains("0x2a") && explanation.contains("0x7"));
        assert!(explanation.contains("cancun") && explanation.contains(CALLEE));
        assert_human_ssa(&explanation);
        let requests = server.finish();
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "eth_chainId")
                .count(),
            2
        );
        assert_eq!(
            requests
                .iter()
                .filter(|r| r["method"] == "eth_getBlockByHash")
                .count(),
            2
        );
        for address in [ENTRY, CALLEE] {
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| {
                        request["method"] == "eth_getCode" && request["params"][0] == address
                    })
                    .count(),
                1,
                "one fixed world acquisition must fetch each account once"
            );
        }
        assert!(
            requests
                .iter()
                .any(|r| r["method"] == "eth_getCode" && r["params"][0] == CALLEE)
        );
        assert!(
            requests
                .iter()
                .any(|r| r["method"] == "eth_getStorageAt" && r["params"][1] == "0x10")
        );
        assert_pinned(&requests);
    });
}

fn rpc_command(server: &RpcServer, extra: &[&str]) -> Output {
    rpc_command_named("explain", server, extra)
}

fn rpc_command_named(command: &str, server: &RpcServer, extra: &[&str]) -> Output {
    let mut args = vec![
        command,
        "--rpc",
        &server.endpoint,
        "--block-hash",
        BLOCK,
        "--evm.to",
        ENTRY,
    ];
    if command == "explain" {
        args.push("--verbose");
    }
    args.extend_from_slice(extra);
    run_concrete(&args)
}

fn assert_rpc_report<'scope, 'env>(
    scope: &'scope thread::Scope<'scope, 'env>,
    explanation: &str,
    extra: &[&str],
    expected_exit: i32,
) {
    let oracle = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
    let report = rpc_command_named("analyze", &oracle, extra);
    assert_eq!(report.status.code(), Some(expected_exit));
    assert_full_report(explanation, &report);
    assert_pinned(&oracle.finish());
}

fn requests_for(requests: &[Json], method: &str, address: &str) -> usize {
    requests
        .iter()
        .filter(|request| request["method"] == method && request["params"][0] == address)
        .count()
}

fn assert_pinned(requests: &[Json]) {
    for request in requests {
        match request["method"].as_str().unwrap() {
            "eth_chainId" => assert_eq!(request["params"], json!([])),
            "eth_getBlockByHash" => assert_eq!(request["params"], json!([BLOCK, false])),
            _ => assert_eq!(
                request["params"].as_array().unwrap().last().unwrap(),
                &json!({"blockHash":BLOCK,"requireCanonical":true})
            ),
        }
        assert_ne!(request["method"], "eth_getProof");
        assert!(!request.to_string().contains("latest"));
    }
}

#[test]
fn rpc_explain_discovers_missing_callee_snapshot_code_with_default_and_explicit_limits() {
    thread::scope(|scope| {
        let mut explanations = Vec::new();
        for extra in [
            vec![],
            vec!["--max-rpc-accounts", "256", "--max-rpc-requests", "16384"],
            vec!["--max-rpc-accounts", "4097", "--max-rpc-requests", "65537"],
        ] {
            let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
            let output = rpc_command(&server, &extra);
            success(&output);
            let explanation = text(&output);
            let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
            assert!(disassembly.contains(ENTRY) && disassembly.contains(CALLEE));
            assert!(disassembly.contains("CALL") && disassembly.contains("MSTORE"));
            assert!(
                disassembly.contains(&keccak256(hex::decode(RPC_CALLEE_CODE).unwrap()).to_string())
            );
            assert!(
                explanation.contains("RPC acquisition")
                    && explanation.contains("fetched accounts=")
            );
            assert!(explanation.contains("0x2a") && explanation.contains("Converged"));
            assert_human_ssa(&explanation);
            let requests = server.finish();
            for address in [ENTRY, CALLEE] {
                for method in ["eth_getCode", "eth_getBalance", "eth_getTransactionCount"] {
                    assert_eq!(requests_for(&requests, method, address), 1);
                }
            }
            assert_pinned(&requests);
            assert_rpc_report(scope, &explanation, &extra, 0);
            explanations.push(explanation);
        }
        assert_eq!(explanations[0], explanations[1]);
    });
}

#[test]
fn rpc_explain_discovery_off_keeps_missing_code_partial_graph_without_verified_ssa() {
    thread::scope(|scope| {
        let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
        let output = rpc_command(&server, &["--no-rpc-discovery"]);
        assert_eq!(output.status.code(), Some(2));
        let explanation = text(&output);
        assert!(explanation.contains("Incomplete") && explanation.contains("MissingCode"));
        assert!(explanation.contains("Frontiers") && explanation.contains("SSA unavailable"));
        assert!(!explanation.contains(VERIFIED));
        let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
        assert!(disassembly.contains("CALL") && !disassembly.contains("MSTORE"));
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getCode", ENTRY), 1);
        assert_eq!(requests_for(&requests, "eth_getCode", CALLEE), 0);
        assert_pinned(&requests);
        assert_rpc_report(scope, &explanation, &["--no-rpc-discovery"], 2);
    });
}

#[test]
fn rpc_explain_explicit_account_closes_the_graph_with_discovery_disabled() {
    thread::scope(|scope| {
        let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
        let output = rpc_command(&server, &["--no-rpc-discovery", "--account", CALLEE]);
        success(&output);
        let explanation = text(&output);
        let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
        assert!(disassembly.contains("MSTORE") && disassembly.contains(CALLEE));
        assert_human_ssa(&explanation);
        let requests = server.finish();
        assert_eq!(requests_for(&requests, "eth_getCode", CALLEE), 1);
        assert_pinned(&requests);
        assert_rpc_report(
            scope,
            &explanation,
            &["--no-rpc-discovery", "--account", CALLEE],
            0,
        );
    });
}

#[test]
fn rpc_explain_discovery_caps_keep_typed_acquisition_frontiers_and_partial_output() {
    thread::scope(|scope| {
        for (flag, limit, resource, callee_requests, requests_limit) in [
            ("--max-rpc-accounts", "1", "Accounts", 0, 7),
            ("--max-rpc-requests", "7", "Requests", 0, 7),
            // Observations arrive, but final identity checks cannot complete.
            ("--max-rpc-requests", "12", "Requests", 1, 12),
        ] {
            let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
            let output = rpc_command(&server, &[flag, limit]);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{flag}={limit}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let explanation = text(&output);
            assert!(explanation.contains("Incomplete") && explanation.contains("RpcAcquisition"));
            assert!(explanation.contains("AcquisitionLimit") && explanation.contains(resource));
            assert!(explanation.contains("RPC acquisition") && explanation.contains("Frontiers"));
            assert!(explanation.contains("SSA unavailable") && !explanation.contains(VERIFIED));
            let (disassembly, _) = explanation.split_once("Analysis\n").unwrap();
            assert!(disassembly.contains("CALL") && !disassembly.contains("MSTORE"));
            let requests = server.finish();
            assert_eq!(
                requests_for(&requests, "eth_getCode", CALLEE),
                callee_requests
            );
            assert!(requests.len() <= requests_limit);
            assert_pinned(&requests);
            assert_rpc_report(scope, &explanation, &[flag, limit], 2);
        }
    });
}

#[test]
fn rpc_explain_invalid_acquisition_limits_fail_before_networking() {
    thread::scope(|scope| {
        let server = RpcServer::with_fixture(scope, RpcFixture::CallReturn);
        for (extra, diagnostic) in [
            (
                vec!["--max-rpc-requests", "0"],
                "request limit must be positive",
            ),
            (vec!["--max-rpc-accounts", "0"], "must be positive"),
            (
                vec!["--max-rpc-accounts", "1", "--account", CALLEE],
                "fit the account limit",
            ),
        ] {
            let output = rpc_command(&server, &extra);
            assert_eq!(output.status.code(), Some(1), "{extra:?}");
            assert!(output.stdout.is_empty());
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains(diagnostic) && !error.contains("transport"),
                "{error}"
            );
        }
        assert!(server.finish().is_empty());
    });
}

const RPC_ROOT_CODE: &str = "60205f5f5f5f6102006207a120f15060205ff3";
const RPC_CALLEE_CODE: &str = "602a5f5260205ff3";

#[derive(Clone, Copy)]
pub(super) enum RpcFixture {
    Observations,
    CallReturn,
    UnknownTarget,
}

pub(super) struct RpcServer {
    pub(super) endpoint: String,
    stop: Option<mpsc::Sender<()>>,
    done: Option<mpsc::Receiver<Vec<Json>>>,
}

impl RpcServer {
    fn new<'scope, 'env>(scope: &'scope thread::Scope<'scope, 'env>) -> Self {
        Self::with_fixture(scope, RpcFixture::Observations)
    }

    pub(super) fn with_fixture<'scope, 'env>(
        scope: &'scope thread::Scope<'scope, 'env>,
        fixture: RpcFixture,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let (stop, stopped) = mpsc::channel();
        let (completed, done) = mpsc::channel();
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
                stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                let mut header = Vec::new();
                let mut byte = [0];
                while !header.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    header.push(byte[0]);
                    assert!(header.len() <= 8192);
                }
                let header = String::from_utf8(header).unwrap();
                let length = header.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                }).unwrap();
                assert!(length <= 8192);
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                let request: Json = serde_json::from_slice(&body).unwrap();
                let response = serde_json::to_vec(&json!({
                    "jsonrpc":"2.0","id":request["id"],"result":rpc_result(&request, fixture)
                })).unwrap();
                write!(stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                ).unwrap();
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

    pub(super) fn finish(mut self) -> Vec<Json> {
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

fn rpc_result(request: &Json, fixture: RpcFixture) -> Json {
    let code = match fixture {
        RpcFixture::Observations => "3460105400",
        RpcFixture::UnknownTarget => "5f5f5f5f5f33600f1660f0175af1",
        RpcFixture::CallReturn if request["params"][0] == ENTRY => RPC_ROOT_CODE,
        RpcFixture::CallReturn => RPC_CALLEE_CODE,
    };
    match request["method"].as_str().unwrap() {
        "eth_chainId" => json!("0x1"),
        "eth_getBlockByHash" => {
            assert_eq!(request["params"], json!([BLOCK, false]));
            json!({"hash":BLOCK,"number":"0x10", "parentHash":BLOCK, "timestamp":"0x1234", "miner":"0x0000000000000000000000000000000000000033", "mixHash":BLOCK, "gasLimit":"0x1c9c380", "baseFeePerGas":"0x7"})
        }
        "eth_getCode" => json!(format!("0x{code}")),
        "eth_getBalance" => json!("0x1000000"),
        "eth_getTransactionCount" => json!("0x0"),
        "eth_getStorageAt" => json!(format!("0x{:064x}", U256::from(42))),
        method => panic!("unexpected mock RPC method: {method}"),
    }
}

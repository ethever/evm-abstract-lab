//! Actual command defaults and environment overrides reach the native engine.

use alloy_primitives::U256;
use serde_json::{Value as Json, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

const TO: &str = "0x0000000000000000000000000000000000000101";
const CALLER: &str = "0x0000000000000000000000000000000000001000";
const ORIGIN: &str = "0x0000000000000000000000000000000000002000";
const CALLEE: &str = "0x0000000000000000000000000000000000000200";
const COINBASE: &str = "0x0000000000000000000000000000000000003000";
const HASH: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
const OTHER_HASH: &str = "0x2222222222222222222222222222222222222222222222222222222222222222";
// ADDRESS; ORIGIN; CALLER; CALLVALUE; CALLDATASIZE; CALLDATALOAD(0); GASPRICE; COINBASE;
// TIMESTAMP; NUMBER; PREVRANDAO; GASLIMIT; CHAINID; BASEFEE; BLOBBASEFEE;
// BLOCKHASH(16); BLOBHASH(0); GAS.
const READ_ENV: &str = "30323334365f353a414243444546484a6010405f495a";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .args(args)
        .output()
        .unwrap()
}

fn success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn raw(command: &str, code: &str, extra: &[&str]) -> Json {
    let mut args = vec![command, "--hex", code, "--format", "json"];
    args.extend_from_slice(extra);
    let output = run(&args);
    success(&output);
    let result: Json = serde_json::from_slice(&output.stdout).unwrap();
    if command == "ssa" {
        result["analysis"].clone()
    } else {
        result
    }
}

fn constant(value: &Json, expected: U256) {
    assert_eq!(
        value["Constants"],
        json!([format!("{expected:#x}")]),
        "{value}"
    );
}

fn word(address: &str) -> U256 {
    address.parse().unwrap()
}

fn assert_symbolic_environment(analysis: &Json) {
    let environment = &analysis["environment"];
    assert!(
        environment.is_object(),
        "raw analysis must record its input scope"
    );
    assert_eq!(environment["to"], json!({"Symbolic":"To"}));
    assert_eq!(environment["caller"], json!({"Symbolic":"Caller"}));
    assert!(environment["origin"].is_null());
    assert_eq!(environment["coinbase"], json!({"Symbolic":"Coinbase"}));
    for field in [
        "value",
        "gas_price",
        "timestamp",
        "number",
        "prevrandao",
        "gas_limit",
        "base_fee",
        "blob_base_fee",
    ] {
        assert!(!environment[field].is_null(), "missing {field}");
        assert!(
            environment[field]["Constants"].is_null(),
            "{field} must remain symbolic"
        );
    }
    assert!(!environment["calldata"].is_null());
    assert!(environment["calldata"]["length"]["Constants"].is_null());
    assert!(environment["calldata"]["default"]["Constants"].is_null());
    assert!(environment["chain_id"].is_null());
    assert_eq!(environment["gas"], "Unknown");
    assert_eq!(environment["is_static"], false);
    assert_eq!(environment["block_hashes"], json!({}));
    assert!(environment["blob_hashes"]["length"]["Constants"].is_null());
    assert_eq!(environment["blob_hashes"]["hashes"], json!({}));
    assert!(environment.get("input_scope").is_none());
}

fn assert_supplied_environment(analysis: &Json) {
    let environment = &analysis["environment"];
    assert!(
        environment.is_object(),
        "raw analysis must record supplied inputs"
    );
    for (field, address) in [
        ("to", TO),
        ("caller", CALLER),
        ("origin", ORIGIN),
        ("coinbase", COINBASE),
    ] {
        assert_eq!(environment[field], json!({"Concrete":address}));
    }
    for (field, value) in [
        ("value", 9),
        ("gas_price", 10),
        ("timestamp", 11),
        ("number", 17),
        ("prevrandao", 12),
        ("gas_limit", 13),
        ("chain_id", 14),
        ("base_fee", 15),
        ("blob_base_fee", 16),
    ] {
        constant(&environment[field], U256::from(value));
    }
    constant(&environment["calldata"]["length"], U256::from(1));
    constant(&environment["calldata"]["bytes"]["0"], U256::from(42));
    assert_eq!(environment["gas"], json!({"UpperBound":"0xf4240"}));
    assert_eq!(environment["is_static"], true);
    assert_eq!(environment["block_hashes"], json!({"0x10":HASH}));
    constant(&environment["blob_hashes"]["length"], U256::from(1));
    assert_eq!(environment["blob_hashes"]["hashes"], json!({"0x0":HASH}));
    assert!(environment.get("input_scope").is_none());
}

struct World(PathBuf);

impl World {
    fn new(root: &str, callee: Option<&str>) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-cli-environment-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        let mut accounts = vec![
            json!({"address":TO,"code":format!("0x{root}"),"balance":"0x1000000","nonce":"0x0","existence":"present"}),
        ];
        if let Some(code) = callee {
            accounts.push(json!({"address":CALLEE,"code":format!("0x{code}"),"balance":"0x0","nonce":"0x0","existence":"present"}));
        }
        fs::write(
            &path,
            json!({"fork":"osaka","provenance":"synthetic:cli-environment","accounts":accounts})
                .to_string(),
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = vec![command, "--world", self.0.to_str().unwrap(), "--evm.to", TO];
        if command == "analyze" {
            args.extend(["--format", "json"]);
        }
        args.extend_from_slice(extra);
        run(&args)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn raw_and_world_defaults_keep_inputs_symbolic_and_origin_aliases_caller() {
    // Keep two distinct constraints: shared sender/origin and arbitrary value,
    // arbitrary calldata bytes/length, and an unknown raw ADDRESS.
    for command in ["cfg", "ssa"] {
        for profile in ["product", "constants-only"] {
            let analysis = raw(command, "33321434365f353000", &["--domain", profile]);
            assert_symbolic_environment(&analysis);
            let stack = analysis["states"][0]["exit_stack"].as_array().unwrap();
            constant(&stack[0], U256::from(1));
            for value in &stack[1..] {
                assert!(value["Constants"].is_null(), "{value}");
            }
        }
    }
    let world = World::new("33321434365f353000", None);
    let output = world.run("analyze", &[]);
    success(&output);
    let analysis: Json = serde_json::from_slice(&output.stdout).unwrap();
    let environment = &analysis["entry"]["environment"];
    assert!(environment["value"]["Constants"].is_null());
    assert!(environment["calldata"]["length"]["Constants"].is_null());
    assert!(environment["origin"].is_null());
    assert_eq!(environment["to"], json!({"Concrete":TO}));
    assert_eq!(environment["caller"], json!({"Symbolic":"Caller"}));
    let stack = analysis["states"][0]["exit_stack"].as_array().unwrap();
    constant(&stack[0], U256::from(1));
    for value in &stack[1..4] {
        assert!(value["Constants"].is_null(), "{value}");
    }
    constant(&stack[4], word(TO));
    for source in [
        vec!["--hex", "33321434365f353000"],
        vec!["--world", world.0.to_str().unwrap(), "--evm.to", TO],
    ] {
        let mut args = vec!["explain"];
        args.extend(source);
        let output = run(&args);
        success(&output);
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("CALLER") && text.contains("ORIGIN") && text.contains("CALLVALUE"));
    }
}

#[test]
fn explicit_empty_calldata_zero_value_and_independent_origin_narrow_defaults() {
    for command in ["cfg", "ssa"] {
        let analysis = raw(
            command,
            "33321434365f353000",
            &[
                "--evm.to",
                TO,
                "--evm.caller",
                CALLER,
                "--evm.origin",
                ORIGIN,
                "--evm.value",
                "0",
                "--evm.calldata",
                "0x",
            ],
        );
        let stack = analysis["states"][0]["exit_stack"].as_array().unwrap();
        let environment = &analysis["environment"];
        assert_eq!(environment["to"], json!({"Concrete":TO}));
        assert_eq!(environment["caller"], json!({"Concrete":CALLER}));
        assert_eq!(environment["origin"], json!({"Concrete":ORIGIN}));
        constant(&environment["value"], U256::ZERO);
        constant(&environment["calldata"]["length"], U256::ZERO);
        assert_eq!(environment["calldata"]["bytes"], json!({}));
        for value in &stack[..4] {
            constant(value, U256::ZERO);
        }
        constant(&stack[4], word(TO));
        let alias = raw(command, "333233321400", &["--evm.caller", CALLER]);
        constant(&alias["states"][0]["exit_stack"][0], word(CALLER));
        constant(&alias["states"][0]["exit_stack"][1], word(CALLER));
        constant(&alias["states"][0]["exit_stack"][2], U256::from(1));
    }
}

fn supplied() -> Vec<&'static str> {
    vec![
        "--evm.to",
        TO,
        "--evm.caller",
        CALLER,
        "--evm.origin",
        ORIGIN,
        "--evm.value",
        "9",
        "--evm.calldata",
        "0x2a",
        "--evm.gas-price",
        "10",
        "--evm.coinbase",
        COINBASE,
        "--evm.timestamp",
        "11",
        "--evm.number",
        "17",
        "--evm.prevrandao",
        "12",
        "--evm.gas-limit",
        "13",
        "--evm.chain-id",
        "14",
        "--evm.basefee",
        "15",
        "--evm.blob-basefee",
        "16",
        "--evm.gas",
        "1000000",
        "--evm.block-hash",
        "16:0x1111111111111111111111111111111111111111111111111111111111111111",
        "--evm.blob-hash",
        "0:0x1111111111111111111111111111111111111111111111111111111111111111",
        "--evm.blob-count",
        "1",
        "--evm.static",
    ]
}

fn assert_supplied(stack: &[Json], to: &str, caller: &str, value: U256, calldata_len: U256) {
    let expected = [
        word(to),
        word(ORIGIN),
        word(caller),
        value,
        calldata_len,
        U256::from(42) << 248,
        U256::from(10),
        word(COINBASE),
        U256::from(11),
        U256::from(17),
        U256::from(12),
        U256::from(13),
        U256::from(14),
        U256::from(15),
        U256::from(16),
        word(HASH),
        word(HASH),
    ];
    assert_eq!(stack.len(), expected.len() + 1);
    for (actual, expected) in stack.iter().zip(expected) {
        constant(actual, expected);
    }
    // GAS is an observation bounded by the supplied entry budget, not a fabricated exact remainder.
    let gas = stack.last().unwrap();
    if let Some(constants) = gas["Constants"].as_array() {
        for value in constants {
            assert!(value.as_str().unwrap().parse::<U256>().unwrap() <= U256::from(1_000_000));
        }
    } else {
        assert_eq!(gas["interval"]["unsigned_lo"], "0x0");
        let upper = gas["interval"]["unsigned_hi"]
            .as_str()
            .unwrap()
            .parse::<U256>()
            .unwrap();
        assert!(
            upper <= U256::from(1_000_000),
            "GAS must retain its supplied bound: {gas}"
        );
    }
}

#[test]
fn every_supplied_environment_reaches_raw_world_and_delegated_frames() {
    let code = format!("{READ_ENV}00");
    for command in ["cfg", "ssa"] {
        let analysis = raw(command, &code, &supplied());
        assert_supplied_environment(&analysis);
        assert_supplied(
            analysis["states"][0]["exit_stack"].as_array().unwrap(),
            TO,
            CALLER,
            U256::from(9),
            U256::from(1),
        );
    }
    // Copy observed transaction calldata, then delegate with that same input.
    let root = format!("365f5f375f5f365f61020062fffffff450{READ_ENV}00");
    let world = World::new(&root, Some(&code));
    let extra: Vec<_> = supplied().into_iter().skip(2).collect();
    let output = world.run("analyze", &extra);
    success(&output);
    let analysis: Json = serde_json::from_slice(&output.stdout).unwrap();
    let states = analysis["states"].as_array().unwrap();
    for depth in [1, 2] {
        let state = states
            .iter()
            .find(|state| {
                state["key"]["frames"].as_array().unwrap().len() == depth
                    && state["exit_stack"].as_array().unwrap().len() == 18
            })
            .unwrap();
        assert!(
            state["key"]["frames"]
                .as_array()
                .unwrap()
                .iter()
                .all(|frame| frame["is_static"] == true)
        );
        assert_supplied(
            state["exit_stack"].as_array().unwrap(),
            TO,
            CALLER,
            U256::from(9),
            U256::from(1),
        );
    }
    let output = world.run("explain", &extra);
    success(&output);
    let explanation = String::from_utf8(output.stdout).unwrap();
    for opcode in [
        "GASPRICE",
        "COINBASE",
        "TIMESTAMP",
        "NUMBER",
        "GASLIMIT",
        "CHAINID",
        "BASEFEE",
        "BLOBBASEFEE",
        "BLOCKHASH",
        "BLOBHASH",
        "GAS",
    ] {
        assert!(explanation.contains(opcode), "missing {opcode}");
    }
    assert!(explanation.contains("PREVRANDAO") || explanation.contains("DIFFICULTY"));
}

#[test]
fn blob_list_length_and_blockhash_window_preserve_evm_zero_rules() {
    let code = "6010406011406012405f4960014900";
    let block = format!("16:{HASH}");
    let blob = format!("0:{HASH}");
    let analysis = raw(
        "cfg",
        code,
        &[
            "--evm.number",
            "17",
            "--evm.block-hash",
            &block,
            "--evm.blob-hash",
            &blob,
            "--evm.blob-count",
            "1",
        ],
    );
    let stack = analysis["states"][0]["exit_stack"].as_array().unwrap();
    for (value, expected) in
        stack
            .iter()
            .zip([word(HASH), U256::ZERO, U256::ZERO, word(HASH), U256::ZERO])
    {
        constant(value, expected);
    }
    let empty = raw("cfg", "5f495f4900", &["--evm.blob-count", "0"]);
    for value in empty["states"][0]["exit_stack"].as_array().unwrap() {
        constant(value, U256::ZERO);
    }
    let open = raw("cfg", "60014900", &["--evm.blob-hash", &blob]);
    assert!(open["states"][0]["exit_stack"][0]["Constants"].is_null());
}

#[test]
fn malformed_and_conflicting_environment_observations_fail_before_acquisition() {
    for (flag, value) in [
        ("--evm.to", "0x01"),
        ("--evm.caller", "0x01"),
        ("--evm.origin", "0x01"),
        ("--evm.coinbase", "0x01"),
        ("--evm.calldata", "0xzz"),
        ("--evm.block-hash", "-1:0x00"),
        ("--evm.blob-hash", "0:0x00"),
    ] {
        let argument = format!("{flag}={value}");
        let mut args = vec!["analyze", "--rpc", "http://127.0.0.1:1"];
        if flag != "--evm.to" {
            args.extend(["--evm.to", TO]);
        }
        args.push(&argument);
        let output = run(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains(flag));
    }
    for flag in [
        "--evm.value",
        "--evm.gas",
        "--evm.gas-price",
        "--evm.timestamp",
        "--evm.number",
        "--evm.prevrandao",
        "--evm.gas-limit",
        "--evm.chain-id",
        "--evm.basefee",
        "--evm.blob-basefee",
        "--evm.blob-count",
    ] {
        for value in [
            "1.5",
            "115792089237316195423570985008687907853269984665640564039457584007913129639936",
        ] {
            let argument = format!("{flag}={value}");
            let output = run(&[
                "analyze",
                "--rpc",
                "http://127.0.0.1:1",
                "--evm.to",
                TO,
                &argument,
            ]);
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains(flag));
        }
    }
    for flag in ["--evm.block-hash", "--evm.blob-hash"] {
        let first = format!("0:{HASH}");
        let second = format!("0:{OTHER_HASH}");
        let output = run(&[
            "analyze",
            "--rpc",
            "http://127.0.0.1:1",
            "--evm.to",
            TO,
            flag,
            &first,
            flag,
            &second,
        ]);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("conflicting") && !error.contains("RPC transport"),
            "{error}"
        );
    }
    let blob = format!("0:{HASH}");
    let output = run(&[
        "analyze",
        "--rpc",
        "http://127.0.0.1:1",
        "--evm.to",
        TO,
        "--evm.blob-count",
        "0",
        "--evm.blob-hash",
        &blob,
    ]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("outside --evm.blob-count"));
}

#[test]
fn removed_unprefixed_inputs_are_rejected_and_disasm_has_no_execution_environment() {
    for flag in ["--entry", "--caller", "--calldata", "--value", "--static"] {
        let output = run(&["cfg", "--hex", "00", flag, "0"]);
        assert_eq!(output.status.code(), Some(2));
    }
    let output = run(&["disasm", "--hex", "00", "--evm.value", "0"]);
    assert_eq!(output.status.code(), Some(2));
}

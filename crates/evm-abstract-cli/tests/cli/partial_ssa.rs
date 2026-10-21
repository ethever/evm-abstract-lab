//! Partial SSA opt-in retains the incomplete analysis and its observed coverage.

use serde_json::{Value as Json, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
// GAS supplies an unbounded initcode size, so CREATE retains a real typed gap.
const UNCOVERED_CREATE: &str = "5a5f5ff0";
const PARTIAL_CREATE: &str = "6003565b5a5f5ff0";
const PARTIAL: &str = "Partial SSA (machine state IDs):";
const VERIFIED: &str = "Verified cross-contract SSA:";

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_evm-abstract"))
        .args(args)
        .output()
        .unwrap()
}

// Keep the value-domain capacity explicit while the unresolved CREATE
// establishes partial coverage independently of unknown-call execution.
fn run_unknown(args: &[&str]) -> Output {
    let mut args = args.to_vec();
    args.extend(["--max-constants", "8"]);
    run(&args)
}

fn text(output: &Output, exit: i32) -> String {
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn report(output: &Output) -> Json {
    text(output, 2);
    let json: Json = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["analysis"]["status"], "Incomplete");
    assert_eq!(json["partial_ssa"]["status"], "Incomplete");
    assert!(json.get("ssa").is_none());
    assert!(
        !json["partial_ssa"]["frontiers"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    json
}

fn assert_partial(rendered: &str, reason: &str) {
    assert!(rendered.contains(PARTIAL), "{rendered}");
    assert!(rendered.contains("status=Incomplete"));
    assert!(rendered.contains("coverage:") && rendered.contains("Frontiers retained"));
    assert!(rendered.contains(reason));
    assert!(!rendered.contains(VERIFIED));
    assert!(!rendered.contains("SSA unavailable"));
}

fn partial_body(rendered: &str) -> &str {
    rendered.split_once(PARTIAL).unwrap().1
}

fn assert_concise(rendered: &str) {
    let partial = partial_body(rendered);
    for detail in [
        "progress=Completed",
        "progress=Dispatched",
        "effect μ",
        "effect φ",
        "recorded effect",
        "recorded prefix effect",
        "entry effect",
        "coverage=Current",
        "open incoming=[]",
    ] {
        assert!(
            !partial.contains(detail),
            "default detail {detail}:\n{partial}"
        );
    }
    assert!(!partial.contains('μ'));
}

fn assert_verbose(rendered: &str) {
    let partial = partial_body(rendered);
    assert!(partial.contains("progress=Completed"));
    assert!(partial.contains("partial effect φ") && partial.contains("effect μ"));
    assert!(partial.contains('μ'));
}

struct WorldCode(PathBuf);

impl WorldCode {
    fn new(code: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-partial-ssa-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "fork": "osaka", "provenance": "partial-ssa-cli",
                "accounts": [{"address": ENTRY, "code": format!("0x{code}"),
                              "balance": "0x0", "nonce": "0x0", "storage_unknown": false}]
            }))
            .unwrap(),
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            command,
            "--world",
            self.0.to_str().unwrap(),
            "--evm.to",
            ENTRY,
            "--evm.value",
            "0",
            "--evm.calldata",
            "0x",
        ];
        args.extend_from_slice(extra);
        run(&args)
    }

    fn run_unknown(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = extra.to_vec();
        args.extend(["--max-constants", "8"]);
        self.run(command, &args)
    }

    fn add_callee(&self, code: &str) {
        let mut input: Json = serde_json::from_slice(&fs::read(&self.0).unwrap()).unwrap();
        input["accounts"].as_array_mut().unwrap().push(json!({
            "address": "0x0000000000000000000000000000000000000200",
            "code": format!("0x{code}"), "balance": "0x0", "nonce": "0x0",
            "storage_unknown": false,
        }));
        fs::write(&self.0, serde_json::to_vec(&input).unwrap()).unwrap();
    }
}

impl Drop for WorldCode {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn raw_uncovered_creation_partial_ssa_uses_native_ids_and_retains_exit_two() {
    let strict = run_unknown(&["ssa", "--hex", PARTIAL_CREATE]);
    assert_eq!(strict.status.code(), Some(2));
    assert!(strict.stdout.is_empty());
    assert!(String::from_utf8_lossy(&strict.stderr).contains("SSA unavailable"));
    for command in ["ssa", "explain"] {
        let output = run_unknown(&[command, "--hex", PARTIAL_CREATE, "--allow-partial-ssa"]);
        let rendered = text(&output, 2);
        assert_partial(&rendered, "UnknownInitCode");
        assert_concise(&rendered);
        assert!(partial_body(&rendered).contains("Known transitions"));
        assert!(partial_body(&rendered).contains("= CONST 0x0 ; origin=PUSH0"));
        assert!(
            partial_body(&rendered)
                .lines()
                .any(|line| line.contains(": CREATE ")
                    && line.contains("UnknownInitCode")
                    && line.contains('U'))
        );
        assert!(rendered.contains("projected σᵖ₀ → machine σ₀"));
    }
    let json = report(&run_unknown(&[
        "ssa",
        "--hex",
        PARTIAL_CREATE,
        "--allow-partial-ssa",
        "--format",
        "json",
    ]));
    assert!(!json["state_mapping"].as_array().unwrap().is_empty());
    assert_eq!(
        json["state_mapping"][0],
        json!({"local_state": 0, "machine_state": 0})
    );
    // A native child is absent from the local CFG. A real post-call POP/STOP
    // block makes the later continuation IDs observable in the projection.
    let nested = format!("5f5f5f5f5f60046207a120f1506010565b{UNCOVERED_CREATE}5000");
    let json = report(&run_unknown(&[
        "ssa",
        "--hex",
        &nested,
        "--evm.to",
        ENTRY,
        "--evm.value",
        "0",
        "--evm.calldata",
        "0x",
        "--allow-partial-ssa",
        "--format",
        "json",
    ]));
    assert!(
        json["state_mapping"]
            .as_array()
            .unwrap()
            .iter()
            .any(|mapping| mapping["local_state"] != mapping["machine_state"])
    );
    assert!(
        json["analysis"]["states"].as_array().unwrap().len()
            < json["partial_ssa"]["blocks"].as_array().unwrap().len()
    );
    let machine = &json["machine_analysis"];
    assert_eq!(machine["status"], "Incomplete");
    for mapping in json["state_mapping"].as_array().unwrap() {
        let local = mapping["local_state"].as_u64().unwrap() as usize;
        let native = mapping["machine_state"].as_u64().unwrap() as usize;
        assert_eq!(machine["states"][native]["id"], native);
        assert_eq!(
            machine["states"][native]["entry"]["call_stack"]["root"]["state"]["stack"],
            json["analysis"]["states"][local]["entry_stack"],
        );
    }
    for block in json["partial_ssa"]["blocks"].as_array().unwrap() {
        let state = block["state"].as_u64().unwrap() as usize;
        assert_eq!(machine["states"][state]["id"], state);
    }
    for transition in json["partial_ssa"]["transitions"].as_array().unwrap() {
        let edge = transition["edge"].as_u64().unwrap() as usize;
        let source = &machine["edges"][edge];
        assert!(source["from"].is_u64() && source["to"].is_u64());
        assert_eq!(source["kind"], transition["kind"]);
    }
    for deferred in json["partial_ssa"]["deferred_edges"].as_array().unwrap() {
        let edge = deferred["edge"].as_u64().unwrap() as usize;
        let source = &machine["edges"][edge];
        assert_eq!(source["from"], deferred["from"]);
        assert_eq!(source["to"], deferred["to"]);
        assert_eq!(source["kind"], deferred["kind"]);
    }
    let rendered = text(
        &run_unknown(&[
            "ssa",
            "--hex",
            &nested,
            "--evm.to",
            ENTRY,
            "--evm.value",
            "0",
            "--evm.calldata",
            "0x",
            "--allow-partial-ssa",
        ]),
        2,
    );
    assert_concise(&rendered);
    assert_partial(&rendered, "UnknownInitCode");
    assert_eq!(json["partial_ssa"]["frontiers"], machine["frontiers"]);
    // Completing the deeper precompile before its caller leaves current
    // receipts for every observed edge. Unknown execution is a frontier;
    // it does not by itself imply an obsolete edge or open phi input.
    assert!(
        json["partial_ssa"]["deferred_edges"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(
        json["partial_ssa"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|block| block["coverage"] == "Current" && block["open_incoming"] == json!([]))
    );
    assert!(!partial_body(&rendered).contains("Deferred edges"));
    assert!(!partial_body(&rendered).contains("open incoming=["));
}

#[test]
fn budgeted_loop_partial_ssa_keeps_stale_edges_and_open_incoming() {
    // The second transfer grows the loop entry after executing its old body.
    // Stopping there deliberately leaves a stale receipt, independent of how
    // ready calls at different depths are scheduled.
    let args = [
        "ssa",
        "--hex",
        "5f5b600101600156",
        "--context-depth",
        "0",
        "--max-transfers",
        "2",
        "--allow-partial-ssa",
    ];
    let json = report(&run(&[args.as_slice(), &["--format", "json"]].concat()));
    let machine = &json["machine_analysis"];
    let partial = &json["partial_ssa"];
    assert_eq!(partial["frontiers"], machine["frontiers"]);
    assert!(
        partial["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frontier| frontier["reason"] == json!({"Budget": "Transfers"}))
    );
    let deferred = partial["deferred_edges"].as_array().unwrap();
    assert!(!deferred.is_empty());
    for deferred in deferred {
        let edge = deferred["edge"].as_u64().unwrap() as usize;
        let source = &machine["edges"][edge];
        assert_eq!(source["from"], deferred["from"]);
        assert_eq!(source["to"], deferred["to"]);
        assert_eq!(source["kind"], deferred["kind"]);
        assert_eq!(deferred["reason"], "SourceStale");
        let block = partial["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["state"] == deferred["from"])
            .unwrap();
        assert_eq!(block["coverage"], "Stale");
        assert_eq!(block["instructions"], json!([]));
        assert!(
            block["open_incoming"]
                .as_array()
                .unwrap()
                .contains(&deferred["edge"])
        );
        assert!(
            !partial["transitions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|transition| transition["edge"] == deferred["edge"])
        );
    }
    let rendered = text(&run(&args), 2);
    assert_partial(&rendered, "Transfers");
    assert_concise(&rendered);
    assert!(partial_body(&rendered).contains("Deferred edges"));
    assert!(partial_body(&rendered).contains("coverage=Stale"));
    assert!(partial_body(&rendered).contains("open incoming=["));
}

#[test]
fn world_and_both_explain_views_keep_partial_frontiers_and_default_contracts() {
    let world = WorldCode::new(PARTIAL_CREATE);
    let strict = world.run_unknown("analyze", &["--ssa", "--format", "json"]);
    text(&strict, 2);
    let strict_json: Json = serde_json::from_slice(&strict.stdout).unwrap();
    assert_eq!(strict_json["status"], "Incomplete");
    assert!(strict_json.get("partial_ssa").is_none() && strict_json.get("ssa").is_none());
    let json = report(&world.run_unknown(
        "analyze",
        &["--ssa", "--allow-partial-ssa", "--format", "json"],
    ));
    assert_eq!(
        json["analysis"]["frontiers"],
        json["partial_ssa"]["frontiers"]
    );
    for (command, extra) in [
        ("analyze", vec!["--ssa"]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let verbose = extra.contains(&"--verbose");
        let mut args = extra;
        args.push("--allow-partial-ssa");
        let rendered = text(&world.run_unknown(command, &args), 2);
        assert_partial(&rendered, "UnknownInitCode");
        if verbose {
            assert_verbose(&rendered);
            assert!(partial_body(&rendered).contains("progress=Dispatched"));
        } else {
            assert_concise(&rendered);
            assert!(partial_body(&rendered).contains("Known transitions"));
            assert!(partial_body(&rendered).contains("= CONST 0x0 ; origin=PUSH0"));
        }
    }
    for item in json["partial_ssa"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|block| block["instructions"].as_array().unwrap())
    {
        if item["opcode"] == 0xf0 {
            assert!(item["results"].as_array().unwrap().is_empty());
        }
    }
}

#[test]
fn blocked_memory_instruction_is_pending_without_a_normal_result() {
    let world = WorldCode::new("602a60405100");
    let work = super::work_limit_visiting_pc(4, |limit| {
        let work = limit.map(|limit| limit.to_string());
        let mut args = vec![
            "--format",
            "json",
            "--max-memory-bytes",
            "32",
            "--no-root-terminal-coverage",
        ];
        if let Some(work) = &work {
            args.extend(["--max-work", work.as_str()]);
        }
        world.run("analyze", &args)
    })
    .to_string();
    let json = report(&world.run(
        "analyze",
        &[
            "--ssa",
            "--allow-partial-ssa",
            "--format",
            "json",
            "--max-memory-bytes",
            "32",
            "--max-work",
            &work,
            "--no-root-terminal-coverage",
        ],
    ));
    let partial = &json["partial_ssa"];
    assert_eq!(partial["complete"], false);
    assert_eq!(partial["blocks"][0]["coverage"], "Current");
    assert_eq!(partial["frontiers"], json["analysis"]["frontiers"]);
    let frontiers = partial["frontiers"].as_array().unwrap();
    assert_eq!(frontiers.len(), 1);
    assert_eq!(frontiers[0]["reason"], "Work");
    assert_eq!(frontiers[0]["from"], 0);
    assert_eq!(frontiers[0]["pc"], 4);
    assert_eq!(
        json["analysis"]["states"][0]["executed_pcs"],
        json!([0, 2, 4])
    );
    let stack = json["analysis"]["states"][0]["exit_stack"]
        .as_array()
        .unwrap();
    assert_eq!(stack.len(), 1);
    assert_eq!(stack[0]["numeric"]["constants"], json!(["0x2a"]));
    let outcomes = json["analysis"]["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 1);
    assert!(outcomes.iter().all(|outcome| outcome["kind"] == "Failure"));
    let instructions = partial["blocks"][0]["instructions"].as_array().unwrap();
    let load = instructions
        .iter()
        .find(|item| item["opcode"] == 0x51)
        .unwrap();
    assert_eq!(load["pc"], 4);
    assert_eq!(load["progress"], "OperandsConsumed");
    assert_eq!(load["operands"], json!([1]));
    assert!(load["results"].as_array().unwrap().is_empty());
    assert!(load["operation"]["Opcode"].is_object());
    assert_eq!(partial["blocks"][0]["exit_frames"], json!([[0]]));
    assert_eq!(partial["value_count"], 2);
    assert!(!instructions.iter().any(|item| item["opcode"] == 0x00));
    for extra in [vec![], vec!["--verbose"]] {
        let verbose = extra.contains(&"--verbose");
        let mut args = vec![
            "--allow-partial-ssa",
            "--max-memory-bytes",
            "32",
            "--max-work",
            &work,
            "--no-root-terminal-coverage",
        ];
        args.extend(extra);
        let rendered = text(&world.run("explain", &args), 2);
        assert_partial(&rendered, "Work");
        assert!(rendered.contains("progress=OperandsConsumed; pending operation"));
        if verbose {
            assert_verbose(&rendered);
        } else {
            assert_concise(&rendered);
        }
    }
}

#[test]
fn work_prefix_unprocessed_nodes_and_empty_analysis_have_distinct_coverage() {
    let world = WorldCode::new("600160020160030100");
    let work = super::work_limit_visiting_pc(4, |limit| {
        let work = limit.map(|limit| limit.to_string());
        let mut args = vec!["--format", "json", "--no-root-terminal-coverage"];
        if let Some(work) = &work {
            args.extend(["--max-work", work.as_str()]);
        }
        world.run("analyze", &args)
    })
    .to_string();
    let json = report(&world.run(
        "analyze",
        &[
            "--ssa",
            "--allow-partial-ssa",
            "--format",
            "json",
            "--max-work",
            &work,
            "--no-root-terminal-coverage",
        ],
    ));
    let instructions = json["partial_ssa"]["blocks"][0]["instructions"]
        .as_array()
        .unwrap();
    assert!(!instructions.is_empty() && instructions.len() < 6);
    assert!(
        instructions
            .iter()
            .any(|item| item["progress"] == "Completed")
    );
    assert_eq!(json["partial_ssa"]["blocks"][0]["coverage"], "Current");
    assert_eq!(
        json["analysis"]["states"][0]["executed_pcs"],
        json!([0, 2, 4])
    );
    assert!(
        json["analysis"]["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frontier| frontier["reason"] == "Work")
    );
    let zero = world.run(
        "analyze",
        &[
            "--ssa",
            "--allow-partial-ssa",
            "--format",
            "json",
            "--max-work",
            "0",
            "--no-root-terminal-coverage",
        ],
    );
    assert_eq!(zero.status.code(), Some(1));
    assert!(zero.stdout.is_empty());
    assert!(String::from_utf8_lossy(&zero.stderr).contains("positive"));
    let empty = report(&world.run(
        "analyze",
        &[
            "--ssa",
            "--allow-partial-ssa",
            "--format",
            "json",
            "--max-work",
            "1",
            "--no-root-terminal-coverage",
        ],
    ));
    assert_eq!(empty["analysis"]["config"]["max_work"], 1);
    let frontiers = empty["analysis"]["frontiers"].as_array().unwrap();
    assert_eq!(frontiers.len(), 1);
    assert_eq!(frontiers[0]["reason"], "Work");
    assert!(frontiers[0]["from"].is_null() && frontiers[0]["pc"].is_null());
    assert!(empty["analysis"]["states"].as_array().unwrap().is_empty());
    assert!(
        empty["partial_ssa"]["blocks"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let jump = WorldCode::new("6003565b00");
    let json = report(&jump.run(
        "analyze",
        &[
            "--ssa",
            "--allow-partial-ssa",
            "--format",
            "json",
            "--max-transfers",
            "1",
        ],
    ));
    let unprocessed = json["partial_ssa"]["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["coverage"] == "Unexecuted")
        .unwrap();
    assert!(unprocessed["instructions"].as_array().unwrap().is_empty());
    assert!(
        json["analysis"]["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frontier| frontier["reason"] == json!({"Budget":"Transfers"}))
    );
    let state = unprocessed["state"].as_u64().unwrap() as usize;
    assert_eq!(json["analysis"]["states"][state]["executed_pcs"], json!([]));
    assert!(json["analysis"]["states"][state]["exit"].is_null());
    let rendered = text(
        &jump.run("explain", &["--allow-partial-ssa", "--max-transfers", "1"]),
        2,
    );
    let partial = rendered.split_once(PARTIAL).unwrap().1;
    assert!(partial.contains("coverage=Unexecuted"));
    assert!(!partial.contains("synthetic end-of-code"));
    assert_concise(&rendered);
    let faulted = WorldCode::new(&format!("33600957{UNCOVERED_CREATE}005bfe"));
    let rendered = text(&faulted.run_unknown("explain", &["--allow-partial-ssa"]), 2);
    assert_concise(&rendered);
    assert!(partial_body(&rendered).contains("progress=Faulted"));
}

#[test]
fn verbose_return_dispatch_distinguishes_root_outcomes_from_child_continuations() {
    let world = WorldCode::new(&format!(
        "5f5f5f5f5f6102006207a120f15033601757{UNCOVERED_CREATE}005b5f5ff3"
    ));
    let mut input: Json = serde_json::from_slice(&fs::read(&world.0).unwrap()).unwrap();
    input["accounts"].as_array_mut().unwrap().push(json!({
        "address": "0x0000000000000000000000000000000000000200",
        "code": "0x602a5f5260205ff3", "balance": "0x0", "nonce": "0x0",
        "storage_unknown": false,
    }));
    fs::write(&world.0, serde_json::to_vec(&input).unwrap()).unwrap();
    let compact = text(&world.run_unknown("explain", &["--allow-partial-ssa"]), 2);
    assert_partial(&compact, "UnknownInitCode");
    assert_concise(&compact);
    let verbose = text(
        &world.run_unknown("explain", &["--allow-partial-ssa", "--verbose"]),
        2,
    );
    assert_verbose(&verbose);
    let returns: Vec<_> = partial_body(&verbose)
        .lines()
        .filter(|line| line.contains(": RETURN ") && line.contains("progress=Dispatched"))
        .collect();
    assert!(
        returns
            .iter()
            .any(|line| line.contains("return dispatch; inspect recorded outcomes"))
    );
    assert!(
        returns
            .iter()
            .any(|line| line.contains("return dispatch; inspect recorded caller continuations"))
    );
    assert!(!partial_body(&verbose).contains("results belong only to recorded transitions"));
}

#[test]
fn concise_and_verbose_retain_proven_exceptional_dispatch_halts() {
    let static_child = WorldCode::new(&format!("5f5f5f5f6102006207a120fa50{UNCOVERED_CREATE}00"));
    static_child.add_callee("60015f5500");
    let return_oob = WorldCode::new(&format!("5a600957{UNCOVERED_CREATE}005b60015f5f3e00"));
    for (world, diagnostic, opcode) in [
        (
            &static_child,
            "exceptional halt: state change in static frame",
            ": SSTORE ",
        ),
        (
            &return_oob,
            "exceptional halt: return data out of bounds",
            ": RETURNDATACOPY ",
        ),
    ] {
        for verbose in [false, true] {
            let mut args = vec!["--allow-partial-ssa"];
            if verbose {
                args.push("--verbose");
            }
            let rendered = text(&world.run_unknown("explain", &args), 2);
            assert_partial(&rendered, "UnknownInitCode");
            let line = partial_body(&rendered)
                .lines()
                .find(|line| line.contains(opcode))
                .unwrap();
            assert!(line.contains(diagnostic), "{line}");
            if verbose {
                assert!(line.contains("progress=Dispatched"));
                assert!(!line.contains("inspect recorded caller continuations"));
            } else {
                assert_concise(&rendered);
            }
        }
    }
}

#[test]
fn complete_outputs_are_identical_with_and_without_partial_opt_in() {
    for args in [
        vec!["ssa", "--hex", "600160020100"],
        vec!["ssa", "--hex", "600160020100", "--format", "json"],
        vec!["explain", "--hex", "600160020100"],
    ] {
        let expected = run(&args);
        text(&expected, 0);
        let json = args.contains(&"json");
        let mut partial = args;
        partial.push("--allow-partial-ssa");
        let actual = run(&partial);
        text(&actual, 0);
        if json {
            let actual: Json = serde_json::from_slice(&actual.stdout).unwrap();
            let expected: Json = serde_json::from_slice(&expected.stdout).unwrap();
            assert_eq!(
                super::native::canonical(&actual),
                super::native::canonical(&expected)
            );
        } else {
            assert_eq!(actual.stdout, expected.stdout);
        }
        assert_eq!(actual.stderr, expected.stderr);
    }
    let world = WorldCode::new("602a5f5260205ff3");
    for (command, args) in [
        ("analyze", vec!["--ssa"]),
        ("analyze", vec!["--ssa", "--format", "json"]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let expected = world.run(command, &args);
        text(&expected, 0);
        let json = args.contains(&"json");
        let mut partial = args;
        partial.push("--allow-partial-ssa");
        let actual = world.run(command, &partial);
        text(&actual, 0);
        if json {
            let actual: Json = serde_json::from_slice(&actual.stdout).unwrap();
            let expected: Json = serde_json::from_slice(&expected.stdout).unwrap();
            assert_eq!(
                super::native::canonical(&actual),
                super::native::canonical(&expected)
            );
        } else {
            assert_eq!(actual.stdout, expected.stdout);
        }
        assert_eq!(actual.stderr, expected.stderr);
    }
}

#[test]
fn partial_ssa_requires_analyze_ssa_and_rejects_dot_before_analysis() {
    let world = WorldCode::new("00");
    for args in [
        vec!["--allow-partial-ssa"],
        vec!["--ssa", "--allow-partial-ssa", "--format", "dot"],
    ] {
        let output = world.run("analyze", &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-partial-ssa"));
    }
}

#[test]
fn fixed_rpc_uncovered_creation_supports_partial_analysis_and_both_explain_views() {
    use super::explain::{RpcFixture, RpcServer};
    const BLOCK: &str = "0x1111111111111111111111111111111111111111111111111111111111111111";
    std::thread::scope(|scope| {
        for (command, extra) in [
            ("analyze", vec!["--ssa", "--format", "json"]),
            ("explain", vec![]),
            ("explain", vec!["--verbose"]),
        ] {
            let verbose = extra.contains(&"--verbose");
            let server = RpcServer::with_fixture(scope, RpcFixture::UnknownInitCode);
            let mut args = vec![
                command,
                "--rpc",
                &server.endpoint,
                "--block-hash",
                BLOCK,
                "--evm.to",
                ENTRY,
                "--evm.value",
                "0",
                "--evm.calldata",
                "0x",
                "--allow-partial-ssa",
            ];
            args.extend(extra);
            let output = run_unknown(&args);
            if command == "analyze" {
                let json = report(&output);
                assert_eq!(
                    json["analysis"]["frontiers"],
                    json["partial_ssa"]["frontiers"]
                );
                assert!(
                    json["partial_ssa"]["frontiers"]
                        .to_string()
                        .contains("UnknownInitCode")
                );
            } else {
                let rendered = text(&output, 2);
                assert_partial(&rendered, "UnknownInitCode");
                if verbose {
                    assert_verbose(&rendered);
                    assert!(partial_body(&rendered).contains("progress=Dispatched"));
                } else {
                    assert_concise(&rendered);
                    assert!(partial_body(&rendered).contains("Known transitions"));
                }
            }
            let requests = server.finish();
            assert_eq!(
                requests
                    .iter()
                    .filter(|request| request["method"] == "eth_getCode")
                    .count(),
                1
            );
            for request in requests {
                match request["method"].as_str().unwrap() {
                    "eth_getBlockByHash" => assert_eq!(request["params"][0], BLOCK),
                    "eth_getCode" | "eth_getBalance" | "eth_getTransactionCount" => {
                        assert_eq!(
                            request["params"][1],
                            json!({"blockHash": BLOCK, "requireCanonical": true})
                        );
                    }
                    "eth_chainId" => {}
                    method => panic!("unexpected RPC method {method}"),
                }
            }
        }
    });
}

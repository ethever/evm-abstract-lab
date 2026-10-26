//! 从安装后同名的真实二进制验证输入、状态、JSON 和 exit code 契约。

use evm_abstract_notation::normalize_subscripts;
use std::process::Command;

#[path = "support/native.rs"]
mod native;

#[path = "cli/environment.rs"]
mod environment;

#[path = "cli/alignment.rs"]
mod alignment;

#[path = "cli/shared_listing.rs"]
mod shared_listing;

#[path = "cli/explain.rs"]
mod explain;

#[path = "cli/teaching.rs"]
mod teaching;

#[path = "cli/capacity.rs"]
mod capacity;
#[path = "cli/defaults.rs"]
mod defaults;
#[path = "cli/diagnostics.rs"]
mod diagnostics;
#[path = "cli/domains.rs"]
mod domains;
#[path = "cli/known_bits.rs"]
mod known_bits;
#[path = "cli/normalized.rs"]
mod normalized;
#[path = "cli/numbers.rs"]
mod numbers;
#[path = "cli/partial_ssa.rs"]
mod partial_ssa;
#[path = "cli/precision_policy.rs"]
mod precision_policy;
#[path = "cli/relations.rs"]
mod relations;
#[path = "cli/root_terminal_coverage.rs"]
mod root_terminal_coverage;
#[path = "cli/rpc.rs"]
mod rpc;

fn run_concrete(args: &[&str]) -> std::process::Output {
    let mut scoped = args.to_vec();
    if args.iter().any(|arg| matches!(*arg, "--world" | "--rpc")) {
        for (flag, value) in [
            ("--evm.value", "0"),
            ("--evm.calldata", "0x"),
            ("--evm.caller", "0x0000000000000000000000000000000000001000"),
        ] {
            if !args.iter().any(|arg| arg.split('=').next() == Some(flag)) {
                scoped.extend([flag, value]);
            }
        }
    }
    Command::new(crate::native::binary())
        .args(scoped)
        .output()
        .unwrap()
}

// These fixtures have one fixed, acyclic prefix. Visiting a given PC is
// monotone in Work, so the full native run bounds at most usize::BITS bisections.
// The first MLOAD visit consumes its offset before its positive read-scan fee;
// callers still assert the actual phase/result rather than infer it here.
fn work_limit_visiting_pc(
    pc: usize,
    mut run: impl FnMut(Option<usize>) -> std::process::Output,
) -> usize {
    let native = |output: std::process::Output| {
        let exit = output.status.code();
        assert!(
            matches!(exit, Some(0 | 2)),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let raw: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        if raw.get("global").is_some() {
            native::assert_exit(&raw, &output);
        }
        if raw.get("global").is_some() && raw["native"].is_null() {
            let coverage = &raw["hybrid"]["coverage"];
            assert_eq!(coverage["detail_current"], false);
            assert_eq!(raw["hybrid"]["local_detail"], serde_json::json!([]));
            if coverage["baseline_current"] == true {
                assert_eq!(native::global(&raw)["status"], "Converged");
                assert_eq!(coverage["detail"], "Work");
            } else {
                assert_eq!(native::global(&raw)["status"], "Incomplete");
                assert_eq!(coverage["detail"], "BaselinePending");
            }
            return None;
        }
        let mut report = crate::native::select(&raw);
        if report.get("machine_analysis").is_some() {
            report = report["machine_analysis"].take();
        }
        if raw.get("global").is_none() {
            assert_eq!(report["status"] == "Converged", exit == Some(0));
        }
        Some(native::select(&report))
    };
    let visited = |report: &serde_json::Value| {
        report["states"].as_array().unwrap().iter().any(|state| {
            state["executed_pcs"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!(pc))
        })
    };
    let complete = native(run(None)).expect("full run retains Native execution");
    assert_eq!(complete["status"], "Converged");
    assert!(visited(&complete));
    let mut upper = usize::try_from(complete["work"].as_u64().unwrap()).unwrap();
    assert!(upper > 0);
    let mut lower = 1;
    for _ in 0..usize::BITS {
        if lower == upper {
            break;
        }
        let midpoint = lower + (upper - lower) / 2;
        if native(run(Some(midpoint))).as_ref().is_some_and(visited) {
            upper = midpoint;
        } else {
            lower = midpoint + 1;
        }
    }
    assert_eq!(lower, upper, "Work calibration must be bounded");
    let boundary = native(run(Some(lower))).expect("boundary has a Native PC receipt");
    assert_eq!(boundary["status"], "Incomplete");
    assert!(visited(&boundary));
    assert!(
        boundary["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frontier| frontier["reason"] == "Work")
    );
    lower
}

#[test]
fn help_and_version_are_available() {
    assert!(run_concrete(&["--help"]).status.success());
    assert!(
        String::from_utf8(run_concrete(&["--version"]).stdout)
            .unwrap()
            .contains(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn cfg_json_includes_status_contexts_and_unknown_jump_diagnostic() {
    let output = run_concrete(&[
        "cfg",
        "--hex",
        "600035565b00",
        "--format",
        "json",
        "--context-depth",
        "1",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["status"], "Converged");
    assert_eq!(json["config"]["context_depth"], 1);
    let states = json["states"].as_array().unwrap();
    for state in states {
        assert!(state["key"]["basic_block_index"].is_u64());
        assert!(state["key"].get("block").is_none());
    }
    assert!(
        states
            .iter()
            .any(|state| state["key"]["basic_block_index"] == 1)
    );
    assert_eq!(json["program"]["blocks"][1]["start_pc"], 4);
    assert!(
        json["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["kind"] == "UnknownJump")
    );
}

#[test]
fn cfg_file_preserves_default_128_and_explicit_context_depths() {
    use std::collections::BTreeSet;

    // Independent of transfer's remove/append/truncate implementation.
    fn latest_distinct(sources: &[usize], depth: usize) -> Vec<usize> {
        let mut seen = BTreeSet::new();
        let mut newest: Vec<_> = sources
            .iter()
            .rev()
            .copied()
            .filter(|source| seen.insert(*source))
            .take(depth)
            .collect();
        newest.reverse();
        newest
    }

    let path = format!(
        "{}/../../examples/internal-calls.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = run_concrete(&["cfg", "--file", &path, "--context-depth", "10"]);
    assert!(
        text.status.success(),
        "{}",
        String::from_utf8_lossy(&text.stderr)
    );
    assert!(String::from_utf8_lossy(&text.stdout).contains("context_depth=10"));
    for explicit in [
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(8),
        Some(10),
        Some(128),
        Some(usize::MAX),
    ] {
        let mut args = vec!["cfg", "--file", &path, "--format", "json"];
        let depth = explicit.map(|value| value.to_string());
        if let Some(depth) = &depth {
            args.extend(["--context-depth", depth.as_str()]);
        }
        let output = run_concrete(&args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let json = crate::native::select(&json);
        assert_eq!(json["status"], "Converged");
        assert_eq!(json["config"]["context_depth"], explicit.unwrap_or(128));
        let states = json["states"].as_array().unwrap();
        let depth = explicit.unwrap_or(128);
        assert!(
            states
                .iter()
                .all(|state| state["key"]["context"].as_array().unwrap().len() <= depth)
        );
        // The second visit to source 14 moves it to the newest position.
        let complete_history = [0, 14, 5, 14];
        let expected_history = serde_json::json!(latest_distinct(&complete_history, depth));
        assert!(
            states
                .iter()
                .any(|state| state["key"]["context"] == expected_history)
        );
        let helper_block = json["program"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .position(|block| block["start_pc"] == 14)
            .unwrap();
        let helpers: Vec<_> = states
            .iter()
            .filter(|state| state["key"]["basic_block_index"] == helper_block)
            .collect();
        assert_eq!(helpers.len(), if depth == 0 { 1 } else { 2 });
        for helper in helpers {
            assert_eq!(
                helper["entry_stack"][0]["numeric"]["constants"]
                    .as_array()
                    .unwrap()
                    .len(),
                if depth == 0 { 2 } else { 1 }
            );
        }
    }

    // 130 distinct sources exercise the default 128-entry bound and explicit
    // depths without allocating from the requested depth (including usize::MAX).
    for backedge in [None, Some(126usize)] {
        let count = 130usize;
        let stop_pc = count.checked_mul(5).unwrap();
        let mut bytes = Vec::with_capacity(stop_pc.checked_add(2).unwrap());
        for index in 0..count {
            let next = index.checked_add(1).unwrap();
            let target = if next == count {
                backedge.unwrap_or(count)
            } else {
                next
            };
            let target = u16::try_from(target.checked_mul(5).unwrap())
                .unwrap()
                .to_be_bytes();
            bytes.extend([0x5b, 0x61, target[0], target[1], 0x56]);
        }
        bytes.extend([0x5b, 0x00]);
        let program = evm_abstract::bytecode::Program::decode(&bytes).unwrap();
        assert_eq!(program.blocks().len(), count.checked_add(1).unwrap());
        let source_pcs: Vec<_> = program.blocks()[..count]
            .iter()
            .enumerate()
            .map(|(index, block)| {
                assert_eq!(block.start_pc, index.checked_mul(5).unwrap());
                let next = index.checked_add(1).unwrap();
                let target = if next == count {
                    backedge.unwrap_or(count)
                } else {
                    next
                };
                assert_eq!(block.instructions[0].opcode, 0x5b);
                assert_eq!(block.instructions[1].opcode, 0x61);
                assert_eq!(block.instructions[2].opcode, 0x56);
                assert_eq!(
                    block.instructions[1].immediate,
                    Some(alloy_primitives::U256::from(
                        program.blocks()[target].start_pc
                    ))
                );
                block.start_pc
            })
            .collect();
        assert_eq!(program.blocks()[count].start_pc, stop_pc);
        assert_eq!(
            source_pcs.iter().copied().collect::<BTreeSet<_>>().len(),
            count
        );
        let mut trace = source_pcs.clone();
        if let Some(target) = backedge {
            for _ in 0..2 {
                trace.extend_from_slice(&source_pcs[target..]);
            }
            trace.push(source_pcs[target]);
        } else {
            trace.push(stop_pc);
        }
        let path = std::env::temp_dir().join(format!(
            "evm-cli-context-{}-{backedge:?}.hex",
            std::process::id()
        ));
        std::fs::write(&path, alloy_primitives::hex::encode(&bytes)).unwrap();
        for explicit in [
            None,
            Some(0),
            Some(1),
            Some(2),
            Some(8),
            Some(10),
            Some(128),
            Some(usize::MAX),
        ] {
            let depth = explicit.unwrap_or(128);
            let depth_arg = explicit.map(|depth| depth.to_string());
            let mut args = vec!["cfg", "--file", path.to_str().unwrap(), "--format", "json"];
            if let Some(depth_arg) = &depth_arg {
                args.extend(["--context-depth", depth_arg.as_str()]);
            }
            let output = run_concrete(&args);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let cfg: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let cfg = crate::native::select(&cfg);
            assert_eq!(cfg["config"]["context_depth"], depth);
            assert_eq!(cfg["status"], "Converged");
            assert_eq!(cfg["frontiers"], serde_json::json!([]));
            let states = cfg["states"].as_array().unwrap();
            for state in states {
                let history: Vec<_> = state["key"]["context"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|pc| usize::try_from(pc.as_u64().unwrap()).unwrap())
                    .collect();
                assert_eq!(history, latest_distinct(&history, depth));
                assert!(history.iter().all(|pc| source_pcs.contains(pc)));
            }
            if backedge.is_none() {
                assert_eq!(states.len(), count.checked_add(1).unwrap());
                assert_eq!(cfg["edges"].as_array().unwrap().len(), count);
            }
            let mut sources = Vec::new();
            let mut previous = None;
            let mut trace_states = BTreeSet::new();
            for pc in &trace {
                let block = program
                    .blocks()
                    .iter()
                    .position(|block| block.start_pc == *pc)
                    .unwrap();
                let context = serde_json::json!(latest_distinct(&sources, depth));
                let state = states
                    .iter()
                    .find(|state| {
                        state["key"]["basic_block_index"] == block
                            && state["key"]["context"] == context
                    })
                    .unwrap();
                assert_eq!(state["entry_stack"], serde_json::json!([]));
                assert_eq!(state["exit_stack"], serde_json::json!([]));
                assert_eq!(
                    state["executed_pcs"],
                    serde_json::json!(
                        program.blocks()[block]
                            .instructions
                            .iter()
                            .map(|item| item.pc)
                            .collect::<Vec<_>>()
                    )
                );
                if let Some(previous) = previous {
                    assert!(cfg["edges"].as_array().unwrap().iter().any(|edge| {
                        edge["from"] == previous
                            && edge["to"] == state["id"]
                            && edge["kind"]["Intraprocedural"] == "Jump"
                    }));
                }
                previous = state["id"].as_u64();
                trace_states.insert(previous.unwrap());
                sources.push(*pc);
            }
            assert_eq!(trace_states.len(), states.len());
            // The real CLI invokes both the closed and normalized SSA verifiers.
            args[0] = "ssa";
            let output = run_concrete(&args);
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let ssa: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let ssa = crate::native::select(&ssa);
            assert_eq!(ssa["ssa"]["complete"], true);
            for mapping in ssa["state_mapping"].as_array().unwrap() {
                let local = mapping["local_state"].as_u64().unwrap() as usize;
                let machine = mapping["machine_state"].as_u64().unwrap() as usize;
                let state = &ssa["analysis"]["states"][local];
                let native = &ssa["machine_analysis"]["states"][machine];
                let frames = native["key"]["frames"].as_array().unwrap();
                assert_eq!(frames.len(), 1);
                assert_eq!(
                    frames[0]["basic_block_index"],
                    state["key"]["basic_block_index"]
                );
                assert_eq!(frames[0]["jump_history"], state["key"]["context"]);
                assert_eq!(frames[0]["stack_height"], 0);
                let block = state["key"]["basic_block_index"].as_u64().unwrap() as usize;
                let body = ssa["ssa"]["blocks"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|body| body["state"] == machine)
                    .unwrap();
                let rows = body["instructions"].as_array().unwrap();
                assert_eq!(rows.len(), program.blocks()[block].instructions.len());
                for (row, source) in rows.iter().zip(&program.blocks()[block].instructions) {
                    assert_eq!(row["pc"], source.pc);
                    assert_eq!(row["opcode"], source.opcode);
                }
            }
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn ssa_json_carries_cfg_and_value_definitions() {
    let output = run_concrete(&["ssa", "--hex", "600160020100", "--format", "json"]);
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["ssa"]["value_count"], 3);
    assert_eq!(json["analysis"]["status"], "Converged");
    assert_eq!(json["analysis"]["config"]["context_depth"], 128);
}

#[test]
fn budgets_exit_two_and_never_return_partial_ssa() {
    let cfg = run_concrete(&[
        "cfg",
        "--hex",
        "6003565b00",
        "--max-states",
        "1",
        "--format",
        "json",
    ]);
    assert_eq!(cfg.status.code(), Some(2));
    let json: serde_json::Value = serde_json::from_slice(&cfg.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["status"], "Incomplete");
    assert!(!json["frontiers"].as_array().unwrap().is_empty());
    let ssa = run_concrete(&["ssa", "--hex", "6003565b00", "--max-states", "1"]);
    assert_eq!(ssa.status.code(), Some(2));
    assert!(ssa.stdout.is_empty());
}

#[test]
fn invalid_hex_has_a_useful_error() {
    let output = run_concrete(&["disasm", "--hex", "zz"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("invalid hex")
    );
}

#[test]
fn input_choices_are_exclusive_and_required() {
    assert_eq!(run_concrete(&["cfg"]).status.code(), Some(2));
    assert_eq!(
        run_concrete(&["cfg", "--fork", "osaka"]).status.code(),
        Some(2)
    );
    assert_eq!(
        run_concrete(&["cfg", "--hex", "00", "--file", "example.hex"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn fork_changes_clz_cfg_and_all_exports_report_the_selected_rules() {
    let code = "60011e60f79003565b602a00";
    let default = run_concrete(&["cfg", "--hex", code, "--format", "json"]);
    assert!(default.status.success());
    let json: serde_json::Value = serde_json::from_slice(&default.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["program"]["fork"], "osaka");
    assert_eq!(json["edges"].as_array().unwrap().len(), 1);
    for fork in ["cancun", "prague", "osaka"] {
        let output = run_concrete(&["cfg", "--hex", code, "--format", "json", "--fork", fork]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let json = crate::native::select(&json);
        assert_eq!(json["program"]["fork"], fork);
        assert_eq!(
            json["edges"].as_array().unwrap().len(),
            usize::from(fork == "osaka")
        );
        let disasm = run_concrete(&["disasm", "--hex", code, "--fork", fork]);
        assert!(
            String::from_utf8(disasm.stdout)
                .unwrap()
                .contains(&format!("fork={fork}"))
        );
        let dot = run_concrete(&["cfg", "--hex", code, "--fork", fork, "--format", "dot"]);
        assert!(
            String::from_utf8(dot.stdout)
                .unwrap()
                .contains(&format!("fork={fork}"))
        );
        let ssa = run_concrete(&["ssa", "--hex", code, "--fork", fork, "--format", "json"]);
        assert!(ssa.status.success());
        let json: serde_json::Value = serde_json::from_slice(&ssa.stdout).unwrap();
        let json = crate::native::select(&json);
        assert_eq!(json["analysis"]["program"]["fork"], fork);
    }
    assert_eq!(
        run_concrete(&["cfg", "--hex", code, "--fork", "amsterdam"])
            .status
            .code(),
        Some(2)
    );
}

#[test]
fn delegation_code_reports_its_target_instead_of_a_false_completed_cfg() {
    let code = "ef01001111111111111111111111111111111111111111";
    let output = run_concrete(&["cfg", "--hex", code, "--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains("EIP-7702") && error.contains("0x1111111111111111111111111111111111111111")
    );
}

fn analyze(name: &str, extra: &[&str]) -> std::process::Output {
    let path = format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let mut args = vec!["analyze", "--world", &path];
    if !extra.contains(&"--evm.to") {
        args.extend(["--evm.to", "0x0000000000000000000000000000000000000101"]);
    }
    args.extend(extra);
    run_concrete(&args)
}

fn assert_shared_budget(report: &serde_json::Value, regions: usize, maximum: u64) {
    let global = native::global(report);
    let coverage = &report["hybrid"]["coverage"];
    let work = &coverage["work"];
    assert_eq!(coverage["requested_detail_regions"], regions);
    assert_eq!(coverage["requested_detail_work"], usize::MAX);
    assert_eq!(global["config"]["max_work"], maximum);
    assert_eq!(work["prior"], 0);
    let baseline = work["baseline"].as_u64().unwrap();
    let preparation = work["preparation"].as_u64().unwrap();
    let detail = work["detail"].as_u64().unwrap();
    let total = work["total"].as_u64().unwrap();
    assert_eq!(total, baseline + preparation + detail);
    assert_eq!(report["work"], total);
    assert_eq!(global["work"], baseline);
    assert!(total <= maximum);
    if coverage["baseline_current"] == true {
        assert_eq!(work["detail_allowance"], maximum - baseline);
    }
    let transfers = report["transfers"].as_u64().unwrap();
    assert!(global["transfers"].as_u64().unwrap() <= transfers);
    assert!(
        transfers
            <= global["config"]["analysis"]["max_transfers"]
                .as_u64()
                .unwrap()
    );
    assert_eq!(global["config"]["root_terminal_coverage"], false);
    if let Some(graph) = report.get("native").filter(|graph| !graph.is_null()) {
        for key in [
            "analysis",
            "max_work",
            "max_call_depth",
            "max_memory_bytes",
            "use_summaries",
        ] {
            assert_eq!(graph["analysis"]["config"][key], global["config"][key]);
        }
        assert_eq!(
            graph["analysis"]["config"]["root_terminal_coverage"],
            coverage["requested_native_terminal_coverage"]
        );
        assert_eq!(graph["analysis"]["work"], total);
        assert_eq!(graph["analysis"]["transfers"], transfers);
    }
}

// Relative block receipts establish Native control and joint output facts,
// without claiming that a private World frame recorded instruction PCs or SSA.
fn relative_native_call<'a>(
    analysis: &'a serde_json::Value,
    source: &str,
    caller: &str,
    is_static: bool,
) -> &'a serde_json::Value {
    let record = analysis["procedure_summaries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| {
            let root = record["root_procedure"].as_u64().unwrap() as usize;
            record["procedures"][root]["frame"]["code_address"] == source
        })
        .expect("callee has a retained relative procedure");
    assert_eq!(record["kind"], "KnownCall");
    assert_eq!(record["source_current"], true);
    assert_eq!(record["closed"], true);
    let root = record["root_procedure"].as_u64().unwrap() as usize;
    let procedure = &record["procedures"][root];
    let frame = &procedure["frame"];
    assert_eq!(procedure["semantics"], "Native");
    assert!(procedure["process_input"].is_null());
    assert_eq!(frame["mode"], "Runtime");
    assert_eq!(frame["address"], source);
    assert_eq!(frame["address_value"]["Concrete"], source);
    assert_eq!(frame["caller"]["Concrete"], caller);
    assert_eq!(frame["is_static"], is_static);
    let hash = &analysis["world"]["code_hashes"][source];
    assert!(hash.is_string());
    assert_eq!(frame["code_hash"], *hash);
    let source_state = analysis["states"]
        .as_array()
        .unwrap()
        .iter()
        .find(|state| state["id"] == record["source_state"])
        .unwrap();
    assert_eq!(
        source_state["key"]["frames"]
            .as_array()
            .unwrap()
            .last()
            .unwrap(),
        frame
    );
    assert_eq!(source_state["executed_pcs"], serde_json::json!([]));
    let nodes: Vec<_> = record["regional_cfg"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|node| node["procedure"] == root)
        .collect();
    assert!(!nodes.is_empty());
    for node in &nodes {
        for key in [
            "code_address",
            "code_hash",
            "address",
            "address_value",
            "caller",
            "is_static",
        ] {
            assert_eq!(node["frame"][key], frame[key]);
        }
        assert_eq!(node["excluded"], false);
        let transfer = &node["transfer"];
        assert_eq!(transfer["semantics"], "Native");
        assert!(transfer["process"].is_null());
        assert_eq!(transfer["current"], true);
        assert_eq!(transfer["complete"], true);
        assert_eq!(transfer["visited_block"], true);
        assert_eq!(transfer["input_revision"], node["input_revision"]);
        assert_eq!(
            node["input_revision"],
            procedure["version"]["entry_revision"]
        );
        assert!(
            procedure["covered_blocks"]
                .as_array()
                .unwrap()
                .contains(&node["frame"]["basic_block_index"])
        );
    }
    for output in procedure["outputs"].as_array().unwrap() {
        assert!(nodes.iter().any(|node| {
            node["transfer"]["outcomes"]
                .as_array()
                .unwrap()
                .contains(&output["kind"])
        }));
    }
    procedure
}

#[test]
fn world_analyze_preserves_default_128_and_explicit_context_depths() {
    for explicit in [
        None,
        Some(0),
        Some(1),
        Some(2),
        Some(8),
        Some(10),
        Some(128),
        Some(usize::MAX),
    ] {
        let mut extra = vec!["--format", "json"];
        let depth = explicit.map(|value| value.to_string());
        if let Some(depth) = &depth {
            extra.extend(["--context-depth", depth.as_str()]);
        }
        let output = analyze("call-return-branch", &extra);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        let json = crate::native::select(&json);
        assert_eq!(json["status"], "Converged");
        assert_eq!(
            json["config"]["analysis"]["context_depth"],
            explicit.unwrap_or(128)
        );
        let depth = explicit.unwrap_or(128);
        let histories: Vec<_> = json["states"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|state| state["key"]["frames"].as_array().unwrap())
            .map(|frame| frame["jump_history"].as_array().unwrap())
            .collect();
        assert!(histories.iter().all(|history| history.len() <= depth));
        if depth > 0 {
            assert!(histories.iter().any(|history| !history.is_empty()));
        }
    }
}

fn assert_call_return_world(json: &serde_json::Value) {
    assert_eq!(json["status"], "Converged");
    assert_eq!(json["world"]["fork"], "osaka");
    assert_eq!(json["world"]["provenance"], "offline:call-return-branch:v1");
    let edges = json["edges"].as_array().unwrap();
    assert!(edges.iter().any(|edge| edge["kind"] == "Call"));
    assert!(
        json["states"]
            .as_array()
            .unwrap()
            .iter()
            .any(|state| state["key"]["frames"].as_array().unwrap().len() == 2)
    );
    for state in json["states"].as_array().unwrap() {
        for key in state["key"]["frames"].as_array().unwrap() {
            assert!(key["basic_block_index"].is_u64());
            assert!(key.get("block").is_none());
        }
        let stack = &state["entry"]["call_stack"];
        for frame in std::iter::once(&stack["root"]).chain(stack["children"].as_array().unwrap()) {
            assert!(frame["state"]["key"]["basic_block_index"].is_u64());
            assert!(frame["state"]["key"].get("block").is_none());
        }
    }
    assert!(!json["outcomes"].as_array().unwrap().is_empty());
}

#[test]
fn primary_analyze_json_has_world_call_frames_returns_and_outcomes() {
    let output = analyze("call-return-branch", &["--format", "json"]);
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    native::assert_exit(&raw, &output);
    assert_eq!(output.status.code(), Some(0));
    assert_shared_budget(&raw, 1, 2_000_000_000);
    assert_eq!(native::global(&raw)["status"], "Converged");
    assert_eq!(raw["hybrid"]["coverage"]["detail_current"], true);
    assert_eq!(raw["hybrid"]["coverage"]["detail_binding"], "OriginalOwner");
    let json = native::select(&raw);
    assert_call_return_world(&json);
    assert!(
        json["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["kind"] == serde_json::json!({"Summary": "Return"}))
    );
    let child = "0x0000000000000000000000000000000000000200";
    let procedure = relative_native_call(
        &json,
        child,
        "0x0000000000000000000000000000000000000101",
        false,
    );
    let returned = procedure["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|output| output["kind"] == "Return")
        .unwrap();
    assert_eq!(
        returned["data"]["length"]["numeric"]["constants"],
        serde_json::json!(["0x20"])
    );
    let mut word = vec![0; 32];
    word[31] = 1;
    assert_eq!(
        returned["data"]["content"]["Exact"],
        serde_json::json!(word)
    );
    assert!(
        json["states"]
            .as_array()
            .unwrap()
            .iter()
            .filter(
                |state| state["key"]["frames"].as_array().unwrap().last().unwrap()["code_address"]
                    == child
            )
            .all(|state| state["executed_pcs"] == serde_json::json!([]))
    );
}

#[test]
fn primary_analyze_two_private_regions_preserve_world_return_edges() {
    let output = analyze(
        "call-return-branch",
        &["--format", "json", "--max-detail-regions", "2"],
    );
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    native::assert_exit(&raw, &output);
    assert_eq!(output.status.code(), Some(0));
    assert_shared_budget(&raw, 2, 2_000_000_000);
    let json = native::select(&raw);
    assert_call_return_world(&json);
    assert!(
        json["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["kind"] == "Return")
    );
    assert!(json["states"].as_array().unwrap().iter().any(|state| {
        state["key"]["frames"].as_array().unwrap().len() == 2
            && state["executed_pcs"] == serde_json::json!([0, 2, 3, 4, 6, 7])
    }));
}

#[test]
fn installed_json_keeps_graph_placement_and_source_qualified_prefixes_with_ssa() {
    for ssa in [false, true] {
        let mut arguments = vec!["--format", "json"];
        if ssa {
            arguments.push("--ssa");
        }
        let output = analyze("identity-precompile", &arguments);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Deliberately inspect actual installed bytes before any test view selection.
        let wire: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(wire["selected_view"], "Native");
        assert_eq!(wire["status"], "Converged");
        assert!(wire.get("states").is_none() && wire.get("analysis").is_none());
        assert!(wire["global"]["analysis"]["states"].is_array());
        assert!(wire["native"]["analysis"]["states"].is_array());
        assert_eq!(wire["native"].get("ssa").is_some(), ssa);
        assert!(wire["hybrid"].get("baseline").is_none());
        assert!(wire["hybrid"].get("detail").is_none());
        assert!(wire["hybrid"].get("detail_ssa").is_none());
        for prefix in wire["hybrid"]["local_detail"].as_array().unwrap() {
            assert_eq!(prefix["namespace"], "Native");
            assert_eq!(prefix["revision"], wire["native"]["analysis"]["revision"]);
            let state = prefix["state_id"].as_u64().unwrap();
            assert!(
                wire["native"]["analysis"]["states"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|row| row["id"] == state)
            );
        }
    }
}

#[test]
fn world_ssa_verifies_complete_calls_and_is_not_built_for_frontiers() {
    let output = analyze("proxy-storage", &["--format", "json", "--ssa"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["analysis"]["status"], "Converged");
    assert!(json["ssa"].is_object());
    let incomplete = analyze(
        "missing-code",
        &["--format", "json", "--ssa", "--max-call-depth", "1"],
    );
    let raw: serde_json::Value = serde_json::from_slice(&incomplete.stdout).unwrap();
    let raw = crate::native::select(&raw);
    native::assert_exit(&raw, &incomplete);
    let json = native::select(&raw);
    assert_eq!(json["status"], "Incomplete");
    assert!(json.get("ssa").is_none());
    assert!(String::from_utf8_lossy(&incomplete.stderr).contains("SSA unavailable"));
}

#[test]
fn shared_budgets_keep_typed_frontiers_and_exit_two() {
    let bounded = analyze(
        "call-return-branch",
        &["--format", "json", "--max-memory-bytes", "1"],
    );
    assert!(
        bounded.status.success(),
        "{}",
        String::from_utf8_lossy(&bounded.stderr)
    );
    let bounded: serde_json::Value = serde_json::from_slice(&bounded.stdout).unwrap();
    assert_shared_budget(&bounded, 1, 2_000_000_000);
    let bounded = crate::native::select(&bounded);
    assert_eq!(bounded["config"]["max_memory_bytes"], 1);
    assert_eq!(bounded["status"], "Converged");
    assert_eq!(bounded["frontiers"], serde_json::json!([]));
    assert!(
        bounded["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| outcome["kind"] == "Return")
    );
    for (name, args, reason) in [
        (
            "missing-code",
            vec!["--format", "json", "--max-call-depth", "1"],
            serde_json::json!("CallDepth"),
        ),
        (
            "reentry",
            vec!["--format", "json", "--max-call-depth", "2"],
            serde_json::json!("CallDepth"),
        ),
        (
            "call-return-branch",
            vec!["--format", "json", "--max-work", "1"],
            serde_json::json!("Work"),
        ),
        (
            "call-return-branch",
            vec!["--format", "json", "--max-symbolic-nodes", "2"],
            serde_json::json!({"Relations":"ExpressionLimit"}),
        ),
    ] {
        let mut detail = args.clone();
        detail.push("--no-root-terminal-coverage");
        let output = analyze(name, &detail);
        let raw: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        native::assert_exit(&raw, &output);
        assert_shared_budget(
            &raw,
            1,
            if args.contains(&"--max-work") {
                1
            } else {
                2_000_000_000
            },
        );
        let global = native::global(&raw);
        let coverage = &raw["hybrid"]["coverage"];
        assert_eq!(coverage["requested_native_terminal_coverage"], false);
        if name == "reentry" {
            // Relative procedures close without unrolling the private stack.
            // The separate two-region control retains the same depth refusal.
            assert_eq!(output.status.code(), Some(0));
            assert_eq!(global["status"], "Converged");
            assert_eq!(coverage["detail_current"], true);
            let json = native::select(&raw);
            assert_eq!(json["config"]["max_call_depth"], 2);
            assert_eq!(json["status"], "Converged");
            assert_eq!(json["frontiers"], serde_json::json!([]));
            let procedure = relative_native_call(
                &json,
                "0x0000000000000000000000000000000000000200",
                "0x0000000000000000000000000000000000000101",
                false,
            );
            assert!(
                procedure["outputs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|output| output["kind"] == "Return"
                        && output["data"]["length"]["numeric"]["constants"]
                            == serde_json::json!(["0x20"]))
            );
        } else {
            let json = if args.contains(&"--max-call-depth") {
                assert_eq!(output.status.code(), Some(0));
                assert_eq!(global["status"], "Converged");
                assert_eq!(coverage["baseline_current"], true);
                assert_eq!(coverage["detail"], "Incomplete");
                let json = &raw["native"]["analysis"];
                assert!(json.is_object());
                assert_eq!(json["config"]["max_call_depth"], 1);
                json
            } else {
                // These caps refuse Global work before Native admission.
                assert_eq!(output.status.code(), Some(2));
                assert_eq!(global["status"], "Incomplete");
                assert!(raw["native"].is_null());
                assert_eq!(coverage["baseline_current"], false);
                assert_eq!(coverage["detail"], "BaselinePending");
                assert_eq!(raw["hybrid"]["local_detail"], serde_json::json!([]));
                global
            };
            assert_eq!(coverage["detail_current"], false);
            assert_eq!(json["status"], "Incomplete");
            assert!(
                json["frontiers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|frontier| frontier["reason"] == reason)
            );
            if args.contains(&"--max-symbolic-nodes") {
                assert_eq!(json["domain_spec"]["relations"]["max_nodes"], 2);
                assert!(
                    json["frontiers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|frontier| {
                            frontier["reason"] == reason
                                && frontier["from"].is_u64()
                                && frontier["pc"].is_u64()
                        })
                );
            }
        }
    }
}

#[test]
fn reentry_two_private_regions_retains_the_original_call_depth_frontier() {
    let output = analyze(
        "reentry",
        &[
            "--format",
            "json",
            "--max-call-depth",
            "2",
            "--max-detail-regions",
            "2",
            "--no-root-terminal-coverage",
        ],
    );
    let raw: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    native::assert_exit(&raw, &output);
    assert_shared_budget(&raw, 2, 2_000_000_000);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(native::global(&raw)["status"], "Converged");
    assert_eq!(raw["hybrid"]["coverage"]["detail_current"], false);
    let json = native::select(&raw);
    assert_eq!(json["config"]["max_call_depth"], 2);
    assert_eq!(json["status"], "Incomplete");
    assert!(
        json["frontiers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frontier| frontier["reason"] == "CallDepth"
                && frontier["from"].is_u64()
                && frontier["pc"] == 13)
    );
}

#[test]
fn world_text_and_dot_expose_code_storage_and_call_identity() {
    let text = analyze("proxy-storage", &[]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    let state_header = text
        .lines()
        .find(|line| line.contains("Stack height") && line.contains("Code hash"))
        .unwrap();
    let columns: Vec<_> = state_header.split('|').map(str::trim).collect();
    assert!(
        columns.contains(&"Code") && columns.contains(&"Address") && columns.contains(&"Caller")
    );
    for address in [
        "0x0000000000000000000000000000000000000101",
        "0x0000000000000000000000000000000000000201",
        "0x0000000000000000000000000000000000000202",
        "0x0000000000000000000000000000000000000300",
    ] {
        assert!(
            text.contains(address),
            "missing full account identity {address}"
        );
    }
    assert!(text.contains("Call") && text.contains("Return"));
    let dot = analyze("proxy-storage", &["--format", "dot"]);
    assert!(dot.status.success());
    let dot = String::from_utf8(dot.stdout).unwrap();
    assert!(dot.contains("digraph world") && dot.contains("code=") && dot.contains("address="));
    assert!(dot.contains("Call") && dot.contains("Return"));
}

#[test]
fn requested_world_text_is_readable_and_format_switches_keep_complete_evidence() {
    let text = analyze("returndata-copy", &[]);
    assert!(text.status.success());
    let text = String::from_utf8(text.stdout).unwrap();
    for section in [
        "Analysis",
        "Snapshot",
        "States",
        "State details",
        "Transitions",
        "Outcomes",
        "Call summaries",
        "Diagnostics",
        "Frontiers",
    ] {
        assert!(
            text.lines().any(|line| line.trim() == section),
            "missing section {section}:\n{text}"
        );
    }
    assert!(text.contains("stack in") && text.contains("stack out"));
    let json = analyze("returndata-copy", &["--format", "json"]);
    assert!(json.status.success());
    let json: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["status"], "Converged");
    let outcomes = json["outcomes"].as_array().unwrap();
    assert!(outcomes.iter().any(|outcome| outcome["kind"] == "Failure"));
    assert!(outcomes.iter().any(|outcome| outcome["kind"] == "Return"));
    let outcome_headers: Vec<_> = text
        .lines()
        .filter(|line| {
            let first = line.split('|').next().unwrap().trim();
            first.strip_prefix('O').is_some_and(|index| {
                !index.is_empty()
                    && normalize_subscripts(index)
                        .bytes()
                        .all(|byte| byte.is_ascii_digit())
            })
        })
        .collect();
    assert_eq!(
        outcome_headers.len(),
        outcomes.len(),
        "text must preserve every JSON outcome"
    );
    for outcome in outcomes {
        assert!(outcome["data"]["length"].is_object());
        assert!(outcome["data"]["bytes"].is_object());
        assert!(outcome["data"]["default"].is_object());
        assert!(outcome["store"]["account_observations"].is_array());
    }
    let dot = analyze("returndata-copy", &["--format", "dot"]);
    assert!(dot.status.success());
    let dot = String::from_utf8(dot.stdout).unwrap();
    for (index, outcome) in outcomes.iter().enumerate() {
        assert!(dot.contains(&format!("O{index} [label=")));
        assert!(dot.contains(&format!(
            "S{} -> O{index}",
            outcome["state"].as_u64().unwrap()
        )));
    }
    let incomplete = analyze("missing-code", &["--max-call-depth", "1"]);
    let incomplete = String::from_utf8(incomplete.stdout).unwrap();
    assert!(
        incomplete.contains("Incomplete")
            && incomplete.contains("Frontiers")
            && incomplete.contains("CallDepth")
    );
}

#[test]
fn unknown_code_world_converges_with_closed_summary_evidence_and_verified_ssa() {
    let output = analyze("missing-code", &["--format", "json", "--ssa"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let json = crate::native::select(&json);
    let analysis = &json["analysis"];
    assert_eq!(analysis["status"], "Converged");
    assert!(analysis["frontiers"].as_array().unwrap().is_empty());
    let summaries = analysis["procedure_summaries"].as_array().unwrap();
    assert!(!summaries.is_empty());
    assert!(summaries.iter().all(|summary| summary["closed"] == true));
    assert!(json["ssa"].is_object());
    assert!(
        analysis["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["kind"].get("Summary").is_some())
    );
}

#[test]
fn world_entry_and_data_errors_are_reported_before_execution() {
    for (args, expected) in [
        (vec!["--evm.to", "0x01"], "EVM environment address"),
        (vec!["--evm.calldata", "0xzz"], "calldata hex"),
        (vec!["--evm.value", "-1"], "unexpected argument"),
        (vec!["--evm.value", "0xzz"], "ASCII decimal digits"),
    ] {
        let output = analyze("call-return-branch", &args);
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn entry_static_mode_faults_at_write_and_returns_a_completed_failure() {
    let output = analyze("call-return-branch", &["--format", "json", "--evm.static"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let json = crate::native::select(&json);
    assert_eq!(json["entry"]["environment"]["is_static"], true);
    assert_eq!(json["status"], "Converged");
    assert!(
        json["outcomes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|outcome| outcome["kind"] == "Failure")
    );
}

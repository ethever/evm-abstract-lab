//! 教学视图通过真实 CLI 验证，完整机器证据仍可由 verbose/analyze 取得。

use evm_abstract_notation::Symbol;

use super::run_concrete;
use serde_json::Value as Json;
use std::{
    fs,
    path::PathBuf,
    process::Output,
    sync::atomic::{AtomicUsize, Ordering},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const VERIFIED: &str = "Verified cross-contract SSA:";

fn fixture(name: &str) -> String {
    format!(
        "{}/../../examples/worlds/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    )
}

fn world(command: &str, name: &str, extra: &[&str]) -> Output {
    world_path(command, &fixture(name), extra)
}

fn world_path(command: &str, path: &str, extra: &[&str]) -> Output {
    let mut args = vec![command, "--world", path, "--evm.to", ENTRY];
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

fn json(output: &Output) -> Json {
    success(output);
    crate::native::select(&serde_json::from_slice(&output.stdout).unwrap())
}

fn variant(value: &Json) -> &str {
    value
        .as_str()
        .or_else(|| {
            value
                .as_object()
                .and_then(|fields| fields.keys().next().map(String::as_str))
        })
        .unwrap()
}

fn assert_diagnostics(explanation: &str, report: &Json) {
    let section = explanation
        .split_once("\nDiagnostics\n")
        .unwrap()
        .1
        .split_once("\nFrontiers\n")
        .unwrap()
        .0;
    let rows = section
        .lines()
        .filter(|line| {
            line.trim_start().strip_prefix('σ').is_some_and(|tail| {
                tail.starts_with(|character: char| "₀₁₂₃₄₅₆₇₈₉".contains(character))
            })
        })
        .count();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(rows, diagnostics.len());
    for diagnostic in diagnostics {
        let state = format!(
            "{}",
            Symbol::State((diagnostic["state"]).as_u64().unwrap() as usize)
        );
        let pc = format!("0x{:04x}", diagnostic["pc"].as_u64().unwrap());
        let kind = variant(&diagnostic["kind"]);
        assert!(
            section.lines().any(|line| {
                line.trim_start().starts_with(&state) && line.contains(&pc) && line.contains(kind)
            }),
            "missing diagnostic {state} {pc} {kind}"
        );
    }
}

fn assert_frontiers(explanation: &str, report: &Json) {
    let frontiers = explanation.split_once("\nFrontiers\n").unwrap().1;
    let rows = frontiers
        .lines()
        .filter(|line| {
            line.trim_start().strip_prefix('U').is_some_and(|tail| {
                tail.starts_with(|character: char| matches!(character, '₀'..='₉'))
                    && tail.contains(" | from=")
            })
        })
        .count();
    assert_eq!(rows, report["frontiers"].as_array().unwrap().len());
    for (id, frontier) in report["frontiers"].as_array().unwrap().iter().enumerate() {
        let line = frontiers
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("{} | from=", Symbol::Frontier(id)))
            })
            .unwrap();
        assert!(line.contains(variant(&frontier["reason"])));
        let source = frontier["from"].as_u64().map_or_else(
            || "none".to_owned(),
            |state| format!("{}", Symbol::State(state as usize)),
        );
        assert!(line.contains(&format!("from={source}")));
    }
    assert_diagnostics(explanation, report);
}

struct EntryCode(PathBuf);

impl EntryCode {
    fn new(code: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-abstract-teaching-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let mut input: Json =
            serde_json::from_slice(&fs::read(fixture("identity-precompile")).unwrap()).unwrap();
        input["accounts"][0]["code"] = code.into();
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        Self(path)
    }

    fn run_concrete(&self, command: &str, extra: &[&str]) -> Output {
        world_path(command, self.0.to_str().unwrap(), extra)
    }
}

impl Drop for EntryCode {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn default_world_explain_connects_disassembly_cfg_values_and_separate_outcomes() {
    let output = world("explain", "call-return-branch", &[]);
    success(&output);
    let explanation = text(&output);
    for section in [
        "Execution code",
        "CFG",
        "Outcomes",
        "Diagnostics",
        "Frontiers",
    ] {
        assert!(
            explanation.lines().any(|line| line.trim() == section),
            "missing {section}"
        );
    }
    for evidence in [
        "PUSH1",
        "MLOAD",
        "SSTORE",
        "stack in",
        "stack out",
        VERIFIED,
    ] {
        assert!(explanation.contains(evidence), "missing {evidence}");
    }
    assert!(explanation.contains(" = φ(T"));
    assert!(explanation.contains("origin=MLOAD") || explanation.contains(" = MLOAD "));
    assert!(explanation.contains("SSTORE "));
    assert!(explanation.contains("BranchTrue") && explanation.contains("BranchFalse"));
    let report = json(&world(
        "analyze",
        "call-return-branch",
        &["--format", "json"],
    ));
    for (id, outcome) in report["outcomes"].as_array().unwrap().iter().enumerate() {
        let line = explanation
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("{} |", Symbol::Outcome(id)))
            })
            .unwrap_or_else(|| panic!("missing separate outcome O{id}"));
        assert!(line.contains(outcome["kind"].as_str().unwrap()));
        assert!(line.contains(&format!(
            "{}",
            Symbol::State((outcome["state"]).as_u64().unwrap() as usize)
        )));
    }
    for detail in [
        "captured frames:",
        "Active abstract transfers:",
        "machine code identity=",
        "jump history=",
        "frame φ functions (stack in):",
        "instruction effects:",
        "opcode=",
        "immediate=",
        "operands=",
        "results=",
        "fault=",
    ] {
        assert!(
            !explanation.contains(detail),
            "verbose field in teaching view: {detail}"
        );
    }
}

#[test]
fn teaching_tac_names_definitions_and_keeps_evm_pop_order() {
    // SUB is deliberately noncommutative; MSTORE and RETURN also expose operand roles.
    let input = EntryCode::new("0x60096002035f5260205ff3");
    let output = input.run_concrete("explain", &[]);
    success(&output);
    let explanation = text(&output);
    for operation in [
        "%0 = CONST 0x9 ; origin=PUSH1",
        "%1 = CONST 0x2 ; origin=PUSH1",
        "%2 = CONST 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9 ; origin=SUB",
        "MSTORE 0x0 0xfffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff9",
        "RETURN 0x0 0x20",
    ] {
        assert!(
            explanation.contains(operation),
            "missing TAC operation {operation}"
        );
    }
    assert!(!explanation.contains("%2 = SUB %0 %1"));
}

#[test]
fn teaching_transitions_explain_deferred_results_and_commit_or_rollback() {
    let call = world("explain", "call-return-branch", &[]);
    success(&call);
    let call = text(&call);
    assert!(call.contains("CALL result %") && call.contains(" = 1") && call.contains(" = 0"));
    assert!(call.contains("rollback checkpoint"));
    assert!(call.contains("from Return summary; apply summarized child effects"));
    assert!(call.contains("from Failure summary; rollback to saved checkpoint"));

    // The default uses a relative callee. Retain the original private-frame
    // transition oracle as a separate two-source request with unchanged inputs.
    let detail = world(
        "explain",
        "call-return-branch",
        &["--max-detail-regions", "2"],
    );
    success(&detail);
    let detail = text(&detail);
    assert!(detail.contains("CALL result %") && detail.contains(" = 1") && detail.contains(" = 0"));
    assert!(detail.contains("rollback checkpoint"));
    assert!(detail.contains("commit child effects"));
    let original = json(&world(
        "analyze",
        "call-return-branch",
        &["--format", "json"],
    ));
    let detailed = json(&world(
        "analyze",
        "call-return-branch",
        &["--format", "json", "--max-detail-regions", "2"],
    ));
    assert_eq!(
        original["hybrid"]["coverage"]["requested_detail_regions"],
        1
    );
    assert_eq!(
        detailed["hybrid"]["coverage"]["requested_detail_regions"],
        2
    );
    let input = |report: &Json| {
        serde_json::json!([
            report["entry"],
            report["world"],
            report["config"],
            report["domain_spec"]
        ])
    };
    assert_eq!(
        super::native::canonical(&input(&original)),
        super::native::canonical(&input(&detailed))
    );
    assert_eq!(original["config"]["max_work"], 2_000_000_000_u64);
    assert_eq!(original["config"]["analysis"]["max_transfers"], 10_000_000);

    let revert = world("explain", "revert-rollback", &[]);
    success(&revert);
    let revert = text(&revert);
    assert!(revert.contains("Revert"));
    assert!(revert.contains("rollback to saved checkpoint"));
    assert!(revert.contains("retain revert data"));

    let creation = world("explain", "create-runtime", &[]);
    success(&creation);
    let creation = text(&creation);
    assert!(creation.contains("CREATE address %"));
    assert!(creation.contains(" = created address") && creation.contains(" = 0"));
}

#[test]
fn teaching_code_catalogue_preserves_proxy_init_runtime_and_native_identity() {
    let proxy = world("explain", "proxy-storage", &[]);
    success(&proxy);
    let proxy = text(&proxy);
    for address in [
        ENTRY,
        "0x0000000000000000000000000000000000000201",
        "0x0000000000000000000000000000000000000202",
        "0x0000000000000000000000000000000000000300",
    ] {
        assert!(
            proxy.contains(address),
            "missing distinct code/owner {address}"
        );
    }
    assert!(proxy.contains("DELEGATECALL"));
    assert!(proxy.contains("owner=") || proxy.contains("storage owner="));
    // The implementation's Native relative blocks have no private instruction
    // listing in this renderer. Check the complete source catalogue, then its
    // disassembly and qualified execution independently of World PC/SSA IDs.
    let report = json(&world("analyze", "proxy-storage", &["--format", "json"]));
    assert_eq!(report["hybrid"]["coverage"]["requested_detail_regions"], 1);
    assert_eq!(report["hybrid"]["coverage"]["detail_current"], true);
    let input: Json = serde_json::from_slice(&fs::read(fixture("proxy-storage")).unwrap()).unwrap();
    for account in input["accounts"].as_array().unwrap() {
        let address = account["address"].as_str().unwrap();
        let captured = &report["world"]["accounts"][address]["code"]["Runtime"];
        assert_eq!(captured["bytecode"], account["code"], "{address}");
        assert_eq!(captured["fork"], "osaka");
        assert_eq!(
            captured["byte_len"].as_u64().unwrap() as usize,
            (account["code"].as_str().unwrap().len() - 2) / 2
        );
    }
    let implementation = "0x0000000000000000000000000000000000000300";
    let code = report["world"]["accounts"][implementation]["code"]["Runtime"]["bytecode"]
        .as_str()
        .unwrap();
    let disassembly = run_concrete(&["disasm", "--hex", code]);
    success(&disassembly);
    let listing = text(&disassembly);
    let instructions: Vec<_> = listing
        .lines()
        .filter_map(|line| {
            let (pc, instruction) = line.trim().split_once(':')?;
            (pc.len() == 4 && pc.chars().all(|digit| digit.is_ascii_hexdigit()))
                .then(|| instruction.split_whitespace().collect::<Vec<_>>().join(" "))
        })
        .collect();
    assert_eq!(
        instructions,
        [
            "PUSH0 0x0",
            "SLOAD",
            "PUSH1 0x1",
            "ADD",
            "DUP1",
            "PUSH0 0x0",
            "SSTORE",
            "PUSH0 0x0",
            "MSTORE",
            "ADDRESS",
            "PUSH1 0x1",
            "SSTORE",
            "CALLER",
            "PUSH1 0x2",
            "SSTORE",
            "CALLVALUE",
            "PUSH1 0x3",
            "SSTORE",
            "PUSH1 0x20",
            "PUSH0 0x0",
            "RETURN",
        ]
    );
    let hash = format!(
        "{:#x}",
        alloy_primitives::keccak256(alloy_primitives::hex::decode(code).unwrap())
    );
    for owner in [
        "0x0000000000000000000000000000000000000201",
        "0x0000000000000000000000000000000000000202",
    ] {
        let summary = report["procedure_summaries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|summary| {
                summary["regional_cfg"]["nodes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|node| {
                        node["frame"]["code_address"] == implementation
                            && node["frame"]["address"] == owner
                    })
            })
            .unwrap();
        assert_eq!(summary["closed"], true);
        assert_eq!(summary["source_current"], true);
        let node = summary["regional_cfg"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| {
                node["frame"]["code_address"] == implementation && node["frame"]["address"] == owner
            })
            .unwrap();
        assert_eq!(node["frame"]["code_hash"], hash);
        assert_eq!(
            node["frame"]["caller"],
            serde_json::json!({"Concrete":ENTRY})
        );
        assert_eq!(node["frame"]["is_static"], false);
        assert_eq!(node["frame"]["basic_block_index"], 0);
        assert_eq!(node["excluded"], false);
        assert_eq!(node["transfer"]["semantics"], "Native");
        assert_eq!(node["transfer"]["input_revision"], node["input_revision"]);
        for field in ["current", "complete", "visited_block"] {
            assert_eq!(node["transfer"][field], true, "{field}");
        }
        assert!(proxy.contains(&format!(
            "code={implementation} | storage namespace={owner} | static=false"
        )));
        assert!(proxy.contains(&format!("frame identity hash={hash}")));
    }

    let creation = world("explain", "create-runtime", &[]);
    success(&creation);
    let creation = text(&creation);
    assert!(creation.contains("mode=InitCode") && creation.contains("mode=Runtime"));
    assert!(creation.contains("Captured instruction list") && creation.contains("MSTORE"));
    assert!(
        creation
            .to_ascii_lowercase()
            .contains("0xea53a153a9a04fd632b2486d84732feb3b71afb7")
    );

    let native = world("explain", "identity-precompile", &[]);
    success(&native);
    let native = text(&native);
    assert!(
        native.contains("Native precompile") && native.contains("no bytecode instruction list")
    );
    assert!(native.contains("native execution; no bytecode instructions"));
}

#[test]
fn empty_code_and_unstarted_analysis_never_fabricate_captured_instructions() {
    let empty = EntryCode::new("0x").run_concrete("explain", &[]);
    success(&empty);
    let empty = text(&empty);
    assert!(empty.contains("Empty executable code") && empty.contains("implicit"));
    assert!(!empty.contains("Captured instruction list"));

    let unstarted = world("explain", "call-return-branch", &["--max-work", "1"]);
    assert_eq!(unstarted.status.code(), Some(2));
    let unstarted = text(&unstarted);
    assert!(unstarted.contains("Input code observations (not execution evidence)"));
    assert!(unstarted.contains("Observed instruction list") && unstarted.contains("CALL"));
    assert!(!unstarted.contains("Captured instruction list"));
    assert!(unstarted.contains("SSA unavailable") && !unstarted.contains(VERIFIED));
}

#[test]
fn incomplete_teaching_keeps_every_frontier_and_diagnostic() {
    for (name, extra, global_status, native_status) in [
        (
            "missing-code",
            vec!["--max-call-depth", "1"],
            "Converged",
            Some("Incomplete"),
        ),
        (
            "call-return-branch",
            vec!["--max-work", "1"],
            "Incomplete",
            None,
        ),
        (
            "call-return-branch",
            vec!["--max-symbolic-nodes", "2"],
            "Incomplete",
            None,
        ),
        (
            "reentry",
            vec!["--max-call-depth", "2"],
            "Converged",
            Some("Converged"),
        ),
        // Retain the original depth-2 request above. Selecting the child as
        // well exercises its private nested call and the original CallDepth
        // frontier, while sharing the same work and transfer limits.
        (
            "reentry",
            vec!["--max-call-depth", "2", "--max-detail-regions", "2"],
            "Converged",
            Some("Incomplete"),
        ),
        (
            "call-return-branch",
            vec!["--max-states", "1"],
            "Incomplete",
            None,
        ),
        (
            "call-return-branch",
            vec!["--max-transfers", "1"],
            "Incomplete",
            None,
        ),
    ] {
        let mut detail = extra.clone();
        detail.push("--no-root-terminal-coverage");
        let output = world("explain", name, &detail);
        let explanation = text(&output);
        let mut query = detail.clone();
        query.extend(["--format", "json"]);
        let report = world("analyze", name, &query);
        let raw: Json = serde_json::from_slice(&report.stdout).unwrap();
        super::native::assert_exit(&raw, &report);
        super::native::assert_exit(&raw, &output);
        assert_eq!(raw["status"], global_status);
        let global = super::native::global(&raw);
        assert_eq!(global["status"], global_status);
        let native = (!raw["native"].is_null()).then(|| super::native::select(&raw));
        assert_eq!(
            native
                .as_ref()
                .map(|native| native["status"].as_str().unwrap()),
            native_status
        );
        let regions = if extra.contains(&"--max-detail-regions") {
            2
        } else {
            1
        };
        assert_eq!(
            raw["hybrid"]["coverage"]["requested_detail_regions"],
            regions
        );
        assert_eq!(global["config"]["root_terminal_coverage"], false);
        assert_eq!(
            raw["hybrid"]["coverage"]["baseline_current"],
            global_status == "Converged"
        );
        assert_eq!(
            raw["hybrid"]["coverage"]["requested_native_terminal_coverage"],
            false
        );
        let work = &raw["hybrid"]["coverage"]["work"];
        assert_eq!(work["total"], raw["work"]);
        assert_eq!(
            work["total"].as_u64().unwrap(),
            ["prior", "baseline", "preparation", "detail"]
                .into_iter()
                .map(|field| work[field].as_u64().unwrap())
                .sum::<u64>()
        );
        assert!(raw["work"].as_u64().unwrap() <= global["config"]["max_work"].as_u64().unwrap());
        let refusal = match extra[0] {
            "--max-work" => Some(serde_json::json!("Work")),
            "--max-states" => Some(serde_json::json!({"Budget":"States"})),
            "--max-transfers" => Some(serde_json::json!({"Budget":"Transfers"})),
            _ => None,
        };
        if let Some(refusal) = refusal {
            let frontiers = global["frontiers"].as_array().unwrap();
            assert!(!frontiers.is_empty());
            for frontier in frontiers {
                assert_eq!(frontier["reason"], refusal);
            }
        }

        // Each rendered graph owns its own state/frontier IDs. A closed Global
        // relation can coexist with pending Native paths in the same response.
        let (primary, companion) = explanation
            .split_once("\nDetailed paths (Native state IDs; conditions and pending work above):\n")
            .map_or((explanation.as_str(), None), |(primary, companion)| {
                (primary, Some(companion))
            });
        if raw["selected_view"] == "Native" {
            let native = native.as_ref().unwrap();
            assert_frontiers(primary, native);
            assert!(primary.contains(VERIFIED));
            assert_eq!(native["frontiers"], serde_json::json!([]));
        } else {
            assert_frontiers(primary, global);
            assert!(primary.contains("SSA unavailable") && !primary.contains(VERIFIED));
            if let Some(native) = native.as_ref() {
                let companion = companion.unwrap();
                assert_frontiers(companion, native);
                assert!(companion.contains("Incomplete"));
                assert!(companion.contains("SSA unavailable") && !companion.contains(VERIFIED));
                assert!(
                    native["frontiers"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|frontier| {
                            frontier["reason"] == "CallDepth"
                                && frontier["from"].is_u64()
                                && frontier["pc"].is_u64()
                        })
                );
            }
        }
        if extra == ["--max-symbolic-nodes", "2"] {
            assert_eq!(global["domain_spec"]["relations"]["max_nodes"], 2);
            let frontiers = global["frontiers"].as_array().unwrap();
            assert_eq!(frontiers.len(), 3);
            for frontier in frontiers {
                assert_eq!(
                    frontier["reason"],
                    serde_json::json!({"Relations":"ExpressionLimit"})
                );
                assert!(frontier["from"].is_u64() && frontier["pc"].is_u64());
            }
            assert!(frontiers.iter().any(|frontier| frontier["pc"] == 13));
            // Global refusal locations are source-control coordinates, while
            // executed instruction PCs remain absent from the Process state.
            assert_eq!(global["states"][0]["executed_pcs"], serde_json::json!([]));
        }
        if name == "reentry" {
            assert_eq!(global["config"]["max_call_depth"], 2);
            assert_eq!(global["config"]["max_work"], 2_000_000_000_u64);
            assert_eq!(global["config"]["analysis"]["max_transfers"], 10_000_000);
            if regions == 1 {
                let native = native.as_ref().unwrap();
                let summary = &native["procedure_summaries"][0];
                assert_eq!(summary["source_current"], true);
                assert_eq!(summary["closed"], true);
                assert_eq!(
                    summary["regional_cfg"]["calls"].as_array().unwrap().len(),
                    1
                );
                for source in [ENTRY, "0x0000000000000000000000000000000000000200"] {
                    assert!(
                        summary["procedures"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .any(|procedure| {
                                procedure["semantics"] == "Native"
                                    && procedure["frame"]["code_address"] == source
                            })
                    );
                }
            }
        }
    }
}

#[test]
fn converged_teaching_retains_precision_diagnostics() {
    let input = EntryCode::new("0x5a565b00");
    let output = input.run_concrete("explain", &[]);
    success(&output);
    let report = json(&input.run_concrete("analyze", &["--format", "json"]));
    assert!(!report["diagnostics"].as_array().unwrap().is_empty());
    assert_diagnostics(&text(&output), &report);
    assert!(text(&output).contains("UnknownJump"));
}

#[test]
fn verbose_preserves_full_analyze_report_and_original_ssa_fields() {
    let verbose = world("explain", "call-return-branch", &["--verbose"]);
    success(&verbose);
    let verbose = text(&verbose);
    let full = world("analyze", "call-return-branch", &["--ssa"]);
    success(&full);
    let full = text(&full);
    let (model, machine_report) = full.split_once('\n').unwrap();
    assert!(verbose.lines().any(|line| line == model));
    assert!(
        verbose.contains(
            machine_report
                .split_once("Analysis\n")
                .unwrap()
                .1
                .trim_end()
        )
    );
    for detail in [
        "State details",
        "Call summaries",
        "captured frames:",
        "machine code identity=",
        "code hash=",
        "jump history=",
        "effect φ(",
        "opcode=",
        "immediate=",
        "operands=",
        "results=",
        "fault=",
        "instruction effects:",
        "exit effect:",
        "deferred result:",
    ] {
        assert!(
            verbose.contains(detail),
            "missing original verbose field {detail}"
        );
    }
    let teaching = world("explain", "call-return-branch", &[]);
    success(&teaching);
    assert!(teaching.stdout.len() < verbose.len());
}

#[test]
fn analyze_json_and_full_ssa_keep_the_same_semantic_analysis() {
    for name in [
        "call-return-branch",
        "proxy-storage",
        "create-runtime",
        "identity-precompile",
    ] {
        let report = json(&world("analyze", name, &["--format", "json"]));
        let with_ssa = json(&world("analyze", name, &["--ssa", "--format", "json"]));
        assert_eq!(
            super::native::canonical(&with_ssa["analysis"]),
            super::native::canonical(&super::native::without_application_metadata(&report)),
            "{name}"
        );
        assert_eq!(report["numeric_model"], with_ssa["numeric_model"]);
        super::native::without_application_metadata(&with_ssa);
        assert_eq!(report["postprocessing_work"], 0);
        assert!(with_ssa["postprocessing_work"].as_u64().unwrap() > 0);
        assert_eq!(report["status"], "Converged");
        assert!(report["world"]["fingerprint"].is_string());
        assert!(
            report["outcomes"]
                .as_array()
                .unwrap()
                .iter()
                .all(|outcome| {
                    outcome["data"].is_object()
                        && outcome["data"]["length"].is_object()
                        && outcome["store"]["persistent"]["slots"].is_array()
                })
        );
        assert!(with_ssa["ssa"]["blocks"].is_array());
        assert!(with_ssa["ssa"]["transitions"].is_array());
        if name == "call-return-branch" {
            for value in ["0x1", "0x2"] {
                assert!(
                    report["outcomes"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|outcome| {
                            outcome["kind"] == "Return"
                                && outcome["store"]["persistent"]["slots"]
                                    .as_array()
                                    .unwrap()
                                    .iter()
                                    .any(|slot| {
                                        slot["address"] == ENTRY
                                            && slot["slot"] == "0x0"
                                            && slot["value"]["numeric"]["constants"]
                                                == serde_json::json!([value])
                                    })
                        })
                );
            }
            assert!(
                report["outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|outcome| {
                        outcome["kind"] == "Failure"
                            && outcome["store"]["persistent"]["slots"]
                                .as_array()
                                .unwrap()
                                .is_empty()
                    })
            );
        }
    }
}

#[test]
fn verbose_requires_world_or_rpc_input() {
    let path = format!(
        "{}/../../examples/straight-line.hex",
        env!("CARGO_MANIFEST_DIR")
    );
    for source in [["--hex", "00"], ["--file", path.as_str()]] {
        let output = run_concrete(&["explain", source[0], source[1], "--verbose"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

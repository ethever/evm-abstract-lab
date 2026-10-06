//! 教学视图通过真实 CLI 验证，完整机器证据仍可由 verbose/analyze 取得。

use super::run;
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
    let mut args = vec![command, "--world", path, "--entry", ENTRY];
    args.extend_from_slice(extra);
    run(&args)
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
    serde_json::from_slice(&output.stdout).unwrap()
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
            line.trim_start()
                .strip_prefix('S')
                .is_some_and(|tail| tail.starts_with(|character: char| character.is_ascii_digit()))
        })
        .count();
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(rows, diagnostics.len());
    for diagnostic in diagnostics {
        let state = format!("S{}", diagnostic["state"]);
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

    fn run(&self, command: &str, extra: &[&str]) -> Output {
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
    assert!(explanation.contains(" = phi(T"));
    assert!(explanation.contains(" = MLOAD %"));
    assert!(explanation.contains("SSTORE %"));
    assert!(explanation.contains("BranchTrue") && explanation.contains("BranchFalse"));
    let report = json(&world(
        "analyze",
        "call-return-branch",
        &["--format", "json"],
    ));
    for (id, outcome) in report["outcomes"].as_array().unwrap().iter().enumerate() {
        let line = explanation
            .lines()
            .find(|line| line.trim_start().starts_with(&format!("O{id} |")))
            .unwrap_or_else(|| panic!("missing separate outcome O{id}"));
        assert!(line.contains(outcome["kind"].as_str().unwrap()));
        assert!(line.contains(&format!("S{}", outcome["state"])));
    }
    for detail in [
        "captured frames:",
        "Active abstract transfers:",
        "machine code identity=",
        "jump history=",
        "frame phis (stack in):",
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
    let output = input.run("explain", &[]);
    success(&output);
    let explanation = text(&output);
    for operation in [
        "%0 = PUSH1 0x9",
        "%1 = PUSH1 0x2",
        "%2 = SUB %1 %0",
        "MSTORE %3 %2",
        "RETURN %5 %4",
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
    assert!(call.contains("commit child effects"));

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
    assert!(proxy.contains("DELEGATECALL") && proxy.contains("SSTORE"));
    assert!(proxy.contains("owner=") || proxy.contains("storage owner="));

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
    let empty = EntryCode::new("0x").run("explain", &[]);
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
    for (name, extra) in [
        ("missing-code", vec![]),
        ("call-return-branch", vec!["--max-work", "1"]),
        ("call-return-branch", vec!["--max-memory-bytes", "1"]),
        ("reentry", vec!["--max-call-depth", "2"]),
        ("call-return-branch", vec!["--max-states", "1"]),
        ("call-return-branch", vec!["--max-transfers", "1"]),
    ] {
        let output = world("explain", name, &extra);
        assert_eq!(output.status.code(), Some(2), "{name} {extra:?}");
        let explanation = text(&output);
        assert!(explanation.contains("Incomplete"));
        assert!(explanation.contains("SSA unavailable") && !explanation.contains(VERIFIED));
        let mut query = extra.clone();
        query.extend(["--format", "json"]);
        let report = world("analyze", name, &query);
        assert_eq!(report.status.code(), Some(2));
        let report: Json = serde_json::from_slice(&report.stdout).unwrap();
        let frontiers = explanation.split_once("\nFrontiers\n").unwrap().1;
        let rows = frontiers
            .lines()
            .filter(|line| {
                line.trim_start().strip_prefix('U').is_some_and(|tail| {
                    tail.starts_with(|character: char| character.is_ascii_digit())
                        && tail.contains(" | from=")
                })
            })
            .count();
        assert_eq!(
            rows,
            report["frontiers"].as_array().unwrap().len(),
            "{name}"
        );
        for (id, frontier) in report["frontiers"].as_array().unwrap().iter().enumerate() {
            let line = frontiers
                .lines()
                .find(|line| line.trim_start().starts_with(&format!("U{id} | from=")))
                .unwrap();
            assert!(line.contains(variant(&frontier["reason"])));
            let source = frontier["from"]
                .as_u64()
                .map_or_else(|| "none".to_owned(), |state| format!("S{state}"));
            assert!(line.contains(&format!("from={source}")));
        }
        assert_diagnostics(&explanation, &report);
    }
}

#[test]
fn converged_teaching_retains_precision_diagnostics() {
    let input = EntryCode::new("0x5a565b00");
    let output = input.run("explain", &[]);
    success(&output);
    let report = json(&input.run("analyze", &["--format", "json"]));
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
    assert!(verbose.contains(text(&full).trim_end()));
    for detail in [
        "State details",
        "Call summaries",
        "captured frames:",
        "machine code identity=",
        "code hash=",
        "jump history=",
        "effect phi(",
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
        assert_eq!(with_ssa["analysis"], report, "{name}");
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
                                            && slot["value"]["Constants"]
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
        let output = run(&["explain", source[0], source[1], "--verbose"]);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}

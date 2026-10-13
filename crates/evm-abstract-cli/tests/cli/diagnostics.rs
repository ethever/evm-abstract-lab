//! Exercise diagnostic evidence through the installed CLI's JSON and text paths.

use super::run_concrete;
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::Output,
    sync::atomic::{AtomicUsize, Ordering},
};

const ROOT: &str = "0x0000000000000000000000000000000000000101";
const CHILD: &str = "0x0000000000000000000000000000000000000200";

fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn invalid_bytes_and_fork_rules_survive_json_and_text_output() {
    for (code, fork, pc, opcode, mnemonic, reason, display) in [
        (
            "fe",
            "osaka",
            0,
            0xfe,
            Some("INVALID"),
            json!("ExplicitInvalid"),
            "0xfe (INVALID)",
        ),
        (
            "ef",
            "osaka",
            0,
            0xef,
            None,
            json!("UndefinedOpcode"),
            "0xef",
        ),
        (
            "5f1e00",
            "cancun",
            1,
            0x1e,
            Some("CLZ"),
            json!({"UnavailableInFork":"cancun"}),
            "0x1e (CLZ)",
        ),
    ] {
        let report: Value = serde_json::from_str(&text(run_concrete(&[
            "cfg",
            "--hex",
            code,
            "--fork",
            fork,
            "--context-depth",
            "0",
            "--format",
            "json",
        ])))
        .unwrap();
        assert_eq!(report["schema_version"], 7);
        assert_eq!(report["status"], "Converged");
        assert_eq!(report["frontiers"], json!([]));
        let diagnostics = report["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0]["state"], 0);
        assert_eq!(diagnostics[0]["pc"], pc);
        let detail = &diagnostics[0]["kind"]["InvalidOpcode"]["Instruction"];
        assert_eq!(detail["opcode"], opcode);
        assert_eq!(detail["mnemonic"], json!(mnemonic));
        assert_eq!(detail["reason"], reason);
        for command in ["cfg", "explain"] {
            let output = text(run_concrete(&[
                command,
                "--hex",
                code,
                "--fork",
                fork,
                "--context-depth",
                "0",
            ]));
            let row = output
                .lines()
                .find(|line| line.starts_with("diagnostic "))
                .unwrap();
            assert!(
                row.contains("InvalidOpcode") && row.contains(display),
                "{row}"
            );
            let explanation = match opcode {
                0xfe => "explicit exceptional-halt instruction",
                0xef => "undefined ordinary-bytecode opcode",
                _ => "instruction unavailable in fork cancun",
            };
            assert!(row.contains(explanation), "{row}");
            if opcode == 0xef {
                assert!(!row.contains("INVALID") && !row.contains("0xfe"), "{row}");
            }
            assert!(output.contains("Records belong to abstract states"));
        }
    }
    for (code, fork) in [("5f1e00", "osaka"), ("00fe", "osaka")] {
        let report: Value = serde_json::from_str(&text(run_concrete(&[
            "cfg", "--hex", code, "--fork", fork, "--format", "json",
        ])))
        .unwrap();
        assert_eq!(report["status"], "Converged");
        assert_eq!(report["diagnostics"], json!([]));
    }
    let disassembly = text(run_concrete(&["disasm", "--hex", "00fe"]));
    assert!(disassembly.contains("INVALID"));
}

struct WorldFile(PathBuf);

impl WorldFile {
    fn new(root: &str, child: &str, additional: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-diagnostics-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let accounts: Vec<_> = [(ROOT, root), (CHILD, child)].into_iter()
            .chain(additional.iter().copied())
            .map(|(address, code)| json!({"address":address, "code":code, "storage":{}, "storage_unknown":false, "balance":"0x1000000", "nonce":"0x1"}))
            .collect();
        fs::write(
            &path,
            serde_json::to_vec(
                &json!({"fork":"osaka", "provenance":"offline:diagnostics", "accounts":accounts}),
            )
            .unwrap(),
        )
        .unwrap();
        Self(path)
    }

    fn run(&self, command: &str, extra: &[&str]) -> String {
        let mut args = vec![
            command,
            "--world",
            self.0.to_str().unwrap(),
            "--evm.to",
            ROOT,
        ];
        args.extend_from_slice(extra);
        text(run_concrete(&args))
    }
}

impl Drop for WorldFile {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn callee_diagnostic_uses_its_opcode_in_all_world_views() {
    // The root has PUSH0 at PC 0; the called program has undefined 0xef there.
    let world = WorldFile::new("5f5f5f5f5f6102006207a120f100", "ef", &[]);
    let report: Value = serde_json::from_str(&world.run("analyze", &["--format", "json"])).unwrap();
    assert_eq!(report["status"], "Converged");
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["pc"], 0);
    assert_eq!(
        diagnostics[0]["kind"]["InvalidOpcode"]["Instruction"]["opcode"],
        0xef
    );
    for (command, extra) in [
        ("analyze", vec![]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let output = world.run(command, &extra);
        let diagnostics = output
            .split_once("\nDiagnostics\n")
            .unwrap()
            .1
            .split_once("\nFrontiers\n")
            .unwrap()
            .0;
        assert!(
            diagnostics.contains("InvalidOpcode · 0xef"),
            "{diagnostics}"
        );
        assert!(diagnostics.contains("undefined ordinary-bytecode opcode"));
        assert!(!diagnostics.contains("0xfe") && !diagnostics.contains("0x5f"));
    }
}

#[test]
fn delegation_entry_text_does_not_claim_an_executed_pc() {
    let world = WorldFile::new(
        "ef01000000000000000000000000000000000000000200",
        "ef01000000000000000000000000000000000000000300",
        &[("0x0000000000000000000000000000000000000300", "fe")],
    );
    for (command, extra) in [
        ("analyze", vec![]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let output = world.run(command, &extra);
        let diagnostics = output
            .split_once("\nDiagnostics\n")
            .unwrap()
            .1
            .split_once("\nFrontiers\n")
            .unwrap()
            .0;
        assert!(
            diagnostics.contains("delegation entry (synthetic PC 0)"),
            "{diagnostics}"
        );
        // The full report table wraps long cells, including within words.
        let compact: String = diagnostics
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '|')
            .collect();
        assert!(
            compact.contains("nesteddelegationindicatorcannotbeexecuted"),
            "{diagnostics}"
        );
        assert!(!diagnostics.contains("0xfe") && !diagnostics.contains("(INVALID)"));
    }
}

//! Human-readable bit constraints preserve every EVM word position.

use super::run;
use alloy_primitives::U256;
use serde_json::{Value as Json, json};
use std::{
    fs,
    path::PathBuf,
    process::Output,
    sync::atomic::{AtomicUsize, Ordering},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";

fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn assert_pattern(rendered: &str, pattern: &str) {
    assert!(
        rendered.contains(pattern),
        "missing full pattern {pattern}:\n{rendered}"
    );
    assert!(!rendered.contains("bits(0="));
    assert!(!rendered.contains('⊤'));
}

struct WorldCode(PathBuf);

impl WorldCode {
    fn new(code: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-abstract-known-bits-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(
            &path,
            serde_json::to_vec(&json!({
                "fork": "osaka",
                "provenance": "synthetic:known-bits-rendering",
                "accounts": [{
                    "address": ENTRY,
                    "code": format!("0x{code}"),
                    "storage_unknown": true,
                    "balance": "0x0",
                    "nonce": "0x0",
                    "existence": "present"
                }]
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
            "--entry",
            ENTRY,
            "--max-constants",
            "1",
        ];
        args.extend_from_slice(extra);
        run(&args)
    }
}

impl Drop for WorldCode {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn clz_cfg_shows_partial_nibbles_without_omitting_unknown_positions() {
    let output = run(&["cfg", "--hex", "5f351e00"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    let pattern = format!("bits=0x{}[000*]**", "0".repeat(61));
    assert!(text.contains(&pattern), "missing {pattern}:\n{text}");
    assert!(!text.contains("bits(0="));
}

#[test]
fn cfg_dot_ssa_and_hex_explain_preserve_full_hex_patterns() {
    // The extra block exposes the value in DOT entry stacks and SSA phis.
    for (code, pattern) in [
        (
            "5f351e6006565b00",
            format!("bits=0x{}[000*]**", "0".repeat(61)),
        ),
        (
            "5f35600716600417600b565b00",
            format!("bits=0x{}[01**]", "0".repeat(63)),
        ),
        (
            "5f356004176008565b00",
            format!("bits=0x{}[*1**]", "*".repeat(63)),
        ),
        ("5f356005565b00", format!("0x{}", "*".repeat(64))),
    ] {
        for (command, extra) in [
            ("cfg", vec![]),
            ("cfg", vec!["--format", "dot"]),
            ("ssa", vec![]),
            ("explain", vec![]),
        ] {
            let mut args = vec![command, "--hex", code, "--max-constants", "1"];
            args.extend_from_slice(&extra);
            assert_pattern(&text(run(&args)), &pattern);
        }
    }
}

#[test]
fn world_reports_teaching_verbose_explain_and_ssa_use_the_same_patterns() {
    // SLOAD supplies an unknown word: world CLI calldata is concrete bytes.
    for (code, pattern) in [
        ("5f541e00", format!("bits=0x{}[000*]**", "0".repeat(61))),
        (
            "5f5460071660041700",
            format!("bits=0x{}[01**]", "0".repeat(63)),
        ),
        ("5f5400", format!("0x{}", "*".repeat(64))),
    ] {
        let world = WorldCode::new(code);
        for (command, extra) in [
            ("analyze", vec![]),
            ("analyze", vec!["--ssa"]),
            ("explain", vec![]),
            ("explain", vec!["--verbose"]),
        ] {
            let rendered = text(world.run(command, &extra));
            assert_pattern(&rendered, &pattern);
            if command == "explain" || extra == ["--ssa"] {
                assert!(rendered.contains("Verified cross-contract SSA:"));
            }
        }
    }
}

fn assert_clz_json(value: &Json) {
    assert_eq!(
        value["known_bits"],
        json!({ "zero": U256::MAX << 9_usize, "one": U256::ZERO })
    );
    assert_eq!(value["known_bits"].as_object().unwrap().len(), 2);
    assert_eq!(value["interval"]["unsigned_lo"], "0x0");
    assert_eq!(value["interval"]["unsigned_hi"], "0x100");
    assert!(value.get("Constants").is_none());
    assert_eq!(value.as_object().unwrap().len(), 5);
}

#[test]
fn json_retains_masks_components_and_top_tags_in_cfg_ssa_and_world() {
    for command in ["cfg", "ssa"] {
        let rendered = text(run(&[command, "--hex", "5f351e00", "--format", "json"]));
        let json: Json = serde_json::from_str(&rendered).unwrap();
        let report = if command == "ssa" {
            &json["analysis"]
        } else {
            &json
        };
        assert_clz_json(&report["states"][0]["exit_stack"][0]);
        assert!(!rendered.contains("bits="));
        assert!(!rendered.contains('*'));
    }
    let world = WorldCode::new("5f541e00");
    for extra in [vec!["--format", "json"], vec!["--format", "json", "--ssa"]] {
        let rendered = text(world.run("analyze", &extra));
        let json: Json = serde_json::from_str(&rendered).unwrap();
        let report = if extra.contains(&"--ssa") {
            &json["analysis"]
        } else {
            &json
        };
        assert_clz_json(&report["states"][0]["exit_stack"][0]);
        assert!(!rendered.contains("bits="));
        assert!(!rendered.contains('*'));
    }
    let top = text(run(&[
        "cfg",
        "--hex",
        "5f3500",
        "--domain",
        "constants-only",
        "--format",
        "json",
    ]));
    let top: Json = serde_json::from_str(&top).unwrap();
    assert_eq!(top["states"][0]["exit_stack"][0], "Top");
}

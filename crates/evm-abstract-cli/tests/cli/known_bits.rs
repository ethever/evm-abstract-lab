//! Human-readable bit constraints preserve every EVM word position.

use super::run_concrete;
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
    if pattern == "⊤" {
        assert_numeric_top(rendered);
    }
}

fn assert_numeric_top(rendered: &str) {
    assert!(
        rendered.contains("[⊤]") || rendered.contains("abstract ⊤"),
        "missing numeric Top:\n{rendered}"
    );
    assert!(!rendered.contains(&format!("0x{}", "*".repeat(64))));
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

    fn run_concrete(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = vec!["--max-constants", "1"];
        args.extend_from_slice(extra);
        self.run_default(command, &args)
    }

    fn run_default(&self, command: &str, extra: &[&str]) -> Output {
        let mut args = vec![
            command,
            "--world",
            self.0.to_str().unwrap(),
            "--evm.to",
            ENTRY,
        ];
        args.extend_from_slice(extra);
        run_concrete(&args)
    }
}

impl Drop for WorldCode {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn clz_cfg_shows_partial_nibbles_without_omitting_unknown_positions() {
    let output = run_concrete(&["cfg", "--hex", "5f351e00"]);
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
        ("5f356005565b00", "⊤".to_owned()),
        ("600a6005565b00", "{0xa}".to_owned()),
    ] {
        for (command, extra) in [
            ("cfg", vec![]),
            ("cfg", vec!["--format", "dot"]),
            ("ssa", vec![]),
            ("explain", vec![]),
        ] {
            let mut args = vec![command, "--hex", code, "--max-constants", "1"];
            args.extend_from_slice(&extra);
            assert_pattern(&text(run_concrete(&args)), &pattern);
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
        ("5f5400", "⊤".to_owned()),
        ("600a00", "{0xa}".to_owned()),
    ] {
        let world = WorldCode::new(code);
        for (command, extra) in [
            ("analyze", vec![]),
            ("analyze", vec!["--ssa"]),
            ("explain", vec![]),
            ("explain", vec!["--verbose"]),
        ] {
            let rendered = text(world.run_concrete(command, &extra));
            assert_pattern(&rendered, &pattern);
            if command == "explain" || extra == ["--ssa"] {
                assert!(rendered.contains("Verified cross-contract SSA:"));
            }
        }
    }
}

#[test]
fn numeric_top_stays_compact_in_all_human_views() {
    assert_numeric_top(&text(run_concrete(&[
        "explain",
        "--hex",
        "5f351e00",
        "--domain",
        "constants-only",
    ])));
    // The 257 CLZ candidates exceed the default capacity of eight.
    for (command, view) in [
        ("cfg", vec![]),
        ("cfg", vec!["--format", "dot"]),
        ("ssa", vec![]),
        ("explain", vec![]),
    ] {
        let mut args = vec![
            command,
            "--hex",
            "5f351e6006565b00",
            "--domain",
            "constants-only",
        ];
        args.extend_from_slice(&view);
        assert_numeric_top(&text(run_concrete(&args)));
    }
    let world = WorldCode::new("5f541e00");
    for (command, view) in [
        ("analyze", vec![]),
        ("analyze", vec!["--ssa"]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let mut args = vec!["--domain", "constants-only"];
        args.extend_from_slice(&view);
        assert_numeric_top(&text(world.run_default(command, &args)));
    }
}

#[test]
fn constrained_values_keep_all_unknown_bit_positions() {
    // Merge two complementary nonzero constants after dropping jump history.
    // Their bit component is Top, but the product retains interval/congruence
    // and nonzero constraints. Its bits= pattern must still show all 64 stars.
    let raw = format!("5f35600a576001602f565b7f{}fe602f565b00", "ff".repeat(31));
    let world_code = raw.replacen("5f35", "5f54", 1);
    let pattern = format!("bits=0x{}", "*".repeat(64));
    for (command, view) in [
        ("cfg", vec![]),
        ("cfg", vec!["--format", "dot"]),
        ("ssa", vec![]),
        ("explain", vec![]),
    ] {
        let mut args = vec![
            command,
            "--hex",
            &raw,
            "--max-constants",
            "1",
            "--context-depth",
            "0",
        ];
        args.extend_from_slice(&view);
        assert_pattern(&text(run_concrete(&args)), &pattern);
    }
    let world = WorldCode::new(&world_code);
    for (command, view) in [
        ("analyze", vec![]),
        ("analyze", vec!["--ssa"]),
        ("explain", vec![]),
        ("explain", vec!["--verbose"]),
    ] {
        let mut args = vec!["--context-depth", "0"];
        args.extend_from_slice(&view);
        assert_pattern(&text(world.run_concrete(command, &args)), &pattern);
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
    // Numeric keys retain their meaning; the independent expression layer
    // records the CLZ relation instead of changing the numeric mask shape.
    assert_eq!(value.as_object().unwrap().len(), 6);
    assert_eq!(value["expression"]["kind"]["Operation"]["opcode"], 0x1e);
}

#[test]
fn json_retains_masks_components_and_top_tags_in_cfg_ssa_and_world() {
    for command in ["cfg", "ssa"] {
        let rendered = text(run_concrete(&[
            command, "--hex", "5f351e00", "--format", "json",
        ]));
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
        let rendered = text(world.run_concrete("analyze", &extra));
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
    let top = text(run_concrete(&[
        "cfg",
        "--hex",
        "5f3500",
        "--domain",
        "constants-only",
        "--format",
        "json",
    ]));
    let top: Json = serde_json::from_str(&top).unwrap();
    let value = &top["states"][0]["exit_stack"][0];
    assert!(value["Constants"].is_null());
    assert_eq!(value["known_bits"]["zero"], "0x0");
    assert_eq!(value["known_bits"]["one"], "0x0");
    assert_eq!(
        value["identity"]["input"]["name"],
        serde_json::json!({"CalldataWord":"0x0"})
    );
    let uncorrelated = text(run_concrete(&[
        "cfg",
        "--hex",
        "5f5400",
        "--domain",
        "constants-only",
        "--format",
        "json",
    ]));
    let uncorrelated: Json = serde_json::from_str(&uncorrelated).unwrap();
    let value = &uncorrelated["states"][0]["exit_stack"][0];
    assert!(value.get("Constants").is_none());
    assert_eq!(value["known_bits"]["zero"], "0x0");
    assert_eq!(value["known_bits"]["one"], "0x0");
    assert_eq!(value["interval"]["unsigned_lo"], "0x0");
    assert_eq!(
        value["interval"]["unsigned_hi"],
        format!("0x{}", "f".repeat(64))
    );
    assert_eq!(value["congruence"], "Top");
    assert_eq!(value["nonzero"], false);
    assert!(
        value.get("expression").is_some(),
        "an opaque runtime read has its own expression"
    );
}

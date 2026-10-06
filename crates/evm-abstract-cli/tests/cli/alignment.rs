//! 通过真实 CLI，把指令列表的列位置逐项对应到解码后的输入程序。

use super::run_concrete;
use evm_abstract::{Fork, bytecode::Program};
use serde_json::Value as Json;
use std::{
    fs,
    path::PathBuf,
    process::Output,
    sync::atomic::{AtomicUsize, Ordering},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";

fn example(path: &str) -> String {
    format!("{}/../../examples/{path}", env!("CARGO_MANIFEST_DIR"))
}

fn text(output: &Output, exit_code: i32) -> String {
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

fn block_header(line: &str) -> bool {
    line.trim_start().strip_prefix('B').is_some_and(|tail| {
        tail.split_once(" @ 0x")
            .is_some_and(|(id, _)| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
    })
}

fn instruction_row(line: &str) -> bool {
    line.trim_start()
        .split_once(": ")
        .is_some_and(|(pc, _)| !pc.is_empty() && pc.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

// 预期块与指令来自输入，不依赖渲染器；逐行核对同时防止 PC 遗漏、重复或标错。
fn assert_instruction_lists(output: &str, programs: &[Program]) {
    let lines: Vec<_> = output.lines().collect();
    let headers: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| block_header(line))
        .collect();
    let blocks: Vec<_> = programs
        .iter()
        .flat_map(|program| program.blocks().iter().map(move |block| (program, block)))
        .collect();
    assert!(!blocks.is_empty(), "test input must contain instructions");
    assert_eq!(headers.len(), blocks.len(), "instruction-list block count");
    assert_eq!(
        lines.iter().filter(|line| instruction_row(line)).count(),
        blocks
            .iter()
            .map(|(_, block)| block.instructions.len())
            .sum::<usize>(),
        "instruction-list row count"
    );
    for ((line_index, header), (program, block)) in headers.into_iter().zip(blocks) {
        assert_eq!(
            *header,
            format!("B{} @ 0x{:04x}:", block.id, block.start_pc),
            "block headers must begin at global column zero"
        );
        let digit_column = header.find("0x").unwrap() + 2;
        for (offset, instruction) in block.instructions.iter().enumerate() {
            let row = lines[line_index + offset + 1];
            let unindented = row.trim_start();
            assert_eq!(
                row.len() - unindented.len(),
                digit_column,
                "instruction PC must start below the header PC digits: {header:?}, {row:?}"
            );
            let (pc, body) = unindented.split_once(": ").unwrap();
            assert_eq!(pc, format!("{:04x}", instruction.pc), "{row:?}");
            let mut expected = instruction.name().to_owned();
            if let Some(value) = instruction.immediate {
                expected.push_str(&format!(" 0x{value:x}"));
            }
            if !instruction.is_valid() {
                expected.push_str(&format!(" [invalid under {}]", program.fork()));
            }
            assert_eq!(
                body.split_whitespace().collect::<Vec<_>>(),
                expected.split_whitespace().collect::<Vec<_>>(),
                "{row:?}"
            );
        }
    }
}

fn program_directory(explanation: &str) -> &str {
    explanation.split_once("\nstatus=").unwrap().0
}

fn world_directory(explanation: &str, verbose: bool) -> &str {
    let next_section = if verbose { "\nAnalysis\n" } else { "\nCFG\n" };
    explanation.split_once(next_section).unwrap().0
}

fn world_programs() -> Vec<Program> {
    let input: Json =
        serde_json::from_slice(&fs::read(example("worlds/call-return-branch.json")).unwrap())
            .unwrap();
    let fork = input["fork"].as_str().unwrap().parse().unwrap();
    let mut accounts: Vec<_> = input["accounts"].as_array().unwrap().iter().collect();
    accounts.sort_unstable_by_key(|account| account["address"].as_str().unwrap());
    accounts
        .into_iter()
        .map(|account| {
            Program::from_hex_with_fork(account["code"].as_str().unwrap(), fork).unwrap()
        })
        .collect()
}

fn world_explain(verbose: bool, extra: &[&str]) -> Output {
    let path = example("worlds/call-return-branch.json");
    let mut args = vec!["explain", "--world", &path, "--evm.to", ENTRY];
    if verbose {
        args.push("--verbose");
    }
    args.extend_from_slice(extra);
    run_concrete(&args)
}

struct TemporaryHex(PathBuf);

impl TemporaryHex {
    fn new(code: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-abstract-alignment-{}-{}.hex",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::write(&path, code).unwrap();
        Self(path)
    }
}

impl Drop for TemporaryHex {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn disasm_aligns_every_instruction_with_its_block_pc() {
    let code = "60016002015b00";
    let output = text(&run_concrete(&["disasm", "--hex", code]), 0);
    assert_instruction_lists(&output, &[Program::from_hex(code).unwrap()]);
}

#[test]
fn disasm_alignment_follows_b9_b10_and_b100_header_widths() {
    let code = format!("00{}", "5b00".repeat(100));
    let program = Program::from_hex(&code).unwrap();
    assert_eq!(program.blocks().len(), 101);
    for id in [9, 10, 100] {
        assert_eq!(program.blocks()[id].id, id);
    }
    let output = text(&run_concrete(&["disasm", "--hex", &code]), 0);
    assert_instruction_lists(&output, &[program]);
}

#[test]
fn disasm_keeps_five_digit_pcs_aligned_with_a_short_block_header() {
    // 只做静态反汇编，不执行这个刻意超过栈容量的 PUSH0 程序。
    let code = format!("{}00", "5f".repeat(0x1_0002));
    let file = TemporaryHex::new(&code);
    let program = Program::from_hex(&code).unwrap();
    assert_eq!(program.blocks().len(), 1);
    assert!(program.blocks()[0].instructions.last().unwrap().pc > 0xffff);
    let output = text(
        &run_concrete(&["disasm", "--file", file.0.to_str().unwrap()]),
        0,
    );
    assert_instruction_lists(&output, &[program]);
}

#[test]
fn explain_file_aligns_known_bits_branch_at_context_depth_zero() {
    let path = example("known-bits-branch.hex");
    let code = fs::read_to_string(&path).unwrap();
    let output = text(
        &run_concrete(&["explain", "--file", &path, "--context-depth", "0"]),
        0,
    );
    assert!(output.contains("context_depth=0"));
    assert_instruction_lists(
        program_directory(&output),
        &[Program::from_hex(&code).unwrap()],
    );
}

#[test]
fn explain_hex_aligns_its_disassembly_directory() {
    let code = "60016002015b00";
    let output = text(&run_concrete(&["explain", "--hex", code]), 0);
    assert_instruction_lists(
        program_directory(&output),
        &[Program::from_hex(code).unwrap()],
    );
}

#[test]
fn incomplete_explain_keeps_aligned_instruction_lists_and_exit_two() {
    let code = "6003565b00";
    let output = text(
        &run_concrete(&["explain", "--hex", code, "--max-states", "1"]),
        2,
    );
    assert!(output.contains("status=Incomplete") && output.contains("SSA unavailable"));
    assert_instruction_lists(
        program_directory(&output),
        &[Program::from_hex(code).unwrap()],
    );
}

#[test]
fn world_default_and_verbose_keep_block_headers_at_global_column_zero() {
    let programs = world_programs();
    for verbose in [false, true] {
        let output = text(&world_explain(verbose, &[]), 0);
        assert!(output.contains("Captured instruction list"));
        assert_instruction_lists(world_directory(&output, verbose), &programs);
    }
}

#[test]
fn world_initial_budget_fallback_aligns_observed_code_in_both_views() {
    let programs = world_programs();
    let report = text(
        &run_concrete(&[
            "analyze",
            "--world",
            &example("worlds/call-return-branch.json"),
            "--evm.to",
            ENTRY,
            "--max-work",
            "1",
            "--format",
            "json",
        ]),
        2,
    );
    let report: Json = serde_json::from_str(&report).unwrap();
    assert!(report["states"].as_array().unwrap().is_empty());
    for verbose in [false, true] {
        let output = text(&world_explain(verbose, &["--max-work", "1"]), 2);
        assert!(output.contains("Input code observations (not execution evidence)"));
        assert!(output.contains("Observed instruction list"));
        assert!(!output.contains("Captured instruction list"));
        assert!(output.contains("Incomplete") && output.contains("SSA unavailable"));
        assert_instruction_lists(world_directory(&output, verbose), &programs);
    }
}

#[test]
fn fork_invalid_opcode_annotation_survives_instruction_alignment() {
    let code = "60011e00";
    for command in ["disasm", "explain"] {
        let output = text(
            &run_concrete(&[command, "--hex", code, "--fork", "cancun"]),
            0,
        );
        let directory = if command == "explain" {
            assert!(
                output
                    .lines()
                    .any(|line| line == "diagnostic S0 @ 0x0002: InvalidOpcode")
            );
            program_directory(&output)
        } else {
            &output
        };
        assert_instruction_lists(
            directory,
            &[Program::from_hex_with_fork(code, Fork::Cancun).unwrap()],
        );
    }
}

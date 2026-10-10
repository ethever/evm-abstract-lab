//! 真实 CLI 的四类指令列表共享布局，正文与输入程序及 JSON IR 逐条对应。

use evm_abstract_notation::{Symbol, normalize_subscripts};

use super::run_concrete;
use alloy_primitives::U256;
use evm_abstract::bytecode::Program;
use serde_json::{Value as Json, json};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

const ENTRY: &str = "0x0000000000000000000000000000000000000101";
const CALLEE: &str = "0x0000000000000000000000000000000000000200";
const VERIFIED: &str = "Verified cross-contract SSA:";

fn example(path: &str) -> String {
    format!("{}/../../examples/{path}", env!("CARGO_MANIFEST_DIR"))
}

fn stdout(args: &[&str], exit_code: i32) -> String {
    let output = run_concrete(args);
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn json(args: &[&str]) -> Json {
    serde_json::from_str(&stdout(args, 0)).unwrap()
}

fn notation_prefix(prefix: char) -> &'static str {
    match prefix {
        'S' => "σ",
        'F' => "f",
        '!' => "μ",
        'B' => "B",
        'T' => "T",
        'C' => "C",
        _ => panic!("unsupported entity prefix {prefix}"),
    }
}

fn named_index(line: &str, prefix: char) -> Option<usize> {
    let normalized = normalize_subscripts(line);
    let tail = normalized
        .trim_start()
        .strip_prefix(notation_prefix(prefix))?;
    let tail = tail.strip_prefix('ᵖ').unwrap_or(tail);
    let length = tail.bytes().take_while(u8::is_ascii_digit).count();
    (length > 0).then(|| tail[..length].parse().unwrap())
}

// 只解析状态与指令的界线；块身份和正文预期均取自独立的输入/IR。
fn state_sections(text: &str) -> Vec<(usize, Vec<&str>)> {
    let mut sections = Vec::<(usize, Vec<&str>)>::new();
    for line in text.lines() {
        if let Some(state) = named_index(line, 'S') {
            sections.push((state, vec![line]));
        } else if let Some((_, lines)) = sections.last_mut() {
            lines.push(line);
        }
    }
    sections
}

fn instruction_rows<'a>(lines: &[&'a str]) -> Vec<(&'a str, &'a str, &'a str)> {
    lines
        .iter()
        .filter_map(|line| {
            let (pc, body) = line.trim_start().split_once(": ")?;
            (pc.len() >= 4
                && pc.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !body.starts_with('μ'))
            .then_some((*line, pc, body))
        })
        .collect()
}

fn indices(value: &str, prefix: char) -> Vec<u64> {
    normalize_subscripts(value)
        .split(if prefix == '%' {
            "%"
        } else {
            notation_prefix(prefix)
        })
        .skip(1)
        .filter_map(|tail| {
            let tail = tail.strip_prefix('ᵖ').unwrap_or(tail);
            let length = tail.bytes().take_while(u8::is_ascii_digit).count();
            (length > 0).then(|| tail[..length].parse().unwrap())
        })
        .collect()
}

fn json_indices(value: &Json) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect()
}

fn word(value: &str) -> U256 {
    U256::from_str_radix(value.strip_prefix("0x").unwrap(), 16).unwrap()
}

fn opcode_name(opcode: u64) -> &'static str {
    Program::from_hex(&format!("{opcode:02x}"))
        .unwrap()
        .blocks()[0]
        .instructions[0]
        .name()
}

// TAC 与完整字段采用不同语法，二者都还原成 IR 的数值字段来比较。
fn assert_instruction(body: &str, instruction: &Json, detailed: bool, aliases: bool) {
    let opcode = instruction["opcode"].as_u64().unwrap();
    if detailed {
        let mut fields = body.split(" | ");
        assert_eq!(fields.next().unwrap(), opcode_name(opcode));
        let fields: BTreeMap<_, _> = fields.map(|field| field.split_once('=').unwrap()).collect();
        assert_eq!(
            fields.len(),
            5,
            "all instruction evidence must remain visible"
        );
        assert_eq!(
            u64::from_str_radix(fields["opcode"].strip_prefix("0x").unwrap(), 16).unwrap(),
            opcode
        );
        match instruction["immediate"].as_str() {
            Some(immediate) => assert_eq!(word(fields["immediate"]), word(immediate)),
            None => assert_eq!(fields["immediate"], "none"),
        }
        for field in ["operands", "results"] {
            assert_eq!(
                indices(fields[field], '%'),
                json_indices(&instruction[field])
            );
        }
        assert_eq!(
            fields["fault"].parse::<bool>().unwrap(),
            instruction["fault"]
        );
    } else {
        let (operation, comment) = body.split_once(" ; ").unwrap_or((body, ""));
        let (results, operation) = operation.split_once(" = ").unwrap_or(("", operation));
        assert_eq!(indices(results, '%'), json_indices(&instruction["results"]));
        let mut fields = operation.split_whitespace();
        assert_eq!(fields.next().unwrap(), opcode_name(opcode));
        if let Some(immediate) = instruction["immediate"].as_str() {
            assert_eq!(word(fields.next().unwrap()), word(immediate));
        }
        let operands: Vec<_> = fields
            .map(|field| field.strip_prefix('%').unwrap().parse::<u64>().unwrap())
            .collect();
        assert_eq!(operands, json_indices(&instruction["operands"]));
        assert_eq!(comment.contains("exceptional halt"), instruction["fault"]);
        if aliases && instruction["fault"] == false {
            if (0x80..=0x8f).contains(&opcode) {
                assert_eq!(comment, "duplicate alias");
            } else if (0x90..=0x9f).contains(&opcode) {
                assert_eq!(comment, "aliases reordered");
            }
        }
    }
}

fn assert_block_rows(lines: &[&str], block: &Json, source: &Json, detailed: bool, aliases: bool) {
    let header = format!(
        "{} @ 0x{:04x}:",
        Symbol::Block((source["id"].as_u64().unwrap()) as usize),
        source["start_pc"].as_u64().unwrap()
    );
    let headers: Vec<_> = lines
        .iter()
        .filter(|line| named_index(line, 'B').is_some())
        .copied()
        .collect();
    assert_eq!(headers, vec![header.as_str()], "basic block identity");
    let digit_column = header[..header.find("0x").unwrap()].chars().count() + 2;
    let rows = instruction_rows(lines);
    let instructions = block["instructions"].as_array().unwrap();
    assert_eq!(rows.len(), instructions.len(), "{header}: row count");
    for ((row, pc, body), instruction) in rows.iter().zip(instructions) {
        assert_eq!(
            row.len() - row.trim_start().len(),
            digit_column,
            "PC digits must align with {header}: {row}"
        );
        assert_eq!(
            *pc,
            format!("{:04x}", instruction["pc"].as_u64().unwrap()),
            "{row}"
        );
        let decoded = source["instructions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|decoded| decoded["pc"] == instruction["pc"])
            .unwrap();
        assert_eq!(decoded["opcode"], instruction["opcode"]);
        assert_eq!(decoded["immediate"], instruction["immediate"]);
        assert_instruction(body, instruction, detailed, aliases);
    }
}

fn assert_single_listing(text: &str, report: &Json) {
    let sections = state_sections(text);
    let blocks = report["ssa"]["blocks"].as_array().unwrap();
    assert_eq!(sections.len(), blocks.len());
    for ((state, lines), block) in sections.iter().zip(blocks) {
        assert_eq!(Some(*state as u64), block["state"].as_u64());
        let key = &report["analysis"]["states"][*state]["key"];
        let source = &report["analysis"]["program"]["blocks"]
            [key["basic_block_index"].as_u64().unwrap() as usize];
        assert!(lines[0].contains("context="));
        let context: Json = serde_json::from_str(
            lines[0]
                .split_once("context=")
                .unwrap()
                .1
                .trim_end_matches(':'),
        )
        .unwrap();
        assert_eq!(context, key["context"]);
        assert!(
            !lines[0].contains("@ 0x"),
            "state metadata must be separate"
        );
        assert_block_rows(lines, block, source, false, false);
        for phi in block["phis"].as_array().unwrap() {
            let line = lines
                .iter()
                .find(|line| {
                    line.trim_start()
                        .starts_with(&format!("%{} = φ(", phi["result"]))
                })
                .unwrap();
            assert_eq!(
                indices(line, 'S'),
                phi["inputs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|input| input["predecessor"].as_u64().unwrap())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                indices(line, '%'),
                std::iter::once(phi["result"].as_u64().unwrap())
                    .chain(
                        phi["inputs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|input| input["value"].as_u64().unwrap())
                    )
                    .collect::<Vec<_>>()
            );
            let slot = phi["slot"].as_u64().unwrap() as usize;
            let comment = line
                .split_once(&format!("; slot {slot}, abstract "))
                .unwrap()
                .1;
            assert!(
                !comment.is_empty(),
                "abstract stack-in annotation must remain visible"
            );
        }
        let exit = lines
            .iter()
            .find(|line| line.trim_start().starts_with("stack out ["))
            .unwrap();
        assert_eq!(indices(exit, '%'), json_indices(&block["exit_stack"]));
    }
}

fn world(command: &str, path: &str, entry: &str, extra: &[&str], exit_code: i32) -> String {
    let mut args = vec![command, "--world", path, "--evm.to", entry];
    args.extend_from_slice(extra);
    stdout(&args, exit_code)
}

fn world_report(path: &str, entry: &str, extra: &[&str]) -> Json {
    let mut options = vec!["--ssa", "--format", "json"];
    options.extend_from_slice(extra);
    serde_json::from_str(&world("analyze", path, entry, &options, 0)).unwrap()
}

fn active_frame(state: &Json) -> &Json {
    let stack = &state["entry"]["call_stack"];
    stack["children"]
        .as_array()
        .unwrap()
        .last()
        .map_or(&stack["root"]["state"], |child| &child["state"])
}

fn resolve_address<'a>(output: &'a str, reference: &'a str) -> &'a str {
    if reference.starts_with("0x") {
        reference
    } else {
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{reference} = ")))
            .unwrap()
    }
}

fn assert_frame_metadata(output: &str, lines: &[&str], state: &Json, detailed: bool) {
    let frame = active_frame(state);
    let key = &frame["key"];
    let depth = state["key"]["frames"].as_array().unwrap().len();
    if detailed {
        assert!(lines[0].contains(&format!(
            "machine code identity={}",
            state["key"]["code_identity"].as_str().unwrap()
        )));
        let active = lines
            .iter()
            .position(|line| {
                line.trim_start()
                    .starts_with(&format!("{} active |", Symbol::Frame(depth - 1)))
            })
            .unwrap();
        let identity: BTreeMap<_, _> = lines[active + 1]
            .trim()
            .split(" | ")
            .map(|field| field.split_once('=').unwrap())
            .collect();
        for (label, field) in [
            ("code address", "code_address"),
            ("storage owner", "address"),
            ("code hash", "code_hash"),
        ] {
            assert_eq!(identity[label], key[field].as_str().unwrap());
        }
        let fields: BTreeMap<_, _> = lines[active + 2]
            .trim()
            .split(" | ")
            .map(|field| field.split_once('=').unwrap())
            .collect();
        assert_eq!(
            fields["caller"],
            key["caller"]["Concrete"].as_str().unwrap()
        );
        assert_eq!(fields["static"].parse::<bool>().unwrap(), key["is_static"]);
        assert_eq!(
            serde_json::from_str::<Json>(fields["jump history"]).unwrap(),
            key["jump_history"]
        );
    } else {
        let metadata = lines[0];
        let code = indices(metadata, 'C');
        assert_eq!(code.len(), 1);
        let reference = output
            .lines()
            .find(|line| {
                line.trim_start()
                    .starts_with(&format!("{} |", Symbol::Code(code[0] as usize)))
            })
            .unwrap();
        let fields: BTreeMap<_, _> = reference
            .split(" | ")
            .skip(1)
            .map(|field| field.split_once('=').unwrap())
            .collect();
        assert_eq!(
            resolve_address(output, fields["code"]),
            key["code_address"].as_str().unwrap()
        );
        assert_eq!(fields["code_hash"], key["code_hash"].as_str().unwrap());
        let fields: BTreeMap<_, _> = metadata
            .split(" | ")
            .filter_map(|field| field.split_once('='))
            .collect();
        assert_eq!(
            resolve_address(output, fields["state owner"]),
            key["address"].as_str().unwrap()
        );
        assert_eq!(
            serde_json::from_str::<Json>(fields["context"].trim_end_matches(':')).unwrap(),
            key["jump_history"]
        );
    }
}

fn assert_world_listing(output: &str, report: &Json, detailed: bool) {
    let listing = output.split_once(VERIFIED).unwrap().1;
    let sections = state_sections(listing.split_once("\nTransitions\n").unwrap().0);
    let blocks = report["ssa"]["blocks"].as_array().unwrap();
    assert_eq!(sections.len(), blocks.len());
    for ((state_id, lines), block) in sections.iter().zip(blocks) {
        assert_eq!(Some(*state_id as u64), block["state"].as_u64());
        assert!(
            !lines[0].contains("@ 0x"),
            "state identity belongs on its own line"
        );
        let state = &report["analysis"]["states"][*state_id];
        assert_frame_metadata(output, lines, state, detailed);
        let frame = active_frame(state);
        let key = &frame["key"];
        let source = frame["program"]["blocks"]
            .as_array()
            .and_then(|blocks| blocks.get(key["basic_block_index"].as_u64().unwrap() as usize));
        if let Some(source) = source {
            assert_block_rows(lines, block, source, detailed, !detailed);
        } else {
            assert!(block["instructions"].as_array().unwrap().is_empty());
            assert!(
                lines.iter().all(|line| named_index(line, 'B').is_none()),
                "non-bytecode state must not acquire a block PC"
            );
            assert!(instruction_rows(lines).is_empty());
        }
        let depth = state["key"]["frames"].as_array().unwrap().len();
        assert!(lines.iter().any(|line| {
            line.contains(&format!("{} active", Symbol::Frame(depth - 1)))
                || line.contains(&format!("active={}", Symbol::Frame(depth - 1)))
        }));
        for phi in block["phis"].as_array().unwrap() {
            let line = lines
                .iter()
                .find(|line| {
                    line.trim_start()
                        .starts_with(&format!("%{} = ", phi["result"]))
                        && line.contains("φ(")
                })
                .unwrap();
            let inputs = phi["inputs"].as_array().unwrap();
            assert_eq!(
                indices(line, 'T'),
                inputs
                    .iter()
                    .map(|input| input[0].as_u64().unwrap())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                indices(line, '%'),
                std::iter::once(phi["result"].as_u64().unwrap())
                    .chain(inputs.iter().map(|input| input[1].as_u64().unwrap()))
                    .collect::<Vec<_>>()
            );
            assert_eq!(indices(line, 'F'), vec![phi["frame"].as_u64().unwrap()]);
        }
        if detailed {
            let effect = lines
                .iter()
                .find(|line| {
                    line.trim_start().starts_with(&format!(
                        "{} = effect φ(",
                        Symbol::Effect((block["effect"]["result"]).as_u64().unwrap() as usize)
                    ))
                })
                .unwrap();
            assert_eq!(
                indices(effect, 'T'),
                block["effect"]["inputs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|input| input["transition"].as_u64().unwrap())
                    .collect::<Vec<_>>()
            );
            assert_eq!(
                indices(effect, '!'),
                std::iter::once(block["effect"]["result"].as_u64().unwrap())
                    .chain(
                        block["effect"]["inputs"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|input| input["effect"].as_u64().unwrap())
                    )
                    .collect::<Vec<_>>()
            );
            let effects = lines
                .iter()
                .position(|line| line.trim() == "instruction effects:")
                .unwrap();
            let actual: Vec<_> = lines[effects + 1..]
                .iter()
                .filter(|line| line.contains(" → μ"))
                .collect();
            let expected = block["effects"].as_array().unwrap();
            assert_eq!(actual.len(), expected.len());
            for (line, effect) in actual.into_iter().zip(expected) {
                assert_eq!(
                    indices(line, '!'),
                    vec![effect[1].as_u64().unwrap(), effect[2].as_u64().unwrap()]
                );
                assert!(line.contains(&format!("{:04x}:", effect[0].as_u64().unwrap())));
            }
            assert!(lines.iter().any(|line| line.trim()
                == format!(
                    "exit effect: {}",
                    Symbol::Effect((block["exit_effect"]).as_u64().unwrap() as usize)
                )));
        }
        let out = lines
            .iter()
            .position(|line| line.trim_start().starts_with("stack out (before dispatch)"))
            .unwrap();
        let frames = block["exit_frames"].as_array().unwrap();
        let stacks = if detailed {
            lines[out + 1..out + 1 + frames.len()].join("; ")
        } else {
            lines[out].to_string()
        };
        assert_eq!(
            indices(&stacks, 'F'),
            (0..frames.len() as u64).collect::<Vec<_>>()
        );
        assert_eq!(
            indices(&stacks, '%'),
            frames.iter().flat_map(json_indices).collect::<Vec<_>>()
        );
    }
    let transitions = listing.split_once("\nTransitions\n").unwrap().1;
    let rows: Vec<_> = transitions
        .lines()
        .filter(|line| named_index(line, 'T').is_some())
        .collect();
    let expected = report["ssa"]["transitions"].as_array().unwrap();
    assert_eq!(rows.len(), expected.len());
    for (index, (line, transition)) in rows.iter().zip(expected).enumerate() {
        assert_eq!(named_index(line, 'T'), Some(index));
        let edge = &report["analysis"]["edges"][transition["edge"].as_u64().unwrap() as usize];
        assert_eq!(
            indices(line, 'S'),
            vec![edge["from"].as_u64().unwrap(), edge["to"].as_u64().unwrap()]
        );
        let kind = transition["kind"]
            .as_str()
            .or_else(|| {
                transition["kind"]
                    .as_object()
                    .unwrap()
                    .values()
                    .next()
                    .unwrap()
                    .as_str()
            })
            .unwrap();
        assert!(line.contains(kind));
    }
}

struct TemporaryWorld(PathBuf);

impl TemporaryWorld {
    fn new(accounts: &[(&str, &str)]) -> Self {
        Self::with_fork(accounts, "osaka")
    }

    fn with_fork(accounts: &[(&str, &str)], fork: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "evm-abstract-shared-listing-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let accounts: Vec<_> = accounts.iter().map(|(address, code)| json!({"address":address,"code":code,"balance":"0x64","nonce":"0x0","existence":"present","storage":{},"storage_unknown":false})).collect();
        fs::write(
            &path,
            serde_json::to_vec(
                &json!({"fork":fork,"provenance":"shared-listing-regression","accounts":accounts}),
            )
            .unwrap(),
        )
        .unwrap();
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().unwrap()
    }
}

impl Drop for TemporaryWorld {
    fn drop(&mut self) {
        fs::remove_file(&self.0).unwrap();
    }
}

fn assert_raw_directory(output: &str, programs: &[Program]) {
    let lines: Vec<_> = output.lines().collect();
    let headers: Vec<_> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| named_index(line, 'B').is_some())
        .collect();
    let blocks: Vec<_> = programs
        .iter()
        .flat_map(|program| program.blocks().iter().map(move |block| (program, block)))
        .collect();
    assert_eq!(headers.len(), blocks.len());
    for ((start, header), (program, block)) in headers.iter().zip(blocks) {
        assert_eq!(
            **header,
            format!("{} @ 0x{:04x}:", Symbol::Block(block.id), block.start_pc)
        );
        for (offset, instruction) in block.instructions.iter().enumerate() {
            let row = lines[start + offset + 1];
            assert_eq!(
                row.len() - row.trim_start().len(),
                header[..header.find("0x").unwrap()].chars().count() + 2
            );
            let (pc, body) = row.trim_start().split_once(": ").unwrap();
            assert_eq!(pc, format!("{:04x}", instruction.pc));
            let mut fields = body.split_whitespace();
            assert_eq!(fields.next().unwrap(), instruction.name());
            if let Some(immediate) = instruction.immediate {
                assert_eq!(word(fields.next().unwrap()), immediate);
            }
            let annotation = fields.collect::<Vec<_>>().join(" ");
            assert_eq!(
                annotation,
                if instruction.is_valid() {
                    String::new()
                } else {
                    format!("[invalid under {}]", program.fork())
                }
            );
        }
    }
}

#[test]
fn single_ssa_and_explain_files_use_basic_block_headers_and_aligned_pcs() {
    for name in ["dynamic-jump.hex", "known-bits-branch.hex"] {
        let path = example(name);
        let code = fs::read_to_string(&path).unwrap();
        let input = Program::from_hex(&code).unwrap();
        let report = json(&["ssa", "--file", &path, "--format", "json"]);
        assert_eq!(
            serde_json::to_value(&input).unwrap(),
            report["analysis"]["program"]
        );
        for command in ["ssa", "explain"] {
            let output = stdout(&[command, "--file", &path], 0);
            let listing = if command == "explain" {
                output.split_once("stack SSA:").unwrap().1
            } else {
                &output
            };
            assert_single_listing(listing, &report);
        }
    }
}

#[test]
fn ssa_repeated_contexts_keep_the_same_program_block_identity() {
    let path = example("internal-calls.hex");
    let report = json(&["ssa", "--file", &path, "--format", "json"]);
    let states = report["analysis"]["states"].as_array().unwrap();
    assert!(
        states
            .iter()
            .enumerate()
            .any(|(index, state)| states[..index]
                .iter()
                .any(|prior| prior["key"]["basic_block_index"]
                    == state["key"]["basic_block_index"]
                    && prior["key"]["context"] != state["key"]["context"]))
    );
    assert_single_listing(&stdout(&["ssa", "--file", &path], 0), &report);
    let code = fs::read_to_string(path).unwrap();
    let input = TemporaryWorld::new(&[(ENTRY, &code)]);
    let report = world_report(input.path(), ENTRY, &[]);
    let states = report["analysis"]["states"].as_array().unwrap();
    assert!(states.iter().enumerate().any(|(index, state)| {
        let key = &active_frame(state)["key"];
        states[..index].iter().any(|prior| {
            let prior = &active_frame(prior)["key"];
            prior["basic_block_index"] == key["basic_block_index"]
                && prior["jump_history"] != key["jump_history"]
        })
    }));
    for (command, extra, detailed) in [
        ("explain", vec![], false),
        ("explain", vec!["--verbose"], true),
        ("analyze", vec!["--ssa"], true),
    ] {
        assert_world_listing(
            &world(command, input.path(), ENTRY, &extra, 0),
            &report,
            detailed,
        );
    }
}

#[test]
fn all_ssa_entrypoints_adjust_numeric_columns_for_b9_b10_and_b100() {
    // 每个 JUMPDEST 可达，块编号增长无需大型栈或额外预算。
    let code = format!("{}00", "5b".repeat(101));
    let program = Program::from_hex(&code).unwrap();
    assert_eq!(program.blocks().len(), 101);
    let report = json(&["ssa", "--hex", &code, "--format", "json"]);
    assert_single_listing(&stdout(&["ssa", "--hex", &code], 0), &report);
    let explanation = stdout(&["explain", "--hex", &code], 0);
    assert_single_listing(explanation.split_once("stack SSA:").unwrap().1, &report);
    assert_raw_directory(
        explanation.split_once("\nstatus=").unwrap().0,
        std::slice::from_ref(&program),
    );
    let input = TemporaryWorld::new(&[(ENTRY, &code)]);
    let report = world_report(input.path(), ENTRY, &[]);
    for (command, extra, detailed) in [
        ("explain", vec![], false),
        ("explain", vec!["--verbose"], true),
        ("analyze", vec!["--ssa"], true),
    ] {
        let output = world(command, input.path(), ENTRY, &extra, 0);
        assert_world_listing(&output, &report, detailed);
        if command == "explain" {
            assert_raw_directory(
                output
                    .split_once(if detailed { "\nAnalysis\n" } else { "\nCFG\n" })
                    .unwrap()
                    .0,
                std::slice::from_ref(&program),
            );
        }
    }
}

#[test]
fn world_explain_and_analyze_ssa_preserve_calls_returns_and_complete_ir() {
    for name in ["call-return-branch", "proxy-storage"] {
        let path = example(&format!("worlds/{name}.json"));
        let report = world_report(&path, ENTRY, &[]);
        let transitions = report["ssa"]["transitions"].as_array().unwrap();
        assert!(
            transitions
                .iter()
                .any(|transition| transition["kind"] == "Call")
        );
        assert!(
            transitions
                .iter()
                .any(|transition| transition["kind"] == "Return")
        );
        for (command, extra, detailed) in [
            ("explain", vec![], false),
            ("explain", vec!["--verbose"], true),
            ("analyze", vec!["--ssa"], true),
        ] {
            let output = world(command, &path, ENTRY, &extra, 0);
            assert_world_listing(&output, &report, detailed);
        }
    }
}

#[test]
fn world_parallel_edges_and_loop_phis_keep_every_transition_input() {
    for code in ["602a5a6006575b00", "60005b600101600256"] {
        let input = TemporaryWorld::new(&[(ENTRY, code)]);
        let report = world_report(input.path(), ENTRY, &["--context-depth", "0"]);
        let phis: Vec<_> = report["ssa"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|block| block["phis"].as_array().unwrap())
            .collect();
        assert!(
            phis.iter()
                .any(|phi| phi["inputs"].as_array().unwrap().len() > 1)
        );
        if code.contains("5a") {
            let edges = report["analysis"]["edges"].as_array().unwrap();
            assert!(edges.iter().enumerate().any(|(index, edge)| {
                edges[..index]
                    .iter()
                    .any(|prior| prior["from"] == edge["from"] && prior["to"] == edge["to"])
            }));
        }
        for extra in [
            vec!["--context-depth", "0"],
            vec!["--context-depth", "0", "--verbose"],
        ] {
            let detailed = extra.contains(&"--verbose");
            assert_world_listing(
                &world("explain", input.path(), ENTRY, &extra, 0),
                &report,
                detailed,
            );
        }
    }
}

#[test]
fn alias_fault_and_fork_annotations_survive_shared_instruction_rows() {
    for (code, extra) in [
        ("6001809000", vec![]),
        ("01", vec![]),
        ("60011e00", vec!["--fork", "cancun"]),
    ] {
        let mut args = vec!["ssa", "--hex", code, "--format", "json"];
        args.extend_from_slice(&extra);
        let report = json(&args);
        args = vec!["ssa", "--hex", code];
        args.extend_from_slice(&extra);
        assert_single_listing(&stdout(&args, 0), &report);
        args = vec!["explain", "--hex", code];
        args.extend_from_slice(&extra);
        let output = stdout(&args, 0);
        assert_single_listing(output.split_once("stack SSA:").unwrap().1, &report);
        let fork = if extra.is_empty() {
            evm_abstract::Fork::Osaka
        } else {
            evm_abstract::Fork::Cancun
        };
        assert_raw_directory(
            output.split_once("\nstatus=").unwrap().0,
            &[Program::from_hex_with_fork(code, fork).unwrap()],
        );
        let input = TemporaryWorld::with_fork(
            &[(ENTRY, code)],
            if extra.is_empty() { "osaka" } else { "cancun" },
        );
        let report = world_report(input.path(), ENTRY, &[]);
        for detailed in [false, true] {
            let mut options = vec![];
            if detailed {
                options.push("--verbose");
            }
            assert_world_listing(
                &world("explain", input.path(), ENTRY, &options, 0),
                &report,
                detailed,
            );
        }
    }
}

#[test]
fn non_bytecode_and_synthetic_states_never_acquire_basic_block_pcs() {
    let empty = TemporaryWorld::new(&[(ENTRY, "0x")]);
    let native = TemporaryWorld::new(&[]);
    let delegated = TemporaryWorld::new(&[
        (ENTRY, "ef01000000000000000000000000000000000000000200"),
        (CALLEE, "ef01000000000000000000000000000000000000000101"),
    ]);
    let synthetic = TemporaryWorld::new(&[(ENTRY, "5f5f5f5f5f61020061fffff1"), (CALLEE, "00")]);
    for (input, entry, reason) in [
        (&empty, ENTRY, "empty executable code; implicit completion"),
        (
            &native,
            "0x0000000000000000000000000000000000000004",
            "native execution; no bytecode instructions",
        ),
        (
            &delegated,
            ENTRY,
            "invalid nested delegation; exceptional halt",
        ),
        (&synthetic, ENTRY, "synthetic end-of-code continuation"),
    ] {
        let report = world_report(input.path(), entry, &[]);
        assert!(
            report["analysis"]["states"]
                .as_array()
                .unwrap()
                .iter()
                .any(|state| {
                    let frame = active_frame(state);
                    frame["program"]["blocks"].as_array().is_none_or(|blocks| {
                        blocks
                            .get(frame["key"]["basic_block_index"].as_u64().unwrap() as usize)
                            .is_none()
                    })
                })
        );
        for (command, extra, detailed) in [
            ("explain", vec![], false),
            ("explain", vec!["--verbose"], true),
            ("analyze", vec!["--ssa"], true),
        ] {
            let output = world(command, input.path(), entry, &extra, 0);
            assert_world_listing(&output, &report, detailed);
            assert!(output.split_once(VERIFIED).unwrap().1.contains(reason));
        }
    }
}

#[test]
fn incomplete_explanations_keep_aligned_input_and_exit_two_without_partial_ssa() {
    let code = "6003565b00";
    let output = stdout(&["explain", "--hex", code, "--max-states", "1"], 2);
    assert_raw_directory(
        output.split_once("\nstatus=").unwrap().0,
        &[Program::from_hex(code).unwrap()],
    );
    assert!(output.contains("status=Incomplete") && output.contains("SSA unavailable"));
    assert!(!output.contains("stack SSA:"));
    let path = example("worlds/call-return-branch.json");
    let input: Json = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let programs: Vec<_> = input["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|account| Program::from_hex(account["code"].as_str().unwrap()).unwrap())
        .collect();
    let report: Json = serde_json::from_str(&world(
        "analyze",
        &path,
        ENTRY,
        &["--ssa", "--format", "json", "--max-work", "1"],
        2,
    ))
    .unwrap();
    assert_eq!(report["status"], "Incomplete");
    assert!(report["states"].as_array().unwrap().is_empty());
    assert!(report.get("ssa").is_none());
    for detailed in [false, true] {
        let mut extra = vec!["--max-work", "1"];
        if detailed {
            extra.push("--verbose");
        }
        let output = world("explain", &path, ENTRY, &extra, 2);
        assert_raw_directory(
            output
                .split_once(if detailed { "\nAnalysis\n" } else { "\nCFG\n" })
                .unwrap()
                .0,
            &programs,
        );
        assert!(output.contains("Incomplete") && output.contains("SSA unavailable"));
        assert!(!output.contains(VERIFIED));
    }
}

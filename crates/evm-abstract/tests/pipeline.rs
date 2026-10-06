//! 回归测试验证端到端的可观察语义，不依赖工作表的具体访问顺序。

use evm_abstract::{
    U256,
    analysis::{self, Analysis, Config, DiagnosticKind, EdgeKind, Status},
    bytecode::{DecodeError, Program},
    ssa,
};

fn analyze(hex: &str) -> Analysis {
    // 基础 CFG/join 回归沿用上下文不敏感的对照，避免精度默认值掩盖汇合。
    analysis::analyze(
        Program::from_hex(hex).unwrap(),
        Config {
            context_depth: 0,
            ..Config::default()
        },
    )
    .unwrap()
}
fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/../../examples/{name}.hex",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}
fn states_at(analysis: &Analysis, pc: usize) -> Vec<&analysis::State> {
    analysis
        .states()
        .iter()
        .filter(|s| analysis.program().blocks()[s.key.basic_block_index].start_pc == pc)
        .collect()
}

#[test]
fn push_data_is_not_a_jumpdest() {
    let program = Program::from_hex("600456605b00").unwrap();
    assert!(program.jumpdest_blocks().is_empty());
    let analysis = analyze("600456605b00");
    assert!(analysis.edges().is_empty());
    assert!(
        analysis
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::InvalidJump)
    );
}

#[test]
fn truncated_push_pads_on_the_right() {
    let program = Program::from_hex("61ab").unwrap();
    assert_eq!(
        program.blocks()[0].instructions[0].immediate,
        Some(U256::from(0xab00))
    );
    assert_eq!(
        analyze("61ab").states()[0].exit_stack[0]
            .constants()
            .unwrap()
            .len(),
        1
    );
    assert!(analyze("61ab").states()[0].exit_stack[0].contains(U256::from(0xab00)));
}

#[test]
fn unsupported_and_invalid_inputs_are_distinct() {
    assert!(matches!(
        Program::from_hex("ef0001"),
        Err(DecodeError::UnsupportedEof)
    ));
    assert!(matches!(
        Program::from_hex("abc"),
        Err(DecodeError::OddHexLength(3))
    ));
    assert!(matches!(
        Program::from_hex("gg"),
        Err(DecodeError::InvalidHex { .. })
    ));
    assert_eq!(Program::from_hex("0x60 01\n00").unwrap().byte_len(), 3);
    for code in ["0c600100", "4b600100", "e6600100", "fe600100", "f8600100"] {
        let a = analyze(code);
        assert_eq!(a.states()[0].executed_pcs, [0]);
        assert!(
            a.diagnostics()
                .iter()
                .any(|d| d.kind == DiagnosticKind::InvalidOpcode)
        );
        ssa::build(&a).unwrap();
    }
}

#[test]
fn empty_program_is_an_implicit_stop() {
    let a = analyze("");
    assert_eq!(a.status(), Status::Converged);
    assert!(a.states().is_empty());
    assert_eq!(ssa::build(&a).unwrap().value_count(), 0);
}

#[test]
fn unreachable_blocks_remain_visible_in_disassembly() {
    let a = analyze("005b600100");
    assert_eq!(a.program().blocks().len(), 2);
    assert_eq!(a.states().len(), 1);
    assert!(a.edges().is_empty());
}

#[test]
fn arithmetic_target_is_recovered() {
    // 3 + 3 -> pc=6，目标不是紧邻 JUMP 的 PUSH 常量。
    let a = analyze("6003600301565b00");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(states_at(&a, 6).len(), 1);
    assert!(
        !a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::UnknownJump)
    );
}

#[test]
fn zero_condition_does_not_validate_invalid_target() {
    let a = analyze("600060ff5700");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(a.edges()[0].kind, EdgeKind::BranchFalse);
    assert!(
        !a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::InvalidJump)
    );
    ssa::build(&a).unwrap();
}

#[test]
fn nonzero_condition_prunes_fallthrough() {
    let a = analyze("600160075700005b00");
    assert_eq!(a.edges().len(), 1);
    assert_eq!(a.edges()[0].kind, EdgeKind::BranchTrue);
    assert_eq!(states_at(&a, 7).len(), 1);
}

#[test]
fn unknown_jump_preserves_the_storage_write_path() {
    let a = analyze(&fixture("dynamic-jump"));
    assert_eq!(a.status(), Status::Converged);
    assert_eq!(states_at(&a, 4).len(), 1);
    assert!(
        a.diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::UnknownJump)
    );
    assert!(
        ssa::build(&a)
            .unwrap()
            .blocks()
            .iter()
            .flat_map(|b| &b.instructions)
            .any(|i| i.opcode == 0x55)
    );
}

#[test]
fn unknown_jump_covers_every_actual_destination() {
    let a = analyze("600035565b005b005b00");
    let targets: std::collections::BTreeSet<_> = a
        .edges()
        .iter()
        .filter(|e| e.from == 0)
        .map(|e| a.program().blocks()[a.states()[e.to].key.basic_block_index].start_pc)
        .collect();
    assert_eq!(targets, std::collections::BTreeSet::from([4, 6, 8]));
}

#[test]
fn diamond_joins_values_and_ssa_tracks_each_predecessor() {
    let a = analyze(&fixture("diamond"));
    let merge = states_at(&a, 14)[0];
    let values = merge.entry_stack[0].constants().unwrap();
    assert_eq!(
        values,
        &std::collections::BTreeSet::from([U256::from(1), U256::from(2)])
    );
    assert!(merge.exit_stack[0].contains(U256::from(11)));
    assert!(merge.exit_stack[0].contains(U256::from(12)));
    let ssa = ssa::build(&a).unwrap();
    let phi = &ssa.blocks()[merge.id].phis[0];
    assert_eq!(phi.inputs.len(), 2);
    assert_ne!(phi.inputs[0].value, phi.inputs[1].value);
}

#[test]
fn loop_reaches_a_finite_fixed_point_and_has_a_backedge_phi() {
    let a = analyze(&fixture("loop"));
    assert_eq!(a.status(), Status::Converged);
    assert!(a.transfers() < 30);
    let head = states_at(&a, 2)[0];
    assert!(head.entry_stack[0].constants().is_none());
    assert!(
        a.edges()
            .iter()
            .any(|e| e.from == head.id && e.to == head.id)
    );
    let ssa = ssa::build(&a).unwrap();
    assert_eq!(ssa.blocks()[head.id].phis[0].inputs.len(), 2);
}

#[test]
fn contexts_keep_two_internal_invocations_separate() {
    let program = Program::from_hex(&fixture("internal-calls")).unwrap();
    let insensitive = analyze(&fixture("internal-calls"));
    let sensitive = analysis::analyze(
        program,
        Config {
            context_depth: 1,
            ..Config::default()
        },
    )
    .unwrap();
    assert_eq!(states_at(&insensitive, 14).len(), 1);
    let calls = states_at(&sensitive, 14);
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0].key.context, calls[1].key.context);
    assert!(
        calls
            .iter()
            .all(|s| s.entry_stack[0].constants().unwrap().len() == 1)
    );
    assert!(
        states_at(&insensitive, 14)[0].entry_stack[0]
            .constants()
            .unwrap()
            .len()
            > 1
    );
    ssa::build(&sensitive).unwrap();
}

#[test]
fn stack_heights_are_separate_states() {
    let a = analyze(&fixture("stack-heights"));
    let merge = states_at(&a, 12);
    assert_eq!(merge.len(), 2);
    let heights: std::collections::BTreeSet<_> = merge.iter().map(|s| s.key.stack_height).collect();
    assert_eq!(heights, std::collections::BTreeSet::from([0, 1]));
    ssa::build(&a).unwrap();
}

#[test]
fn dup_and_swap_preserve_value_identity() {
    let a = analyze("60018060029000");
    let ssa = ssa::build(&a).unwrap();
    let block = &ssa.blocks()[0];
    assert_eq!(ssa.value_count(), 2);
    assert_eq!(block.exit_stack, [0, 1, 0]);
    assert!(
        block
            .instructions
            .iter()
            .filter(|i| matches!(i.opcode, 0x80 | 0x90))
            .all(|i| i.results.is_empty())
    );
}

#[test]
fn exceptional_stack_paths_do_not_execute_later_instructions() {
    let underflow = analyze("01600100");
    assert_eq!(underflow.states()[0].executed_pcs, [0]);
    assert!(
        underflow
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::StackUnderflow)
    );
    ssa::build(&underflow).unwrap();
    let overflow = analyze(&format!("{}00", "5f".repeat(1025)));
    assert_eq!(overflow.states()[0].exit_stack.len(), 1024);
    assert!(
        overflow
            .diagnostics()
            .iter()
            .any(|d| d.kind == DiagnosticKind::StackOverflow)
    );
    assert_eq!(ssa::build(&overflow).unwrap().value_count(), 1024);
}

#[test]
fn budget_frontiers_are_typed_and_prevent_ssa() {
    for config in [
        Config {
            max_states: 1,
            ..Config::default()
        },
        Config {
            max_transfers: 1,
            ..Config::default()
        },
    ] {
        let a = analysis::analyze(Program::from_hex(&fixture("diamond")).unwrap(), config).unwrap();
        assert_eq!(a.status(), Status::Incomplete);
        assert!(!a.frontiers().is_empty());
        assert!(matches!(
            ssa::build(&a),
            Err(ssa::SsaError::IncompleteAnalysis)
        ));
    }
}

#[test]
fn invalid_configs_are_rejected_before_execution() {
    for config in [
        Config {
            max_constants: 0,
            ..Config::default()
        },
        Config {
            max_constants: 65,
            ..Config::default()
        },
        Config {
            max_states: 0,
            ..Config::default()
        },
        Config {
            max_transfers: 0,
            ..Config::default()
        },
    ] {
        assert!(analysis::analyze(Program::from_hex("00").unwrap(), config).is_err());
    }
}

#[test]
fn result_exports_are_deterministic() {
    let first = analyze(&fixture("loop"));
    let second = analyze(&fixture("loop"));
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        serde_json::to_string(&second).unwrap()
    );
    assert_eq!(
        evm_abstract::render::dot(&first),
        evm_abstract::render::dot(&second)
    );
}

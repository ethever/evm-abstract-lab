//! 故意损坏 IR，验证 verifier 真的拦截不变量，而不只对构建器返回 Ok。

use crate::{
    analysis::{self, Analysis, Config},
    bytecode::Program,
    ssa::{self, Ssa, SsaError, SsaInvariant, SsaInvariantKind as Kind},
};

fn invariant(error: SsaError) -> SsaInvariant {
    let SsaError::Invariant(detail) = error else {
        panic!("expected structural invariant, got {error}")
    };
    *detail
}

fn diamond() -> (Analysis, Ssa) {
    let analysis = analysis::analyze(
        Program::from_hex("600035600b576002600e565b60015b600a0100").unwrap(),
        // 这些损坏 IR 的测试需要两条路径汇合到同一个节点，显式关闭上下文区分。
        Config {
            context_depth: 0,
            ..Config::default()
        },
    )
    .unwrap();
    let ssa = ssa::build(&analysis).unwrap();
    (analysis, ssa)
}

fn state_at(analysis: &Analysis, pc: usize) -> usize {
    analysis
        .states()
        .iter()
        .find(|s| analysis.program().blocks()[s.key.basic_block_index].start_pc == pc)
        .unwrap()
        .id
}

#[test]
fn duplicate_definitions_are_rejected() {
    let (analysis, mut ssa) = diamond();
    ssa.blocks[0].instructions[1].results[0] = ssa.blocks[0].instructions[0].results[0];
    let error = invariant(ssa.verify(&analysis).unwrap_err());
    assert_eq!(error.kind, Kind::DuplicateDefinition);
    assert_eq!(error.state, Some(0));
    assert_eq!(error.pc, Some(ssa.blocks[0].instructions[1].pc));
    assert_eq!(error.value, Some(ssa.blocks[0].instructions[0].results[0]));
    assert_eq!((error.expected, error.observed), (Some(1), Some(2)));
}

#[test]
fn same_shape_but_different_opcode_is_rejected() {
    let (analysis, mut ssa) = diamond();
    let merge = state_at(&analysis, 14);
    ssa.blocks[merge]
        .instructions
        .iter_mut()
        .find(|i| i.opcode == 0x01)
        .unwrap()
        .opcode = 0x03;
    assert!(
        ssa.verify(&analysis)
            .unwrap_err()
            .to_string()
            .contains("differs from the bytecode")
    );
}

#[test]
fn undefined_and_use_before_definition_are_rejected() {
    let (analysis, mut ssa) = diamond();
    let original = ssa.blocks[0].instructions[1].operands[0];
    ssa.blocks[0].instructions[1].operands[0] = usize::MAX;
    let error = invariant(ssa.verify(&analysis).unwrap_err());
    assert_eq!(error.kind, Kind::UndefinedValue);
    assert_eq!(error.state, Some(0));
    assert_eq!(error.pc, Some(ssa.blocks[0].instructions[1].pc));
    assert_eq!(error.value, Some(usize::MAX));
    ssa.blocks[0].instructions[1].operands[0] = ssa.blocks[0].instructions[2].results[0];
    assert!(
        ssa.verify(&analysis)
            .unwrap_err()
            .to_string()
            .contains("use before definition")
    );
    ssa.blocks[0].instructions[1].operands[0] = original;
    ssa.verify(&analysis).unwrap();
}

#[test]
fn phi_inputs_must_cover_all_predecessors() {
    let (analysis, mut ssa) = diamond();
    let merge = state_at(&analysis, 14);
    ssa.blocks[merge].phis[0].inputs.pop();
    let error = invariant(ssa.verify(&analysis).unwrap_err());
    assert_eq!(error.kind, Kind::PhiPredecessorCoverage);
    assert_eq!((error.state, error.slot), (Some(merge), Some(0)));
    assert_eq!((error.expected, error.observed), (Some(2), Some(1)));
}

#[test]
fn phi_input_cannot_come_from_the_other_branch() {
    let (analysis, mut ssa) = diamond();
    let merge = state_at(&analysis, 14);
    let wrong = ssa.blocks[merge].phis[0].inputs[1].value;
    ssa.blocks[merge].phis[0].inputs[0].value = wrong;
    assert!(
        ssa.verify(&analysis)
            .unwrap_err()
            .to_string()
            .contains("predecessor's outgoing slot")
    );
}

#[test]
fn branch_local_definition_cannot_replace_a_merge_phi() {
    let (analysis, mut ssa) = diamond();
    let left = state_at(&analysis, 6);
    let merge = state_at(&analysis, 14);
    let value = ssa.blocks[left].instructions[0].results[0];
    ssa.blocks[merge]
        .instructions
        .iter_mut()
        .find(|i| i.opcode == 0x01)
        .unwrap()
        .operands[1] = value;
    assert!(
        ssa.verify(&analysis)
            .unwrap_err()
            .to_string()
            .contains("does not dominate")
    );
}

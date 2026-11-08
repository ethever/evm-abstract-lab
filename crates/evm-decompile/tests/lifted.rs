//! Independent semantic tests for the first typed high-level projection.
use evm_decompile::{
    Artifact, Completion, Config, EntityId, ExecutionError, ExecutionLimits, ExternalResponse,
    Fork, Input, LiftedExpressionKind as Expr, LiftedProgram, LiftedRelation as Relation,
    LiftedType, LiftedValue, MemoryHost, Stage, StageKind, U256, ValueFact, decompile,
    execute_bytecode, execute_ir, execute_structured, render_text,
};
use std::collections::{BTreeMap, BTreeSet};

#[path = "lifted/captured.rs"]
mod captured;
#[path = "lifted/copies.rs"]
mod copies;
#[path = "lifted/error_payloads.rs"]
mod error_payloads;
#[path = "lifted/guards.rs"]
mod guards;
#[path = "lifted/materialize.rs"]
mod materialize;
#[path = "lifted/named_operands.rs"]
mod named_operands;
#[path = "lifted/oracle.rs"]
mod oracle;
#[path = "lifted/resources.rs"]
mod resources;
#[path = "lifted/support.rs"]
mod support;
#[path = "lifted/transport.rs"]
mod transport;
use oracle::{certify, evaluate, signed_less};
use support::{Asm, calldata, checked, comparison_branch, validated};

fn lifted(artifact: &Artifact) -> (&Stage, &LiftedProgram) {
    let stage = artifact.current().unwrap();
    assert_eq!(stage.kind, StageKind::Lifted);
    (
        stage,
        stage
            .ir
            .lifted
            .as_ref()
            .expect("serializable high semantics"),
    )
}

fn structured(artifact: &Artifact) -> &Stage {
    artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .expect("retained Structured history")
}

fn entry_condition(artifact: &Artifact) -> EntityId {
    let (stage, program) = lifted(artifact);
    program
        .block(stage.ir.entry.unwrap())
        .unwrap()
        .condition
        .unwrap()
}

fn reachable(program: &LiftedProgram, root: EntityId) -> BTreeSet<EntityId> {
    let mut result = BTreeSet::new();
    let mut pending = vec![root];
    while let Some(id) = pending.pop() {
        if result.insert(id) {
            pending.extend(program.expression(id).unwrap().kind.operands());
        }
    }
    result
}

fn assert_unchanged_witness(artifact: &Artifact) {
    let (stage, _) = lifted(artifact);
    let mut witness = stage.ir.clone();
    witness.lifted = None;
    assert_eq!(
        witness,
        structured(artifact).ir,
        "lifting rewrote its primitive/structured witness"
    );
}

fn word_cases() -> Vec<U256> {
    vec![
        U256::ZERO,
        U256::from(1),
        U256::from(2),
        U256::from(255),
        (U256::from(1) << 255) - U256::from(1),
        U256::from(1) << 255,
        U256::MAX,
    ]
}

#[test]
fn generic_ult_with_three_iszero_has_typed_uge_and_semantic_source() {
    let input = comparison_branch(0x10, 3);
    let artifact = checked(&input);
    for (left, right) in [(0, 0), (0, 1), (1, 0), (2, 7), (255, 2)] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(left), U256::from(right)])),
        );
        assert_eq!(
            observed.storage[&U256::ZERO],
            U256::from(u64::from(left >= right))
        );
    }
    let text = render_text(&artifact);
    assert!(
        text.contains(" >= "),
        "unknown ULT + ISZERO^3 must expose UGE: {text}"
    );
    let current = serde_json::to_value(artifact.current().unwrap()).unwrap();
    assert_eq!(
        current["kind"], "lifted",
        "high code needs an actual serializable stage"
    );
    let (_, program) = lifted(&artifact);
    assert!(matches!(
        program.expression(entry_condition(&artifact)).unwrap().kind,
        Expr::Compare {
            relation: Relation::Uge,
            ..
        }
    ));
    assert_unchanged_witness(&artifact);
}

#[test]
fn arbitrary_word_comparisons_and_odd_even_boolean_negations_are_typed() {
    for (opcode, base) in [
        (0x10, Relation::Ult),
        (0x11, Relation::Ugt),
        (0x12, Relation::Slt),
        (0x13, Relation::Sgt),
        (0x14, Relation::Eq),
    ] {
        for inversions in 0..=4 {
            let input = comparison_branch(opcode, inversions);
            let artifact = checked(&input);
            let (stage, program) = lifted(&artifact);
            let root = program.expression(entry_condition(&artifact)).unwrap();
            assert_eq!(root.ty, LiftedType::Predicate);
            let expected = if inversions % 2 == 0 {
                base
            } else {
                base.complement()
            };
            assert!(
                matches!(root.kind, Expr::Compare { relation, .. } if relation == expected),
                "{:?}",
                root.kind
            );
            let loads: Vec<_> = stage
                .ir
                .block(stage.ir.entry.unwrap())
                .unwrap()
                .operations
                .iter()
                .filter(|op| op.opcode == 0x35)
                .collect();
            assert_eq!(loads.len(), 2);
            assert!(
                loads
                    .iter()
                    .all(|op| stage.ir.value(op.outputs[0]).unwrap().fact == ValueFact::Unknown)
            );
            for left in word_cases() {
                for right in word_cases() {
                    let comparison = match opcode {
                        0x10 => left < right,
                        0x11 => left > right,
                        0x12 => signed_less(left, right),
                        0x13 => signed_less(right, left),
                        0x14 => left == right,
                        _ => unreachable!(),
                    };
                    let expected = comparison ^ (inversions % 2 == 1);
                    let bindings =
                        BTreeMap::from([(loads[0].outputs[0], right), (loads[1].outputs[0], left)]);
                    assert_eq!(
                        evaluate(program, root.id, root.block, &bindings),
                        LiftedValue::Predicate(expected)
                    );
                }
            }
            // Exercise real branches for both sign interpretations of the high bit.
            let top = U256::from(1) << 255;
            for (left, right) in [(top, U256::ZERO), (U256::ZERO, top), (top, top)] {
                certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[left, right])),
                );
            }
        }
    }
}

#[test]
fn root_browser_fixture_has_unknown_uge_with_unchanged_structured_history() {
    let input = Input::new(
        vec![
            0x60, 0x20, 0x35, 0x5f, 0x35, 0x10, 0x15, 0x15, 0x15, 0x60, 0x0f, 0x57, 0x5f, 0x5f,
            0xfd, 0x5b, 0x00,
        ],
        Fork::Osaka,
    );
    let artifact = checked(&input);
    let (_, program) = lifted(&artifact);
    assert!(matches!(
        program.expression(entry_condition(&artifact)).unwrap().kind,
        Expr::Compare {
            relation: Relation::Uge,
            ..
        }
    ));
    for left in word_cases() {
        for right in word_cases() {
            let (raw, _) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[left, right])),
            );
            assert!(
                matches!(raw.outcome, evm_decompile::ExecutionOutcome::Exit { kind, .. }
            if kind == if left >= right { evm_decompile::ExitKind::Stop } else { evm_decompile::ExitKind::Revert })
            );
        }
    }
    assert_unchanged_witness(&artifact);
}

#[test]
fn double_iszero_of_two_is_numeric_one_and_unknown_word_uses_an_explicit_cast() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .op(0x15)
        .op(0x15)
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (_, program) = lifted(&artifact);
    assert!(
        program
            .expressions
            .iter()
            .any(|expr| matches!(expr.kind, Expr::WordFromPredicate(_)))
    );
    for value in word_cases() {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[value])),
        );
        let expected = U256::from(u64::from(value != U256::ZERO));
        assert_eq!(observed.storage[&U256::ZERO], expected);
        assert_eq!(observed.storage[&U256::from(1)], expected);
    }
    let constant = Input::new(vec![0x60, 2, 0x15, 0x15, 0x5f, 0x55, 0x00], Fork::Osaka);
    let artifact = checked(&constant);
    let (_, observed) = certify(&constant, &artifact, MemoryHost::new(&constant, Vec::new()));
    assert_eq!(observed.storage[&U256::ZERO], U256::from(1));
}

#[test]
fn comparison_consumed_by_addition_remains_zero_or_one() {
    let mut asm = Asm::default();
    asm.push(32)
        .op(0x35)
        .op(0x5f)
        .op(0x35)
        .op(0x10)
        .push(2)
        .op(0x01)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (_, program) = lifted(&artifact);
    assert!(
        program
            .expressions
            .iter()
            .any(|expr| matches!(expr.kind, Expr::WordFromPredicate(_)))
    );
    for (left, right) in [(2, 9), (9, 2), (2, 2)] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(left), U256::from(right)])),
        );
        assert_eq!(
            observed.storage[&U256::ZERO],
            U256::from(2 + u64::from(left < right))
        );
    }
}

#[test]
fn multi_input_boolean_phi_keeps_word_two_distinct_from_numeric_true() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .target("two")
        .op(0x57)
        .op(0x5f)
        .push(9)
        .target("join")
        .op(0x56)
        .dest("two")
        .push(2)
        .push(7)
        .dest("join")
        .push(1)
        .op(0x55)
        .op(0x15)
        .op(0x15)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    for condition in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[condition])),
        );
        assert_eq!(
            observed.storage[&U256::ZERO],
            U256::from(u64::from(condition != U256::ZERO))
        );
        assert_eq!(
            observed.storage[&U256::from(1)],
            U256::from(if condition == U256::ZERO { 9 } else { 7 })
        );
    }
}

#[test]
fn duplicate_masks_and_word_identities_simplify_for_unknown_inputs() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .push(0xf0)
        .op(0x16)
        .push(0xf0)
        .op(0x16)
        .op(0x5f)
        .op(0x17)
        .op(0x5f)
        .op(0x18)
        .op(0x5f)
        .op(0x01)
        .push(1)
        .op(0x02)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let block = program.block(stage.ir.entry.unwrap()).unwrap();
    let store = block
        .statements
        .iter()
        .find(|statement| {
            stage
                .ir
                .block(block.block)
                .unwrap()
                .operations
                .iter()
                .any(|op| op.id == statement.operation && op.opcode == 0x55)
        })
        .unwrap();
    let roots = reachable(program, store.inputs[1]);
    let operations: Vec<_> = program
        .expressions
        .iter()
        .filter(|expr| roots.contains(&expr.id))
        .filter_map(|expr| match expr.kind {
            Expr::WordOp { opcode, .. } => Some(opcode),
            _ => None,
        })
        .collect();
    assert_eq!(
        operations,
        vec![0x16],
        "high mask expression retained duplicate masks/identity operations"
    );
    for value in word_cases() {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[value])),
        );
        assert_eq!(observed.storage[&U256::ZERO], value & U256::from(0xf0));
    }
    for (opcode, expected_zero) in [(0x16, false), (0x17, false), (0x18, true)] {
        let input = Input::new(
            vec![0x5f, 0x35, 0x80, opcode, 0x5f, 0x55, 0x00],
            Fork::Osaka,
        );
        let artifact = checked(&input);
        for value in word_cases() {
            let (_, observed) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[value])),
            );
            assert_eq!(
                observed.storage[&U256::ZERO],
                if expected_zero { U256::ZERO } else { value }
            );
        }
    }
}

#[test]
fn effectful_multi_use_producer_runs_once_and_keeps_its_original_value() {
    let input = Input::new(
        vec![
            0x5f, 0x54, 0x80, 0x60, 1, 0x01, 0x60, 1, 0x55, 0x60, 2, 0x01, 0x60, 2, 0x55, 0x00,
        ],
        Fork::Osaka,
    );
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let read = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == 0x54)
        .unwrap();
    assert_eq!(
        program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter(|statement| statement.operation == read.id)
            .count(),
        1
    );
    let mut host = MemoryHost::new(&input, Vec::new());
    host.storage.insert(U256::ZERO, U256::from(7));
    let (raw, observed) = certify(&input, &artifact, host);
    assert_eq!(
        raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
        1
    );
    assert_eq!(observed.storage[&U256::from(1)], U256::from(8));
    assert_eq!(observed.storage[&U256::from(2)], U256::from(9));
}

#[test]
fn unused_memory_and_call_results_preserve_effects_once_in_order() {
    let mut asm = Asm::default();
    asm.push(42).op(0x5f).op(0x52).op(0x5f).op(0x51).op(0x50);
    for _ in 0..7 {
        asm.op(0x5f);
    }
    asm.op(0xf1)
        .op(0x80)
        .op(0x15)
        .op(0x15)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let retained: Vec<_> = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .map(|statement| {
            stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .find(|op| op.id == statement.operation)
                .unwrap()
                .opcode
        })
        .collect();
    assert_eq!(retained.iter().filter(|op| **op == 0x51).count(), 1);
    assert_eq!(retained.iter().filter(|op| **op == 0xf1).count(), 1);
    for success in [false, true] {
        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(ExternalResponse {
            result: U256::from(u64::from(success)),
            return_data: vec![7, 8],
            ..ExternalResponse::default()
        });
        let (raw, observed) = certify(&input, &artifact, host);
        assert_eq!(
            raw.trace.iter().filter(|step| step.opcode == 0xf1).count(),
            1
        );
        assert!(observed.responses.is_empty());
        assert_eq!(observed.return_data, [7, 8]);
        assert_eq!(observed.memory[31], 42);
        assert_eq!(
            observed.storage[&U256::ZERO],
            U256::from(u64::from(success))
        );
        assert_eq!(
            observed.storage[&U256::from(1)],
            U256::from(u64::from(success))
        );
    }
}

#[test]
fn cross_block_multi_input_phi_keeps_captures_and_projects_parallel_transfers() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .op(0x80)
        .push(1)
        .op(0x01)
        .push(32)
        .op(0x35)
        .target("then")
        .op(0x57)
        .op(0x90)
        .target("join")
        .op(0x56)
        .dest("then")
        .dest("join")
        .op(0x80)
        .op(0x5f)
        .op(0x55)
        .op(0x90)
        .push(1)
        .op(0x55)
        .op(0x50)
        .op(0x00);
    let join = asm.labels["join"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    assert!(program.blocks.iter().any(|high| {
        stage
            .ir
            .block(high.block)
            .unwrap()
            .source
            .is_some_and(|source| source.pc == join)
            && high.parameters.len() == 2
    }));
    for value in [U256::from(2), U256::MAX] {
        for condition in [0, 1] {
            let (_, observed) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[value, U256::from(condition)])),
            );
            let incremented = value.wrapping_add(U256::from(1));
            assert_eq!(
                observed.storage[&U256::ZERO],
                if condition == 0 { value } else { incremented }
            );
            assert_eq!(
                observed.storage[&U256::from(1)],
                if condition == 0 { incremented } else { value }
            );
        }
    }
}

fn loop_registers(sum: bool) -> Input {
    let mut asm = Asm::default();
    asm.push(1)
        .push(2)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .push(if sum { 4 } else { 3 })
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .op(0x5f)
        .op(0x55)
        .push(1)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01);
    if sum {
        asm.op(0x91)
            .op(0x81)
            .op(0x01)
            .op(0x91)
            .op(0x90)
            .op(0x91)
            .op(0x90);
    } else {
        asm.op(0x91).op(0x90).op(0x91);
    }
    asm.target("head").op(0x56);
    asm.finish()
}

#[test]
fn loop_backedge_swaps_evaluate_all_rhs_before_overwriting_parameters() {
    let input = loop_registers(false);
    let artifact = checked(&input);
    let (_, observed) = certify(&input, &artifact, MemoryHost::new(&input, Vec::new()));
    assert_eq!(observed.storage[&U256::ZERO], U256::from(1));
    assert_eq!(observed.storage[&U256::from(1)], U256::from(2));
    let (_, program) = lifted(&artifact);
    assert!(
        program
            .transfers
            .iter()
            .any(|transfer| transfer.arguments.len() >= 3)
    );
}

#[test]
fn loop_prior_iteration_arithmetic_capture_is_not_reexpanded_after_phi_updates() {
    let input = loop_registers(true);
    let artifact = checked(&input);
    let (_, observed) = certify(&input, &artifact, MemoryHost::new(&input, Vec::new()));
    assert_eq!(observed.storage[&U256::ZERO], U256::from(13));
    assert_eq!(observed.storage[&U256::from(1)], U256::from(8));
}

#[test]
fn pop_only_entry_word_and_resolved_jump_address_disappear_from_high_bindings() {
    let mut asm = Asm::default();
    asm.push(201).target("pop").op(0x56);
    let pop_pc = asm.pc();
    asm.dest("pop").op(0x50).op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let original = structured(&artifact)
        .ir
        .blocks
        .iter()
        .find(|block| block.source.is_some_and(|source| source.pc == pop_pc))
        .unwrap();
    assert!(!original.parameters.is_empty());
    let pop = original
        .operations
        .iter()
        .find(|op| op.opcode == 0x50)
        .unwrap();
    assert!(
        pop.inputs.iter().any(|id| original.parameters.contains(id)),
        "Structured must retain the true POP use"
    );
    assert!(program.block(original.id).unwrap().parameters.is_empty());
    assert!(
        program
            .transfers
            .iter()
            .filter(|transfer| stage.ir.edge(transfer.edge).unwrap().to == original.id)
            .all(|transfer| transfer.arguments.is_empty())
    );
    assert!(program.blocks.iter().all(|block| block.target.is_none()));
    assert!(
        program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .all(|statement| statement.operation != pop.id)
    );
    certify(&input, &artifact, MemoryHost::new(&input, Vec::new()));
    assert_unchanged_witness(&artifact);
}

#[test]
fn unresolved_jump_keeps_a_typed_target_and_explicit_partial_boundary() {
    let input = Input::new(vec![0x5f, 0x35, 0x56], Fork::Osaka);
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    validated(&artifact);
    let (stage, program) = lifted(&artifact);
    let raw = stage.ir.block(stage.ir.entry.unwrap()).unwrap();
    let high = program.block(raw.id).unwrap();
    let target = program
        .expression(high.target.expect("unresolved target retained"))
        .unwrap();
    assert_eq!(target.ty, LiftedType::Word256);
    let load = raw.operations.iter().find(|op| op.opcode == 0x35).unwrap();
    let expected = U256::from(123);
    assert_eq!(
        evaluate(
            program,
            target.id,
            raw.id,
            &BTreeMap::from([(load.outputs[0], expected)])
        ),
        LiftedValue::Word(expected)
    );
    assert!(matches!(
        raw.terminator,
        evm_decompile::Terminator::Jump {
            unresolved: true,
            ..
        }
    ));
    assert!(render_text(&artifact).contains("partial_indirect_exit"));
    assert_unchanged_witness(&artifact);
}

#[test]
fn residual_bounds_error_and_exceptional_operations_survive_high_cleanup() {
    let input = Input::new(vec![0x60, 1, 0x5f, 0x5f, 0x3e, 0x00], Fork::Osaka);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let operation = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == 0x3e)
        .unwrap();
    assert!(
        program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .any(|statement| statement.operation == operation.id)
    );
    let limits = ExecutionLimits::default();
    assert_eq!(
        execute_bytecode(&input, &mut MemoryHost::new(&input, Vec::new()), &limits),
        Err(ExecutionError::ReturnDataBounds)
    );
    assert_eq!(
        execute_ir(stage, &mut MemoryHost::new(&input, Vec::new()), &limits),
        Err(ExecutionError::ReturnDataBounds)
    );
    assert_eq!(
        execute_structured(stage, &mut MemoryHost::new(&input, Vec::new()), &limits),
        Err(ExecutionError::ReturnDataBounds)
    );
    for bytes in [vec![0xfe], vec![0x90], vec![0x60, 255, 0x56]] {
        let input = Input::new(bytes, Fork::Osaka);
        let artifact = checked(&input);
        certify(&input, &artifact, MemoryHost::new(&input, Vec::new()));
        assert_unchanged_witness(&artifact);
    }
}

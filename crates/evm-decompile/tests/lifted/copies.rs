//! Copy identity is inferred from bytecode transfers, independently of the
//! projector. The trace oracle binds only surviving high statements/parameters.
use super::captured;
use super::oracle::certify;
use super::support::{Asm, calldata, checked, validated};
use super::{assert_unchanged_witness, lifted, reachable, structured};
use evm_decompile::{
    Artifact, Block, Cancellation, Completion, Config, EntityId, EntityRef, ExternalResponse,
    FactScope, Input, LiftedExpressionKind as Expr, LiftedProgram, MemoryHost, Phase, Stage,
    StageKind, U256, ValidationStatus, ValueFact, decompile, decompile_with_progress,
    validate_stage,
};
use std::collections::BTreeSet;

fn block_at(stage: &Stage, pc: usize) -> &Block {
    let mut blocks = stage
        .ir
        .blocks
        .iter()
        .filter(|block| block.source.is_some_and(|source| source.pc == pc));
    let block = blocks.next().expect("fixture source block");
    assert!(blocks.next().is_none(), "fixture needs an explicit context");
    block
}

fn value_leaves(program: &LiftedProgram, root: EntityId) -> BTreeSet<EntityId> {
    reachable(program, root)
        .into_iter()
        .filter_map(|id| match program.expression(id).unwrap().kind {
            Expr::Value(value) => Some(value),
            _ => None,
        })
        .collect()
}

fn assert_projection_integrity(artifact: &Artifact) {
    assert_unchanged_witness(artifact);
    let (stage, program) = lifted(artifact);
    let leaves: BTreeSet<_> = program
        .expressions
        .iter()
        .filter_map(|expression| match expression.kind {
            Expr::Value(value) => Some(value),
            _ => None,
        })
        .collect();
    let bindings: BTreeSet<_> = program
        .blocks
        .iter()
        .flat_map(|high| {
            high.parameters.iter().chain(
                high.statements
                    .iter()
                    .flat_map(|statement| &statement.outputs),
            )
        })
        .copied()
        .collect();
    assert_eq!(leaves, bindings, "a high value lacks a live binding or use");
    for expression in &program.expressions {
        for operand in expression.kind.operands() {
            assert_eq!(
                expression.block,
                program.expression(operand).unwrap().block,
                "expression arenas must remain consumer-block-local"
            );
        }
    }
    for raw in &stage.ir.blocks {
        let high = program.block(raw.id).unwrap();
        let raw_effects: Vec<_> = raw
            .operations
            .iter()
            .filter(|operation| !operation.effects.is_pure())
            .map(|operation| operation.id)
            .collect();
        let high_effects: Vec<_> = high
            .statements
            .iter()
            .filter(|statement| {
                !raw.operations
                    .iter()
                    .find(|operation| operation.id == statement.operation)
                    .unwrap()
                    .effects
                    .is_pure()
            })
            .map(|statement| statement.operation)
            .collect();
        assert_eq!(raw_effects, high_effects, "primitive effect order/count");
        for statement in &high.statements {
            let operation = raw
                .operations
                .iter()
                .find(|operation| operation.id == statement.operation)
                .unwrap();
            assert_eq!(statement.inputs.len(), operation.inputs.len());
            assert!(
                statement
                    .outputs
                    .iter()
                    .all(|id| operation.outputs.contains(id))
            );
        }
    }
    assert_eq!(program.transfers.len(), stage.ir.edges.len());
    for edge in &stage.ir.edges {
        let high = program
            .transfer(edge.id)
            .expect("even empty edges retain witnesses");
        assert_eq!(
            high.arguments.len(),
            program.block(edge.to).unwrap().parameters.len()
        );
    }
}

fn copy_chain(blocks: usize) -> Input {
    wide_copy_chain(blocks, 1)
}

fn wide_copy_chain(blocks: usize, words: u8) -> Input {
    let mut asm = Asm::default();
    for slot in 0..words {
        asm.push(slot).op(0x54);
    }
    for _ in 0..blocks {
        asm.next_block();
    }
    for slot in (0..words).rev() {
        asm.push(128 + slot).op(0x55);
    }
    asm.op(0x00);
    asm.finish()
}

#[test]
fn positive_sload_chain_captures_old_value_for_store_and_delegatecall() {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x54).next_block().next_block();
    // Mutate the slot while its old value is still carried on the stack.
    asm.push(99).op(0x5f).op(0x55).next_block();
    asm.op(0x80).push(1).op(0x55);
    for _ in 0..4 {
        asm.op(0x5f);
    }
    asm.op(0x84)
        .push(255)
        .op(0xf4)
        .op(0x50)
        .push(2)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let read = stage.ir.blocks[0]
        .operations
        .iter()
        .find(|operation| operation.opcode == 0x54)
        .unwrap();
    assert!(
        stage
            .ir
            .blocks
            .iter()
            .skip(1)
            .all(|block| !block.parameters.is_empty())
    );
    assert!(
        program
            .blocks
            .iter()
            .all(|block| block.parameters.is_empty()),
        "proven forwarding parameters survive the high projection"
    );
    assert!(
        program
            .transfers
            .iter()
            .all(|transfer| transfer.arguments.is_empty())
    );
    let call = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| {
            stage
                .ir
                .blocks
                .iter()
                .flat_map(|block| &block.operations)
                .any(|operation| operation.id == statement.operation && operation.opcode == 0xf4)
        })
        .unwrap();
    assert_eq!(
        value_leaves(program, call.inputs[1]),
        BTreeSet::from([read.outputs[0]])
    );
    for old in [U256::ZERO, U256::from(7), U256::MAX] {
        for success in [false, true] {
            let mut host = MemoryHost::new(&input, Vec::new());
            host.storage.insert(U256::ZERO, old);
            host.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                ..ExternalResponse::default()
            });
            let (raw, observed) = certify(&input, &artifact, host);
            assert_eq!(observed.storage[&U256::ZERO], U256::from(99));
            assert_eq!(observed.storage[&U256::from(1)], old);
            assert_eq!(observed.storage[&U256::from(2)], old);
            let effects: Vec<_> = raw
                .trace
                .iter()
                .filter(|step| matches!(step.opcode, 0x54 | 0x55 | 0xf4))
                .map(|step| step.opcode)
                .collect();
            assert_eq!(effects, [0x54, 0x55, 0x55, 0xf4, 0x55]);
            assert_eq!(
                raw.trace
                    .iter()
                    .find(|step| step.opcode == 0xf4)
                    .unwrap()
                    .inputs[1],
                old
            );
            assert!(observed.responses.is_empty());
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_all_incoming_equal_diamond_removes_join_copies() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x54)
        .op(0x5f)
        .op(0x35)
        .target("right")
        .op(0x57)
        .target("join")
        .op(0x56)
        .dest("right")
        .target("join")
        .op(0x56)
        .dest("join")
        .op(0x80)
        .push(1)
        .op(0x55)
        .push(2)
        .op(0x55)
        .op(0x00);
    let join_pc = asm.labels["join"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let join = block_at(stage, join_pc);
    assert_eq!(
        stage
            .ir
            .edges
            .iter()
            .filter(|edge| edge.to == join.id)
            .count(),
        2
    );
    assert_eq!(join.parameters.len(), 1);
    assert!(
        program
            .blocks
            .iter()
            .all(|block| block.parameters.is_empty())
    );
    for old in [U256::ZERO, U256::from(21), U256::MAX] {
        for branch in [0, 1, 2] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(branch)]));
            host.storage.insert(U256::ZERO, old);
            let (_, observed) = certify(&input, &artifact, host);
            assert_eq!(observed.storage[&U256::from(1)], old);
            assert_eq!(observed.storage[&U256::from(2)], old);
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_pure_word_roots_materialize_once_before_cross_block_use() {
    for opcode in [0x01, 0x10] {
        let mut asm = Asm::default();
        asm.op(0x5f)
            .op(0x35)
            .push(7)
            .op(opcode)
            .next_block()
            .next_block()
            .op(0x80)
            .push(1)
            .op(0x55)
            .next_block()
            .push(2)
            .op(0x55)
            .op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let (stage, program) = lifted(&artifact);
        let original = &stage.ir.blocks[0];
        let operation = original
            .operations
            .iter()
            .find(|operation| operation.opcode == opcode)
            .unwrap();
        let retained: Vec<_> = program
            .blocks
            .iter()
            .flat_map(|block| {
                block
                    .statements
                    .iter()
                    .map(move |statement| (block, statement))
            })
            .filter(|(_, statement)| statement.operation == operation.id)
            .collect();
        assert_eq!(
            retained.len(),
            1,
            "pure canonical root needs exactly one dynamic binding"
        );
        assert_eq!(retained[0].0.block, original.id);
        assert_eq!(retained[0].1.outputs, operation.outputs);
        assert!(
            program
                .blocks
                .iter()
                .all(|block| block.parameters.is_empty())
        );
        for high in program
            .blocks
            .iter()
            .filter(|block| block.block != original.id)
        {
            for statement in &high.statements {
                let raw = stage
                    .ir
                    .block(high.block)
                    .unwrap()
                    .operations
                    .iter()
                    .find(|operation| operation.id == statement.operation)
                    .unwrap();
                if raw.opcode == 0x55 {
                    assert_eq!(
                        value_leaves(program, statement.inputs[1]),
                        BTreeSet::from([operation.outputs[0]])
                    );
                }
            }
        }
        for value in [U256::ZERO, U256::from(7), U256::from(8), U256::MAX] {
            let (_, observed) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[value])),
            );
            let expected = if opcode == 0x01 {
                value.wrapping_add(U256::from(7))
            } else {
                U256::from(u64::from(U256::from(7) < value))
            };
            assert_eq!(observed.storage[&U256::from(1)], expected);
            assert_eq!(observed.storage[&U256::from(2)], expected);
        }
        assert_projection_integrity(&artifact);
    }
}

fn unequal_diamond(reads: bool, tail: bool) -> (Input, usize, usize) {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35).target("right").op(0x57);
    if reads {
        asm.op(0x5f).op(0x54);
    } else {
        asm.push(9);
    }
    asm.target("join").op(0x56).dest("right");
    if reads {
        asm.op(0x5f).op(0x54);
    } else {
        asm.push(13);
    }
    asm.dest("join");
    let join = asm.labels["join"];
    if tail {
        asm.next_block();
    }
    let consumer = if tail { asm.pc() - 1 } else { join };
    asm.push(1).op(0x55).op(0x00);
    (asm.finish(), join, consumer)
}

#[test]
fn negative_distinct_incoming_values_remain_a_true_join() {
    let (input, join_pc, _) = unequal_diamond(false, false);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let join = block_at(stage, join_pc);
    assert_eq!(program.block(join.id).unwrap().parameters, join.parameters);
    assert_eq!(join.parameters.len(), 1);
    for branch in [0, 1, 7] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[U256::from(branch)])),
        );
        assert_eq!(
            observed.storage[&U256::from(1)],
            U256::from(if branch == 0 { 9 } else { 13 })
        );
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn negative_equal_sload_facts_keep_distinct_definitions() {
    let (input, join_pc, _) = unequal_diamond(true, false);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let reads: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .filter(|operation| operation.opcode == 0x54)
        .collect();
    assert_eq!(reads.len(), 2);
    assert_ne!(reads[0].outputs, reads[1].outputs);
    assert_eq!(
        stage.ir.value(reads[0].outputs[0]).unwrap().fact,
        ValueFact::Unknown
    );
    assert_eq!(
        stage.ir.value(reads[1].outputs[0]).unwrap().fact,
        ValueFact::Unknown
    );
    let join = block_at(stage, join_pc);
    assert_eq!(program.block(join.id).unwrap().parameters, join.parameters);
    assert_eq!(join.parameters.len(), 1);
    let incoming: BTreeSet<_> = stage
        .ir
        .edges
        .iter()
        .filter(|edge| edge.to == join.id)
        .flat_map(|edge| value_leaves(program, program.transfer(edge.id).unwrap().arguments[0]))
        .collect();
    assert_eq!(incoming, reads.iter().map(|read| read.outputs[0]).collect());
    for old in [U256::ZERO, U256::from(33), U256::MAX] {
        for branch in [0, 1] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(branch)]));
            host.storage.insert(U256::ZERO, old);
            let (raw, observed) = certify(&input, &artifact, host);
            assert_eq!(observed.storage[&U256::from(1)], old);
            assert_eq!(
                raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
                1
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_true_join_root_forwards_to_dominated_tail() {
    let (input, join_pc, consumer_pc) = unequal_diamond(true, true);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let join = block_at(stage, join_pc);
    let consumer = block_at(stage, consumer_pc);
    assert_eq!(program.block(join.id).unwrap().parameters, join.parameters);
    assert!(program.block(consumer.id).unwrap().parameters.is_empty());
    let store = program
        .block(consumer.id)
        .unwrap()
        .statements
        .iter()
        .find(|statement| {
            consumer
                .operations
                .iter()
                .any(|operation| operation.id == statement.operation && operation.opcode == 0x55)
        })
        .unwrap();
    assert_eq!(
        value_leaves(program, store.inputs[1]),
        BTreeSet::from([join.parameters[0]])
    );
    for branch in [0, 1] {
        let mut host = MemoryHost::new(&input, calldata(&[U256::from(branch)]));
        host.storage.insert(U256::ZERO, U256::from(73));
        certify(&input, &artifact, host);
    }
    assert_projection_integrity(&artifact);
}

fn invariant_loop() -> (Input, usize, usize) {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x54)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .op(0x5f)
        .op(0x35)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .push(1)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .next_block()
        .target("head")
        .op(0x56);
    let head = asm.labels["head"];
    let body = asm.labels["body"];
    (asm.finish(), head, body)
}

#[test]
fn positive_loop_invariant_self_phi_and_anchored_copy_scc_disappear() {
    let (input, head_pc, body_pc) = invariant_loop();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    for pc in [head_pc, body_pc] {
        let raw = block_at(stage, pc);
        assert_eq!(raw.parameters.len(), 2);
        assert!(
            !program
                .block(raw.id)
                .unwrap()
                .parameters
                .contains(&raw.parameters[0]),
            "the dominating SLOAD anchors this loop copy SCC"
        );
    }
    let head = block_at(stage, head_pc);
    assert_eq!(
        program.block(head.id).unwrap().parameters,
        vec![head.parameters[1]],
        "the changing induction value is a real phi"
    );
    for old in [U256::ZERO, U256::from(41), U256::MAX] {
        for iterations in [0, 1, 2, 7] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(iterations)]));
            host.storage.insert(U256::ZERO, old);
            let (raw, observed) = certify(&input, &artifact, host);
            assert_eq!(observed.storage[&U256::from(1)], old);
            assert_eq!(
                raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
                1
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_loop_self_phi_uses_external_observation_anchor() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x54)
        .op(0x5f)
        .dest("head")
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x35)
        .op(0x90)
        .op(0x10)
        .target("head")
        .op(0x57)
        .op(0x50)
        .push(1)
        .op(0x55)
        .op(0x00);
    let head_pc = asm.labels["head"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let head = block_at(stage, head_pc);
    assert!(
        stage
            .ir
            .edges
            .iter()
            .any(|edge| edge.from == head.id && edge.to == head.id)
    );
    assert_eq!(head.parameters.len(), 2);
    assert_eq!(
        program.block(head.id).unwrap().parameters,
        vec![head.parameters[1]]
    );
    for old in [U256::ZERO, U256::MAX] {
        for iterations in [0, 1, 2, 5] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(iterations)]));
            host.storage.insert(U256::ZERO, old);
            let (raw, observed) = certify(&input, &artifact, host);
            assert_eq!(observed.storage[&U256::from(1)], old);
            assert_eq!(
                raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
                1
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_true_phi_copy_inside_dependency_scc_is_eliminated() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .target("right")
        .op(0x57)
        .op(0x5f)
        .op(0x54)
        .op(0x5f)
        .target("head")
        .op(0x56)
        .dest("right")
        .push(1)
        .op(0x54)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .push(32)
        .op(0x35)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .push(2)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .target("head")
        .op(0x56);
    let head_pc = asm.labels["head"];
    let body_pc = asm.labels["body"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let head = block_at(stage, head_pc);
    let body = block_at(stage, body_pc);
    assert!(
        program
            .block(head.id)
            .unwrap()
            .parameters
            .contains(&head.parameters[0]),
        "different entry anchors require the true phi p = phi(a,b,q)"
    );
    assert!(
        !program
            .block(body.id)
            .unwrap()
            .parameters
            .contains(&body.parameters[0]),
        "q = p forwards the retained dominating phi even inside its dependency SCC"
    );
    for branch in [0, 1] {
        for iterations in [0, 1, 3] {
            let mut host = MemoryHost::new(
                &input,
                calldata(&[U256::from(branch), U256::from(iterations)]),
            );
            host.storage.insert(U256::ZERO, U256::from(17));
            host.storage.insert(U256::from(1), U256::from(29));
            let (_, observed) = certify(&input, &artifact, host);
            assert_eq!(
                observed.storage[&U256::from(2)],
                U256::from(if branch == 0 { 17 } else { 29 })
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn negative_loop_header_read_keeps_previous_iteration_capture() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x54)
        .op(0x5f)
        .dest("head")
        .op(0x81)
        .push(1)
        .op(0x55)
        .op(0x5f)
        .op(0x54)
        .op(0x80)
        .push(1)
        .op(0x01)
        .op(0x5f)
        .op(0x55)
        .op(0x91)
        .op(0x50)
        .op(0x80)
        .op(0x5f)
        .op(0x35)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57)
        .op(0x50)
        .push(2)
        .op(0x55)
        .op(0x00)
        .dest("body")
        .push(1)
        .op(0x01)
        .target("head")
        .op(0x56);
    let head_pc = asm.labels["head"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let head = block_at(stage, head_pc);
    let prior = head.parameters[0];
    assert!(
        program.block(head.id).unwrap().parameters.contains(&prior),
        "a header operation is not available at that header's entry"
    );
    let first_store = program
        .block(head.id)
        .unwrap()
        .statements
        .iter()
        .find(|statement| {
            head.operations
                .iter()
                .any(|operation| operation.id == statement.operation && operation.opcode == 0x55)
        })
        .unwrap();
    assert_eq!(
        value_leaves(program, first_store.inputs[1]),
        BTreeSet::from([prior])
    );
    for old in [U256::ZERO, U256::from(9), U256::MAX] {
        for iterations in [0_u64, 1, 2, 5] {
            let mut host = MemoryHost::new(&input, calldata(&[U256::from(iterations)]));
            host.storage.insert(U256::ZERO, old);
            let (raw, observed) = certify(&input, &artifact, host);
            assert_eq!(
                observed.storage[&U256::ZERO],
                old.wrapping_add(U256::from(iterations + 1))
            );
            assert_eq!(
                observed.storage[&U256::from(1)],
                old.wrapping_add(U256::from(iterations.saturating_sub(1)))
            );
            assert_eq!(
                observed.storage[&U256::from(2)],
                old.wrapping_add(U256::from(iterations))
            );
            assert_eq!(
                raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
                usize::try_from(iterations).unwrap() + 2
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn negative_swaps_and_induction_keep_parallel_assignment() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .push(32)
        .op(0x35)
        .op(0x5f)
        .dest("head")
        .op(0x80)
        .push(64)
        .op(0x35)
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
        .op(0x01)
        .op(0x91)
        .op(0x90)
        .op(0x91)
        .target("head")
        .op(0x56);
    let head_pc = asm.labels["head"];
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let head = block_at(stage, head_pc);
    assert_eq!(program.block(head.id).unwrap().parameters, head.parameters);
    assert_eq!(head.parameters.len(), 3);
    for (first, second) in [
        (U256::ZERO, U256::from(1)),
        (U256::from(3), U256::from(17)),
        (U256::MAX, U256::from(1) << 255),
    ] {
        for iterations in 0..=5 {
            let (_, observed) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[first, second, U256::from(iterations)])),
            );
            assert_eq!(
                observed.storage[&U256::ZERO],
                if iterations % 2 == 0 { second } else { first }
            );
            assert_eq!(
                observed.storage[&U256::from(1)],
                if iterations % 2 == 0 { first } else { second }
            );
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn negative_partial_cfg_preserves_single_known_incoming_copies() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x54)
        .next_block()
        .next_block()
        .op(0x5f)
        .op(0x35)
        .push(32)
        .op(0x35)
        .op(0x57)
        .push(1)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = decompile(&input, &Config::default());
    assert_eq!(artifact.completion, Completion::Partial);
    validated(&artifact);
    let (stage, program) = lifted(&artifact);
    assert_eq!(stage.ir.fact_scope, FactScope::RecoveredPaths);
    let forwarding: Vec<_> = stage
        .ir
        .blocks
        .iter()
        .filter(|block| !block.parameters.is_empty())
        .collect();
    assert!(forwarding.len() >= 3);
    for raw in forwarding {
        assert_eq!(
            stage
                .ir
                .edges
                .iter()
                .filter(|edge| edge.to == raw.id)
                .count(),
            1
        );
        assert_eq!(
            program.block(raw.id).unwrap().parameters,
            raw.parameters,
            "one represented incoming edge does not prove complete predecessor coverage"
        );
    }
    assert!(program.blocks.iter().any(|block| block.target.is_some()));
    for old in [U256::ZERO, U256::from(51), U256::MAX] {
        let mut host = MemoryHost::new(&input, calldata(&[U256::ZERO, U256::from(0xffff)]));
        host.storage.insert(U256::ZERO, old);
        let (_, observed) = certify(&input, &artifact, host);
        assert_eq!(observed.storage[&U256::from(1)], old);
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_captured_usdc_forwards_the_original_sload_to_delegatecall() {
    let input = captured::proxy();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let mut routes = Vec::new();
    for read_block in stage
        .ir
        .blocks
        .iter()
        .filter(|block| block.source.is_some_and(|source| source.pc == 0x651))
    {
        for first in stage
            .ir
            .edges
            .iter()
            .filter(|edge| edge.from == read_block.id)
        {
            let shim = stage.ir.block(first.to).unwrap();
            if !shim.source.is_some_and(|source| source.pc == 0x20c) {
                continue;
            }
            for second in stage.ir.edges.iter().filter(|edge| edge.from == shim.id) {
                let consumer = stage.ir.block(second.to).unwrap();
                if consumer.source.is_some_and(|source| source.pc == 0x682) {
                    routes.push((read_block, shim, consumer));
                }
            }
        }
    }
    assert!(
        !routes.is_empty(),
        "actual capture lost the 0x651 -> 0x20c -> 0x682 route"
    );
    for (read_block, shim, consumer) in routes {
        let read = read_block
            .operations
            .iter()
            .find(|operation| operation.opcode == 0x54)
            .unwrap();
        assert_eq!(read.source.unwrap().pc, 0x67c);
        assert!(program.block(shim.id).unwrap().parameters.is_empty());
        assert!(program.block(consumer.id).unwrap().parameters.is_empty());
        let call = program
            .block(consumer.id)
            .unwrap()
            .statements
            .iter()
            .find(|statement| {
                consumer.operations.iter().any(|operation| {
                    operation.id == statement.operation && operation.opcode == 0xf4
                })
            })
            .unwrap();
        assert_eq!(
            value_leaves(program, call.inputs[1]),
            BTreeSet::from([read.outputs[0]])
        );
    }
    for old in [U256::from(0x1234), U256::from(0x9876)] {
        for success in [false, true] {
            let mut host = captured::host(&input, vec![0x12, 0x34, 0x56, 0x78, 9], false);
            let slot = U256::from_str_radix(
                "7050c9e0f4ca769c69bd3a8ef740bc37934f8e2c036e5a723fd8ee048ed3f8c3",
                16,
            )
            .unwrap();
            host.storage.insert(slot, old);
            host.responses.push_back(ExternalResponse {
                result: U256::from(u64::from(success)),
                return_data: vec![0xc0, 0xff, 0xee],
                ..ExternalResponse::default()
            });
            let (raw, observed) = certify(&input, &artifact, host);
            let read = raw
                .trace
                .iter()
                .find(|step| step.pc == Some(0x67c))
                .unwrap();
            assert_eq!(read.outputs, vec![old]);
            let calls: Vec<_> = raw
                .trace
                .iter()
                .filter(|step| step.opcode == 0xf4)
                .collect();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].pc, Some(0x690));
            assert_eq!(calls[0].inputs[1], old);
            assert!(observed.responses.is_empty());
        }
    }
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_eliminated_parameters_keep_transfer_provenance_and_canonical_certificate() {
    let input = copy_chain(4);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let prior = structured(&artifact);
    let mut removed = 0;
    for raw in &stage.ir.blocks {
        let high = program.block(raw.id).unwrap();
        for (slot, parameter) in raw.parameters.iter().enumerate() {
            if high.parameters.contains(parameter) {
                continue;
            }
            removed += 1;
            for edge in stage.ir.edges.iter().filter(|edge| edge.to == raw.id) {
                let transfer = program.transfer(edge.id).unwrap();
                let occurrence = EntityRef {
                    stage: stage.id,
                    entity: transfer.id,
                };
                let origin = artifact.origin(occurrence).unwrap();
                for entity in [*parameter, edge.arguments[slot]] {
                    let ancestor = EntityRef {
                        stage: prior.id,
                        entity,
                    };
                    assert!(
                        origin.parents.contains(&ancestor),
                        "removed copy lost its direct witness {ancestor:?}: {origin:?}"
                    );
                    assert!(artifact.derived_from(ancestor).contains(&occurrence));
                    let source = artifact.origin(ancestor).unwrap();
                    assert!(!source.source.is_empty());
                    assert!(
                        source
                            .source
                            .iter()
                            .any(|span| artifact.descendants(span.pc).contains(&occurrence))
                    );
                }
            }
        }
    }
    assert!(removed >= 4);
    assert!(
        program
            .transfers
            .iter()
            .all(|transfer| transfer.arguments.is_empty())
    );
    let encoded = serde_json::to_vec(&artifact).unwrap();
    let restored: Artifact = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(restored, artifact);
    assert_eq!(serde_json::to_vec(&checked(&input)).unwrap(), encoded);
    assert_eq!(
        validate_stage(restored.current().unwrap()).status,
        ValidationStatus::Passed
    );
    let source = stage.ir.blocks[0]
        .operations
        .iter()
        .find(|operation| operation.opcode == 0x54)
        .unwrap()
        .outputs[0];
    let parameter = stage
        .ir
        .blocks
        .iter()
        .find_map(|block| block.parameters.first())
        .copied()
        .unwrap();
    let mut corrupt = stage.clone();
    let leaf = corrupt
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .expressions
        .iter_mut()
        .find(|expression| {
            expression.kind == Expr::Value(source) && expression.block != stage.ir.blocks[0].id
        })
        .unwrap();
    leaf.kind = Expr::Value(parameter);
    let validation = validate_stage(&corrupt);
    assert_eq!(validation.status, ValidationStatus::Failed);
    assert!(
        validation
            .checks
            .iter()
            .any(|check| check.name == "lifted_semantic_projection" && !check.passed),
        "corrupted canonical leaf escaped semantic certification: {validation:?}"
    );
    certify(&input, &artifact, MemoryHost::new(&input, Vec::new()));
    assert_projection_integrity(&artifact);
}

#[test]
fn positive_long_copy_chain_keeps_bounded_witnesses_and_observations() {
    let input = wide_copy_chain(96, 12);
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    assert!(
        program
            .blocks
            .iter()
            .all(|block| block.parameters.is_empty())
    );
    assert!(
        program
            .transfers
            .iter()
            .all(|transfer| transfer.arguments.is_empty())
    );
    let prior = structured(&artifact);
    for edge in &stage.ir.edges {
        let transfer = program.transfer(edge.id).unwrap();
        let origin = artifact
            .origin(EntityRef {
                stage: stage.id,
                entity: transfer.id,
            })
            .unwrap();
        assert!(
            origin.parents.len() <= 4 * (1 + edge.arguments.len()),
            "copy witness ancestry must use direct arguments rather than flattening the chain"
        );
        assert!(origin.parents.iter().all(|parent| parent.stage == prior.id));
    }
    let mut host = MemoryHost::new(&input, Vec::new());
    for slot in 0_u64..12 {
        host.storage
            .insert(U256::from(slot), U256::from(slot * 17 + 1));
    }
    let (raw, observed) = certify(&input, &artifact, host);
    for slot in 0_u64..12 {
        assert_eq!(
            observed.storage[&U256::from(128 + slot)],
            U256::from(slot * 17 + 1)
        );
    }
    assert_eq!(
        raw.trace.iter().filter(|step| step.opcode == 0x54).count(),
        12
    );
    assert_eq!(
        raw.trace.iter().filter(|step| step.opcode == 0x55).count(),
        12
    );
    assert_projection_integrity(&artifact);
}

#[test]
fn long_copy_chain_refusal_and_cancellation_preserve_atomic_history() {
    // More than a thousand live forwarding slots, below the independent
    // structuring-depth ceiling so the fixture reaches the Lift phase.
    let input = wide_copy_chain(96, 12);
    let before_cancel = Cancellation::new();
    let before = decompile_with_progress(&input, &Config::default(), &before_cancel, |progress| {
        if progress.phase == Phase::Lift {
            before_cancel.cancel();
        }
    });
    assert_eq!(before.current().unwrap().kind, StageKind::Structured);
    let mut lift_start = None;
    let mut checkpoint = None;
    let cancel = Cancellation::new();
    let cancelled = decompile_with_progress(&input, &Config::default(), &cancel, |progress| {
        if progress.phase == Phase::Lift {
            let start = *lift_start.get_or_insert(progress.work);
            if progress.work > start {
                checkpoint = Some(progress.work);
                cancel.cancel();
            }
        }
    });
    let checkpoint =
        checkpoint.expect("a sufficiently large copy chain exposes in-phase work checkpoints");
    assert_eq!(cancelled.completion, Completion::Cancelled);
    assert!(checkpoint > lift_start.unwrap());
    let mut config = Config::default();
    config.limits.max_work = checkpoint;
    let refused = decompile(&input, &config);
    assert_eq!(refused.completion, Completion::Limited);
    assert!(refused.statistics.work <= config.limits.max_work);
    assert!(
        refused.statistics.work > lift_start.unwrap(),
        "exercise refusal after some lifting work"
    );
    for attempt in [&cancelled, &refused] {
        assert_eq!(attempt.current().unwrap().kind, StageKind::Structured);
        assert_eq!(attempt.stages, before.stages);
        assert_eq!(attempt.events, before.events);
        assert_eq!(attempt.mappings, before.mappings);
        assert!(attempt.stages.iter().all(|stage| stage.ir.lifted.is_none()));
        validated(attempt);
    }
}

#[test]
fn positive_interior_copy_cycle_refines_around_retained_true_phi() {
    let mut asm = Asm::default();
    asm.op(0x5f)
        .op(0x35)
        .target("b")
        .op(0x57)
        .dest("a")
        .op(0x5f)
        .op(0x54)
        .target("p")
        .op(0x56)
        .dest("b")
        .push(1)
        .op(0x54)
        .dest("p")
        .push(32)
        .op(0x35)
        .target("r")
        .op(0x57)
        .dest("q");
    // Q increments a memory counter and returns to P on odd counts, otherwise
    // falling into R. R increments it again, then enters Q or exits. The carried
    // observation is the sole stack word at every P/Q/R entry.
    asm.op(0x5f)
        .op(0x51)
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x52)
        .push(1)
        .op(0x16)
        .target("p")
        .op(0x57)
        .dest("r")
        .op(0x5f)
        .op(0x51)
        .push(1)
        .op(0x01)
        .op(0x80)
        .op(0x5f)
        .op(0x52)
        .push(64)
        .op(0x35)
        .op(0x90)
        .op(0x10)
        .target("q")
        .op(0x57)
        .push(2)
        .op(0x55)
        .op(0x00);
    let [a_pc, b_pc, p_pc, q_pc, r_pc] = ["a", "b", "p", "q", "r"].map(|label| asm.labels[label]);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let raw = structured(&artifact);
    assert_eq!(raw.ir.fact_scope, FactScope::CompleteLocalCfg);
    // block_at rejects context clones at each source PC. Exact incoming tuples
    // also reject a recovered/simplified CFG that only needs local q=p folding.
    let [a, b, p, q, r] = [a_pc, b_pc, p_pc, q_pc, r_pc].map(|pc| block_at(raw, pc));
    for block in [p, q, r] {
        assert_eq!(block.parameters.len(), 1, "the copy equation has one word");
    }
    let read = |block: &Block| {
        block
            .operations
            .iter()
            .find(|operation| operation.opcode == 0x54)
            .unwrap()
            .outputs[0]
    };
    let a_value = read(a);
    let b_value = read(b);
    assert_ne!(a_value, b_value);
    let [p_value, q_value, r_value] = [p, q, r].map(|block| block.parameters[0]);
    let incoming = |block: &Block| {
        let edges: Vec<_> = raw
            .ir
            .edges
            .iter()
            .filter(|edge| edge.to == block.id)
            .collect();
        let equations: BTreeSet<_> = edges
            .iter()
            .map(|edge| {
                assert_eq!(edge.arguments.len(), 1);
                (edge.from, edge.arguments[0])
            })
            .collect();
        assert_eq!(
            equations.len(),
            edges.len(),
            "no hidden duplicate incoming edges"
        );
        equations
    };
    assert_eq!(
        incoming(p),
        BTreeSet::from([(a.id, a_value), (b.id, b_value), (q.id, q_value)]),
        "retained boundary p = phi(a,b,q)"
    );
    assert_eq!(
        incoming(q),
        BTreeSet::from([(p.id, p_value), (r.id, r_value)]),
        "interior q = phi(p,r) must not already be a trivial local copy"
    );
    assert_eq!(
        incoming(r),
        BTreeSet::from([(p.id, p_value), (q.id, q_value)]),
        "interior r = phi(p,q) must not already be a trivial local copy"
    );
    assert_eq!(program.block(p.id).unwrap().parameters, vec![p_value]);
    for block in [q, r] {
        assert!(
            program.block(block.id).unwrap().parameters.is_empty(),
            "the interior q/r SCC has the single dominating retained root p"
        );
        for edge in raw.ir.edges.iter().filter(|edge| edge.to == block.id) {
            assert!(program.transfer(edge.id).unwrap().arguments.is_empty());
        }
    }
    assert!(
        program.expressions.iter().any(|expression| {
            expression.block != p.id && expression.kind == Expr::Value(p_value)
        })
    );
    let mut exercised_edges = BTreeSet::new();
    for (a_word, b_word) in [(U256::from(17), U256::from(29)), (U256::MAX, U256::ZERO)] {
        for entry in [0, 1] {
            for route in [0, 1] {
                for iterations in [0, 1, 3, 7] {
                    let mut host = MemoryHost::new(
                        &input,
                        calldata(&[U256::from(entry), U256::from(route), U256::from(iterations)]),
                    );
                    host.storage.insert(U256::ZERO, a_word);
                    host.storage.insert(U256::from(1), b_word);
                    let (execution, observed) = certify(&input, &artifact, host);
                    assert_eq!(
                        observed.storage[&U256::from(2)],
                        if entry == 0 { a_word } else { b_word }
                    );
                    assert_eq!(
                        execution
                            .trace
                            .iter()
                            .filter(|step| step.opcode == 0x54)
                            .count(),
                        1
                    );
                    let visits: Vec<_> = execution
                        .trace
                        .iter()
                        .filter_map(|step| {
                            step.pc
                                .filter(|pc| step.opcode == 0x5b && [p_pc, q_pc, r_pc].contains(pc))
                        })
                        .collect();
                    exercised_edges.extend(visits.windows(2).map(|pair| (pair[0], pair[1])));
                }
            }
        }
    }
    assert_eq!(
        exercised_edges,
        BTreeSet::from([
            (p_pc, q_pc),
            (p_pc, r_pc),
            (q_pc, p_pc),
            (q_pc, r_pc),
            (r_pc, q_pc),
        ])
    );
    assert_projection_integrity(&artifact);
    assert_eq!(stage.ir.fact_scope, FactScope::CompleteLocalCfg);
}

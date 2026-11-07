//! A linear source can have an exponential inline DAG. Named raw bindings
//! bound display expansion without changing primitive positions or Word256 math.
use super::*;
use evm_decompile::{TextExportStatus, export_text};

#[test]
fn repeated_pure_dag_materializes_real_ssa_operations_and_exports_completely() {
    let steps = 24;
    let mut bytes = vec![0x5f, 0x35];
    for _ in 0..steps {
        bytes.extend([0x80, 0x02]);
    }
    bytes.extend([0x5f, 0x52, 0x60, 0x20, 0x5f, 0xf3]);
    let input = Input::new(bytes, Fork::Osaka);
    let artifact = checked(&input);
    assert_unchanged_witness(&artifact);
    let (stage, high) = lifted(&artifact);
    let operations: BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .map(|operation| (operation.id, operation))
        .collect();
    let arithmetic: Vec<_> = high
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| operations[&statement.operation].opcode == 0x02)
        .collect();
    assert!(
        arithmetic.len() >= 2,
        "the 2^24 inline occurrences need actual named computations"
    );
    for statement in &arithmetic {
        let operation = operations[&statement.operation];
        assert_eq!(statement.outputs, operation.outputs);
        assert_eq!(statement.inputs.len(), 2);
        let origin = artifact
            .origin(evm_decompile::EntityRef {
                stage: stage.id,
                entity: statement.id,
            })
            .unwrap();
        assert!(origin.source.contains(&operation.source.unwrap()));
    }
    assert!(
        arithmetic
            .windows(2)
            .all(|pair| operations[&pair[0].operation].source.unwrap().pc
                < operations[&pair[1].operation].source.unwrap().pc)
    );
    let index = high.render_index(&stage.ir);
    for expression in &high.expressions {
        let text = index.render_expression(expression.id, |_| true).unwrap();
        assert!(text.len() < evm_decompile::MAX_LIFTED_EXPRESSION_TEXT);
    }
    let export = export_text(&artifact, &evm_decompile::Cancellation::default());
    assert_eq!(export.status, TextExportStatus::Complete);
    assert!(
        !export.text.contains("expression_limit(") && !export.text.contains("work limit exhausted")
    );
    assert_eq!(export.text.matches("evm.mul(").count(), arithmetic.len());
    for word in [
        U256::ZERO,
        U256::from(1),
        U256::from(2),
        U256::from(255),
        (U256::from(1) << 255) - U256::from(1),
        U256::MAX,
    ] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[word])),
        );
        let expected = (0..steps).fold(word, |value, _| value.wrapping_mul(value));
        assert_eq!(&observed.memory[..32], &expected.to_be_bytes::<32>());
    }
}

fn squares(asm: &mut Asm, steps: usize) {
    for _ in 0..steps {
        asm.op(0x80).op(0x02);
    }
}
fn return_word(asm: &mut Asm) {
    asm.op(0x5f).op(0x52).push(32).op(0x5f).op(0xf3);
}

#[test]
fn materialized_dag_values_cross_a_multi_predecessor_phi_without_reexpansion() {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35);
    squares(&mut asm, 24);
    asm.push(32).op(0x35).target("other").op(0x57);
    asm.push(1).op(0x01).target("join").op(0x56);
    asm.dest("other");
    squares(&mut asm, 12);
    asm.dest("join");
    let join_pc = asm.labels["join"];
    return_word(&mut asm);
    let input = asm.finish();
    let artifact = checked(&input);
    assert_unchanged_witness(&artifact);
    let (stage, program) = lifted(&artifact);
    let join = stage
        .ir
        .blocks
        .iter()
        .find(|block| block.source.is_some_and(|source| source.pc == join_pc))
        .unwrap();
    assert!(
        stage
            .ir
            .edges
            .iter()
            .filter(|edge| edge.to == join.id)
            .count()
            >= 2
    );
    assert!(!program.block(join.id).unwrap().parameters.is_empty());
    assert_eq!(
        export_text(&artifact, &evm_decompile::Cancellation::default()).status,
        TextExportStatus::Complete
    );
    for word in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
        let first = (0..24).fold(word, |value, _| value.wrapping_mul(value));
        for selected in [false, true] {
            let expected = if selected {
                (0..12).fold(first, |value, _| value.wrapping_mul(value))
            } else {
                first.wrapping_add(U256::from(1))
            };
            let (_, observed) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[word, U256::from(u8::from(selected))])),
            );
            assert_eq!(&observed.memory[..32], &expected.to_be_bytes::<32>());
        }
    }
}

#[test]
fn loop_carried_materialized_words_are_rebound_on_each_iteration() {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35);
    squares(&mut asm, 12);
    asm.op(0x5f)
        .dest("header")
        .op(0x80)
        .push(3)
        .op(0x90)
        .op(0x10)
        .target("body")
        .op(0x57);
    asm.op(0x50);
    return_word(&mut asm);
    asm.dest("body").op(0x90);
    squares(&mut asm, 12);
    asm.op(0x90).push(1).op(0x01).target("header").op(0x56);
    let input = asm.finish();
    let artifact = checked(&input);
    assert_unchanged_witness(&artifact);
    let (_, program) = lifted(&artifact);
    assert!(
        program
            .blocks
            .iter()
            .any(|block| block.parameters.len() >= 2)
    );
    assert!(
        program
            .transfers
            .iter()
            .any(|transfer| transfer.arguments.len() >= 2)
    );
    assert_eq!(
        export_text(&artifact, &evm_decompile::Cancellation::default()).status,
        TextExportStatus::Complete
    );
    for word in [U256::ZERO, U256::from(1), U256::from(2), U256::MAX] {
        let (_, observed) = certify(
            &input,
            &artifact,
            MemoryHost::new(&input, calldata(&[word])),
        );
        let expected = (0..48).fold(word, |value, _| value.wrapping_mul(value));
        assert_eq!(&observed.memory[..32], &expected.to_be_bytes::<32>());
    }
}

#[test]
fn seven_large_call_operands_share_the_unchanged_complete_row_budget() {
    let mut asm = Asm::default();
    for slot in 0..7 {
        asm.push(slot * 32).op(0x35);
        squares(&mut asm, 12);
    }
    asm.op(0xf1).op(0x50).op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let operations: BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .map(|operation| (operation.id, operation))
        .collect();
    let call = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| operations[&statement.operation].opcode == 0xf1)
        .unwrap();
    assert_eq!(call.inputs.len(), 7);
    let row = program
        .render_index(&stage.ir)
        .render_statement(call, |_| true)
        .unwrap();
    assert!(row.len() < evm_decompile::MAX_LIFTED_EXPRESSION_TEXT);
    assert_eq!(
        export_text(&artifact, &evm_decompile::Cancellation::default()).status,
        TextExportStatus::Complete
    );
    let mut host = MemoryHost::new(&input, calldata(&[U256::ZERO; 7]));
    host.responses.push_back(ExternalResponse {
        result: U256::from(1),
        ..ExternalResponse::default()
    });
    certify(&input, &artifact, host);
}

#[test]
fn a_materialized_signed_predicate_still_produces_a_zero_or_one_word() {
    let mut asm = Asm::default();
    asm.op(0x5f).op(0x35);
    squares(&mut asm, 7);
    asm.push(32).op(0x35);
    squares(&mut asm, 7);
    asm.op(0x12).push(2).op(0x02);
    return_word(&mut asm);
    let input = asm.finish();
    let artifact = checked(&input);
    let (stage, program) = lifted(&artifact);
    let operation = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|operation| operation.opcode == 0x12)
        .unwrap();
    let statement = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == operation.id)
        .expect("large signed comparison keeps its original Word256 result");
    assert_eq!(statement.outputs, operation.outputs);
    assert!(
        program
            .expressions
            .iter()
            .any(|expression| expression.ty == LiftedType::Word256
                && expression.kind == Expr::Value(operation.outputs[0]))
    );
    assert_eq!(
        export_text(&artifact, &evm_decompile::Cancellation::default()).status,
        TextExportStatus::Complete
    );
    for pair in [
        [U256::ZERO, U256::from(1)],
        [U256::from(1), U256::ZERO],
        [U256::from(2), U256::MAX],
        [U256::MAX, U256::from(2)],
    ] {
        let powers = pair.map(|word| (0..7).fold(word, |value, _| value.wrapping_mul(value)));
        // SLT consumes the top word first. Its result remains an explicit 0/1
        // word when the following MUL consumes it, even after binding it once.
        let expected = U256::from(u8::from(signed_less(powers[1], powers[0]))) * U256::from(2);
        let (_, observed) = certify(&input, &artifact, MemoryHost::new(&input, calldata(&pair)));
        assert_eq!(&observed.memory[..32], &expected.to_be_bytes::<32>());
    }
}

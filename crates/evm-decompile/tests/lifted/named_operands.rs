//! Real bytecode fixes operand order before the presentation layer is involved.
use super::oracle::certify;
use super::support::{Asm, calldata, checked};
use super::{assert_unchanged_witness, lifted, structured, word_cases};
use evm_decompile::{
    Cancellation, EntityRef, ExternalResponse, LiftedExpressionKind, MemoryHost, TextExportStatus,
    U256, export_text, export_text_with_observer, render_text,
};

#[test]
fn actual_cross_block_word_bindings_use_operators_in_original_pop_order() {
    for (opcode, symbol) in [
        (0x01, "+"),
        (0x02, "*"),
        (0x03, "-"),
        (0x04, "/"),
        (0x06, "%"),
        (0x0a, "**"),
        (0x16, "&"),
        (0x17, "|"),
        (0x18, "^"),
        (0x19, "~"),
        (0x1b, "<<"),
        (0x1c, ">>"),
    ] {
        let mut asm = Asm::default();
        if opcode != 0x19 {
            asm.push(32).op(0x35);
        }
        asm.op(0x5f).op(0x35).op(opcode).next_block();
        asm.op(0x5f).op(0x55).op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let before = serde_json::to_vec(&artifact).unwrap();
        let (stage, program) = lifted(&artifact);
        let block = stage
            .ir
            .blocks
            .iter()
            .find(|block| block.operations.iter().any(|op| op.opcode == opcode))
            .unwrap();
        let operation = block
            .operations
            .iter()
            .find(|op| op.opcode == opcode)
            .unwrap();
        let statement = program
            .block(block.id)
            .unwrap()
            .statements
            .iter()
            .find(|statement| statement.operation == operation.id)
            .expect("a word transferred to another block retains its defining operation");
        assert_eq!(statement.outputs, operation.outputs);
        assert!(stage.ir.edges.iter().any(|edge| {
            edge.from == block.id
                && edge.to != block.id
                && edge.arguments.contains(&operation.outputs[0])
        }));
        let origin = artifact
            .origin(EntityRef {
                stage: stage.id,
                entity: statement.id,
            })
            .unwrap();
        assert!(origin.source.contains(&operation.source.unwrap()));
        let index = program.render_index(&stage.ir);
        let args = statement
            .inputs
            .iter()
            .map(|id| index.render_expression(*id, |_| true).unwrap())
            .collect::<Vec<_>>();
        let rhs = if opcode == 0x19 {
            format!("(~{})", args[0])
        } else if matches!(opcode, 0x1b | 0x1c) {
            format!("({} {symbol} {})", args[1], args[0])
        } else {
            format!("({} {symbol} {})", args[0], args[1])
        };
        let row = format!("v{} = {rhs};", statement.outputs[0].0);
        assert_eq!(
            index
                .render_statement_with_dependencies(statement, false, |_| true)
                .unwrap(),
            row
        );
        let export = export_text(&artifact, &Cancellation::new());
        assert_eq!(export.status, TextExportStatus::Complete);
        assert!(export.text.contains(&row), "{}", export.text);
        for (left, right) in [
            (U256::ZERO, U256::ZERO),
            (U256::from(2), U256::from(3)),
            (U256::from(255), U256::from(1) << 255),
            (U256::from(256), U256::MAX),
            (U256::MAX, U256::from(7)),
        ] {
            let (raw, host) = certify(
                &input,
                &artifact,
                MemoryHost::new(&input, calldata(&[left, right])),
            );
            let original = raw.trace.iter().find(|step| step.opcode == opcode).unwrap();
            assert_eq!(original.pc, operation.source.map(|source| source.pc));
            let expected_inputs = if opcode == 0x19 {
                vec![left]
            } else {
                vec![left, right]
            };
            assert_eq!(original.inputs, expected_inputs);
            assert_eq!(host.storage[&U256::ZERO], original.outputs[0]);
        }
        assert_unchanged_witness(&artifact);
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
    }
}

#[test]
fn nested_exp_xor_and_word_not_keep_distinct_parenthesized_syntax() {
    let mut asm = Asm::default();
    asm.push(64)
        .op(0x35)
        .op(0x19)
        .push(32)
        .op(0x35)
        .op(0x5f)
        .op(0x35)
        .op(0x0a)
        .op(0x18)
        .op(0x5f)
        .op(0x55)
        .op(0x00);
    let input = asm.finish();
    let artifact = checked(&input);
    let before = serde_json::to_vec(&artifact).unwrap();
    let (stage, program) = lifted(&artifact);
    let index = program.render_index(&stage.ir);
    let xor = program
        .expressions
        .iter()
        .find(|expression| {
            matches!(
                expression.kind,
                LiftedExpressionKind::WordOp { opcode: 0x18, .. }
            )
        })
        .unwrap();
    let load = |offset: u64| {
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|op| {
                op.opcode == 0x35
                    && stage.ir.value(op.inputs[0]).unwrap().fact.singleton()
                        == Some(U256::from(offset))
            })
            .unwrap();
        format!("v{}", operation.outputs[0].0)
    };
    let expected = format!("(({} ** {}) ^ (~{}))", load(0), load(32), load(64));
    assert_eq!(index.render_expression(xor.id, |_| true).unwrap(), expected);
    let text = render_text(&artifact);
    assert!(text.contains(&expected), "{text}");
    assert!(!text.contains("evm.exp(") && !text.contains("evm.not("));
    for words in [
        [U256::from(2), U256::from(3), U256::from(5)],
        [U256::ZERO, U256::ZERO, U256::MAX],
        [U256::MAX, U256::from(2), U256::ZERO],
    ] {
        let (raw, host) = certify(&input, &artifact, MemoryHost::new(&input, calldata(&words)));
        let result = raw.trace.iter().find(|step| step.opcode == 0x18).unwrap();
        assert_eq!(host.storage[&U256::ZERO], result.outputs[0]);
    }
    assert_unchanged_witness(&artifact);
    assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
}

#[test]
fn retained_unsigned_comparisons_and_iszero_remain_numeric_words() {
    for (opcode, relation) in [(0x10, "<"), (0x11, ">"), (0x14, "=="), (0x15, "== 0")] {
        let mut asm = Asm::default();
        if opcode != 0x15 {
            asm.push(32).op(0x35);
        }
        asm.op(0x5f)
            .op(0x35)
            .op(opcode)
            .next_block()
            .op(0x5f)
            .op(0x55)
            .op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let (stage, program) = lifted(&artifact);
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|op| op.opcode == opcode)
            .unwrap();
        let statement = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| statement.operation == operation.id)
            .unwrap();
        let index = program.render_index(&stage.ir);
        let args = statement
            .inputs
            .iter()
            .map(|id| index.render_expression(*id, |_| true).unwrap())
            .collect::<Vec<_>>();
        let comparison = if opcode == 0x15 {
            format!("{} {relation}", args[0])
        } else {
            format!("{} {relation} {}", args[0], args[1])
        };
        let expected = format!("v{} = word(({comparison}));", statement.outputs[0].0);
        assert_eq!(
            index
                .render_statement_with_dependencies(statement, false, |_| true)
                .unwrap(),
            expected
        );
        for left in word_cases() {
            for right in [U256::ZERO, U256::from(2), U256::MAX] {
                let (_, host) = certify(
                    &input,
                    &artifact,
                    MemoryHost::new(&input, calldata(&[left, right])),
                );
                let expected = match opcode {
                    0x10 => left < right,
                    0x11 => left > right,
                    0x14 => left == right,
                    0x15 => left == U256::ZERO,
                    _ => unreachable!(),
                };
                assert_eq!(host.storage[&U256::ZERO], U256::from(u8::from(expected)));
            }
        }
        assert_unchanged_witness(&artifact);
    }
}

#[test]
fn wide_modular_math_keeps_positional_helpers_and_untruncated_intermediate_results() {
    for (opcode, name) in [(0x08, "addmod"), (0x09, "mulmod")] {
        let mut asm = Asm::default();
        asm.push(64)
            .op(0x35)
            .push(32)
            .op(0x35)
            .op(0x5f)
            .op(0x35)
            .op(opcode)
            .op(0x5f)
            .op(0x55)
            .op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let before = serde_json::to_vec(&artifact).unwrap();
        let (stage, program) = lifted(&artifact);
        let index = program.render_index(&stage.ir);
        let expression = program
            .expressions
            .iter()
            .find(|expression| {
                matches!(expression.kind, LiftedExpressionKind::WordOp { opcode: op, .. } if op == opcode)
            })
            .unwrap();
        let LiftedExpressionKind::WordOp { arguments, .. } = &expression.kind else {
            unreachable!();
        };
        let arguments = arguments
            .iter()
            .map(|id| index.render_expression(*id, |_| true).unwrap())
            .collect::<Vec<_>>()
            .join(", ");
        let expected = format!("evm.{name}({arguments})");
        assert_eq!(
            index.render_expression(expression.id, |_| true).unwrap(),
            expected
        );
        assert!(render_text(&artifact).contains(&expected));
        for (words, expected) in [
            ([U256::MAX, U256::MAX, U256::MAX], U256::ZERO),
            ([U256::MAX, U256::MAX, U256::ZERO], U256::ZERO),
            (
                [U256::from(2), U256::from(3), U256::from(5)],
                U256::from(u8::from(opcode == 0x09)),
            ),
        ] {
            let (_, host) = certify(&input, &artifact, MemoryHost::new(&input, calldata(&words)));
            assert_eq!(host.storage[&U256::ZERO], expected);
        }
        assert_unchanged_witness(&artifact);
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
    }
}

#[test]
fn detailed_load_copy_and_raw_revert_operands_survive_the_operator_policy() {
    let mut asm = Asm::default();
    asm.push(32)
        .op(0x35)
        .op(0x5f)
        .op(0x52)
        .op(0x5f)
        .op(0x51)
        .push(1)
        .op(0x55)
        .push(0xab)
        .push(0x21)
        .op(0x53)
        .push(0x10)
        .push(0x20)
        .push(0x40)
        .op(0x37)
        .push(3)
        .push(1)
        .push(0x60)
        .op(0x39)
        .push(4)
        .push(2)
        .push(0x80)
        .op(0x3e)
        .push(0x10)
        .push(0x40)
        .push(0xa0)
        .op(0x5e)
        .push(3)
        .op(0x5c)
        .push(4)
        .op(0x5d)
        .push(5)
        .op(0x54)
        .push(6)
        .op(0x55)
        .push(0x99)
        .push(0x10)
        .push(0xa0)
        .op(0xa1)
        .push(0x10)
        .push(0xa0)
        .op(0xfd);
    let input = asm.finish();
    let artifact = checked(&input);
    let before = serde_json::to_vec(&artifact).unwrap();
    let export = export_text(&artifact, &Cancellation::new());
    assert_eq!(export.status, TextExportStatus::Complete);
    for expected in [
        "evm.calldataload(offset = 0x20)",
        "evm.mstore(offset = 0x0, value = ",
        "evm.mload(offset = 0x0)",
        "evm.mstore8(offset = 0x21, value = 0xab)",
        "evm.calldatacopy(memory_offset = 0x40, calldata_offset = 0x20, size = 0x10)",
        "evm.codecopy(memory_offset = 0x60, code_offset = 0x1, size = 0x3)",
        "evm.returndatacopy(memory_offset = 0x80, returndata_offset = 0x2, size = 0x4)",
        "evm.mcopy(dest_offset = 0xa0, src_offset = 0x40, size = 0x10)",
        "evm.tload(slot = 0x3)",
        "evm.tstore(slot = 0x4, value = ",
        "evm.sload(slot = 0x5)",
        "evm.sstore(slot = 0x6, value = ",
        "evm.log1(offset = 0xa0, size = 0x10, topic0 = 0x99)",
        "evm.revert(offset = 0xa0, size = 0x10);",
    ] {
        assert!(
            export.text.contains(expected),
            "missing {expected}: {}",
            export.text
        );
    }
    let mut host = MemoryHost::new(&input, calldata(&[U256::from(7), U256::MAX]));
    host.return_data = vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66];
    host.storage.insert(U256::from(5), U256::from(0x1234));
    host.transient.insert(U256::from(3), U256::from(0x5678));
    let (raw, host) = certify(&input, &artifact, host);
    assert_eq!(host.storage[&U256::from(1)], U256::MAX);
    assert_eq!(host.storage[&U256::from(6)], U256::from(0x1234));
    assert_eq!(host.transient[&U256::from(4)], U256::from(0x5678));
    assert_eq!(&host.memory[0xa0..0xb0], &[0xff; 16]);
    let effects = raw
        .trace
        .iter()
        .filter(|step| {
            matches!(
                step.opcode,
                0x35 | 0x37 | 0x39 | 0x3e | 0x51..=0x55 | 0x5c..=0x5e | 0xa1 | 0xfd
            )
        })
        .map(|step| step.opcode)
        .collect::<Vec<_>>();
    assert_eq!(
        effects,
        [
            0x35, 0x52, 0x51, 0x55, 0x53, 0x37, 0x39, 0x3e, 0x5e, 0x5c, 0x5d, 0x54, 0x55, 0xa1,
            0xfd
        ]
    );
    assert_unchanged_witness(&artifact);
    assert_eq!(serde_json::to_vec(&artifact).unwrap(), before);
}

#[test]
fn real_runtime_calls_export_names_without_changing_witness_or_semantics() {
    for (opcode, operands, expected) in [
        (
            0xf1,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.call(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf2,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77][..],
            "evm.callcode(gas = 0x11, target = 0x22, value = 0x33, input_offset = 0x44, input_size = 0x55, output_offset = 0x66, output_size = 0x77);",
        ),
        (
            0xf4,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.delegatecall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
        (
            0xfa,
            &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66][..],
            "evm.staticcall(gas = 0x11, target = 0x22, input_offset = 0x33, input_size = 0x44, output_offset = 0x55, output_size = 0x66);",
        ),
    ] {
        let mut asm = Asm::default();
        for &value in operands.iter().rev() {
            asm.push(value);
        }
        asm.op(opcode).op(0x50).op(0x00);
        let input = asm.finish();
        let artifact = checked(&input);
        let original = artifact.clone();
        let encoded = serde_json::to_vec(&artifact).unwrap();
        let (stage, program) = lifted(&artifact);
        let operation = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .find(|operation| operation.opcode == opcode)
            .unwrap();
        let expected_values: Vec<_> = operands.iter().copied().map(U256::from).collect();
        let witnessed: Vec<_> = operation
            .inputs
            .iter()
            .map(|id| stage.ir.value(*id).unwrap().fact.singleton().unwrap())
            .collect();
        assert_eq!(witnessed, expected_values);
        let statement = program
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .find(|statement| statement.operation == operation.id)
            .unwrap();
        assert_eq!(statement.inputs.len(), expected_values.len());
        assert!(
            program
                .render_statement(statement, &stage.ir)
                .starts_with(expected)
        );
        let export = export_text(&artifact, &Cancellation::new());
        assert_eq!(export.status, TextExportStatus::Complete);
        assert!(export.text.contains(expected), "{}", export.text);

        let mut raw_artifact = artifact.clone();
        raw_artifact.current_stage = Some(structured(&artifact).id);
        let raw = render_text(&raw_artifact);
        let raw_operands = operands
            .iter()
            .map(|value| format!("0x{value:x}"))
            .collect::<Vec<_>>()
            .join(", ");
        let name = operation.name.to_lowercase();
        assert!(raw.contains(&format!("{name}({raw_operands});")), "{raw}");
        assert!(!raw.contains("gas = "), "{raw}");

        let mut host = MemoryHost::new(&input, Vec::new());
        host.responses.push_back(ExternalResponse::default());
        let (execution, observed) = certify(&input, &artifact, host);
        let calls: Vec<_> = execution
            .trace
            .iter()
            .filter(|step| step.opcode == opcode)
            .collect();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].inputs, expected_values);
        assert!(observed.responses.is_empty());
        assert_unchanged_witness(&artifact);
        assert_eq!(artifact, original);
        assert_eq!(serde_json::to_vec(&artifact).unwrap(), encoded);
    }
}

#[test]
fn named_invocation_export_refusal_and_cancellation_are_atomic() {
    let mut asm = Asm::default();
    asm.push(0x22).push(0x11).op(0x55).op(0x00);
    let artifact = checked(&asm.finish());
    let original = artifact.clone();
    let mut checkpoints = Vec::new();
    let complete = export_text_with_observer(&artifact, &Cancellation::new(), u64::MAX, |work| {
        checkpoints.push(work);
    });
    assert_eq!(complete.status, TextExportStatus::Complete);
    assert!(
        complete
            .text
            .contains("evm.sstore(slot = 0x11, value = 0x22);")
    );
    assert!(checkpoints.windows(2).all(|pair| pair[0] <= pair[1]));
    let mut interrupted_after_work = false;
    for allowance in checkpoints {
        let limited = export_text_with_observer(&artifact, &Cancellation::new(), allowance, |_| {});
        if limited.status == TextExportStatus::Complete {
            continue;
        }
        assert_eq!(limited.status, TextExportStatus::Limited);
        assert!(limited.work <= allowance);
        if !limited.text.contains("evm.sstore(") {
            interrupted_after_work |= limited.work > 0;
        } else {
            assert!(
                limited
                    .text
                    .contains("evm.sstore(slot = 0x11, value = 0x22);"),
                "{}",
                limited.text
            );
        }
        let cancel = Cancellation::new();
        let cancelled = export_text_with_observer(&artifact, &cancel, u64::MAX, |work| {
            if work >= allowance {
                cancel.cancel();
            }
        });
        assert!(cancel.is_cancelled());
        assert_eq!(cancelled.status, TextExportStatus::Cancelled);
        if cancelled.text.contains("evm.sstore(") {
            assert!(
                cancelled
                    .text
                    .contains("evm.sstore(slot = 0x11, value = 0x22);"),
                "{}",
                cancelled.text
            );
        }
    }
    assert!(
        interrupted_after_work,
        "exercise refusal after export has started"
    );
    assert_eq!(artifact, original);
}

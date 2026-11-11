//! Renderer-only views over the independently tested raw call-source fixtures.
use super::{E, source};
use crate::{
    EntityId, Ir, LiftedBlock, LiftedExpression, LiftedExpressionKind as X, LiftedProgram,
    LiftedStatement, LiftedType, StructuredKind as K, StructuredNode, StructuredProgram, ValueKind,
};
fn id(n: u64) -> EntityId {
    EntityId(n)
}
fn view(opcodes: &[u8]) -> (Ir, Vec<EntityId>) {
    let (mut ir, ops) = source::tests::fixture(opcodes);
    for block in &mut ir.blocks {
        for op in &mut block.operations {
            op.name = match op.opcode {
                0xf1 => "call",
                0xf2 => "callcode",
                0xf4 => "delegatecall",
                0xfa => "staticcall",
                0xf0 => "create",
                0xf5 => "create2",
                0x3d => "returndatasize",
                0x3e => "returndatacopy",
                0x5a => "gas",
                _ => unreachable!(),
            }
            .into();
        }
    }
    let mut next = 1_000_000_u64;
    let mut fresh = || {
        next += 1;
        id(next)
    };
    let mut high = LiftedProgram::default();
    for block in &ir.blocks {
        let mut statements = Vec::new();
        for op in &block.operations {
            let mut inputs = Vec::new();
            for raw in &op.inputs {
                let kind = match ir.value(*raw).map(|v| &v.kind) {
                    Some(ValueKind::Constant(word)) => X::Word(*word),
                    _ => X::Value(*raw),
                };
                let expr = fresh();
                high.expressions.push(LiftedExpression {
                    id: expr,
                    block: block.id,
                    ty: LiftedType::Word256,
                    kind,
                });
                inputs.push(expr);
            }
            statements.push(LiftedStatement {
                id: fresh(),
                operation: op.id,
                inputs,
                outputs: op.outputs.clone(),
            });
            for raw in &op.outputs {
                high.expressions.push(LiftedExpression {
                    id: fresh(),
                    block: block.id,
                    ty: LiftedType::Word256,
                    kind: X::Value(*raw),
                });
            }
        }
        high.blocks.push(LiftedBlock {
            id: fresh(),
            block: block.id,
            parameters: vec![],
            statements,
            condition: None,
            target: None,
            exit_values: vec![],
        });
    }
    let children: Vec<_> = ir
        .blocks
        .iter()
        .map(|b| StructuredNode {
            id: fresh(),
            kind: K::Block { block: b.id },
        })
        .collect();
    ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: fresh(),
            kind: K::Sequence(children),
        },
    });
    ir.lifted = Some(high);
    (ir, ops)
}
fn statement(ir: &Ir, op: EntityId) -> &LiftedStatement {
    ir.lifted
        .as_ref()
        .unwrap()
        .blocks
        .iter()
        .flat_map(|b| &b.statements)
        .find(|s| s.operation == op)
        .unwrap()
}
fn value_expression(ir: &Ir, value: EntityId) -> EntityId {
    ir.lifted
        .as_ref()
        .unwrap()
        .expressions
        .iter()
        .find(|e| e.kind == X::Value(value))
        .unwrap()
        .id
}

#[test]
fn paired_word_result_and_consumed_size_are_one_unchanged_source_plan() {
    let (ir, ops) = view(&[0xf4, 0x3d]);
    let before = serde_json::to_vec(&ir).unwrap();
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    let call = statement(&ir, ops[0]);
    let size = statement(&ir, ops[1]);
    let text = index
        .render_statement_with_dependencies(call, false, |_| true)
        .unwrap();
    assert!(
        text.starts_with("(success1, returndata1) = evm.delegatecall("),
        "{text}"
    );
    assert_eq!(
        index
            .render_expression(value_expression(&ir, call.outputs[0]), |_| true)
            .unwrap(),
        "success1"
    );
    assert_eq!(
        index
            .render_expression(value_expression(&ir, size.outputs[0]), |_| true)
            .unwrap(),
        "returndata1.size"
    );
    assert!(
        index
            .render_statement_row_with_dependencies(size, false, |_| true)
            .unwrap()
            .is_none()
    );
    assert!(
        index
            .render_statement_with_dependencies(size, false, |_| true)
            .unwrap()
            .contains("returndata1.size")
    );
    assert_eq!(index.returndata_source(ops[1]), Some(ops[0]));
    assert_eq!(serde_json::to_vec(&ir).unwrap(), before);
}

#[test]
fn actual_outline_visibility_and_refusal_never_leave_buffer_aliases_behind() {
    let (ir, ops) = view(&[0xf4, 0x3d]);
    let high = ir.lifted.as_ref().unwrap();
    let producer = match &ir.structured.as_ref().unwrap().root.kind {
        K::Sequence(children) => children[0].id,
        _ => unreachable!(),
    };
    let block = ir.blocks[0].id;
    let mut index = high.render_index(&ir);
    assert_eq!(
        index
            .returndata_name(ops[0], block, |_| true)
            .unwrap()
            .as_deref(),
        Some("returndata1")
    );
    index
        .set_call_result_projection(|source| source == producer, |_| true)
        .unwrap();
    assert_eq!(
        index.returndata_name(ops[0], block, |_| true).unwrap(),
        None
    );
    let text = index
        .render_statement_row_with_dependencies(statement(&ir, ops[1]), false, |_| true)
        .unwrap()
        .unwrap();
    assert!(text.contains("evm.returndatasize()"), "{text}");
    for limit in [0, 1, 3, 6] {
        let mut index = high.render_index(&ir);
        let mut payments = 0;
        let result = index.set_call_result_projection(
            |source| source == producer,
            |_| {
                payments += 1;
                payments <= limit
            },
        );
        assert_eq!(result, Err(E::Limit));
        assert_eq!(
            index.returndata_name(ops[0], block, |_| true).unwrap(),
            None
        );
        let text = index
            .render_statement_row_with_dependencies(statement(&ir, ops[1]), false, |_| true)
            .unwrap()
            .unwrap();
        assert!(
            text.contains("evm.returndatasize()") && !text.contains("returndata1"),
            "{text}"
        );
    }
}

#[test]
fn a_forged_earlier_consumer_cannot_reference_a_future_buffer() {
    let (mut ir, ops) = view(&[0x5a, 0xf4, 0x3d]);
    let raw_size = statement(&ir, ops[2]).outputs[0];
    let expression = value_expression(&ir, raw_size);
    ir.lifted.as_mut().unwrap().blocks[0].statements[0]
        .inputs
        .push(expression);
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    let before = index
        .render_statement_with_dependencies(statement(&ir, ops[0]), false, |_| true)
        .unwrap();
    assert!(!before.contains("returndata1"), "{before}");
    assert!(
        index
            .render_statement_row_with_dependencies(statement(&ir, ops[2]), false, |_| true)
            .unwrap()
            .is_some(),
        "keep the numeric fallback binding"
    );
}

#[test]
fn unused_buffers_and_address_results_do_not_acquire_success_names() {
    let (ir, ops) = view(&[0xf4]);
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    let text = index
        .render_statement(statement(&ir, ops[0]), |_| true)
        .unwrap();
    assert!(
        !text.contains("returndata") && !text.contains("success"),
        "{text}"
    );
    let (ir, ops) = view(&[0xf0, 0x3d]);
    let index = ir.lifted.as_ref().unwrap().render_index(&ir);
    let text = index
        .render_statement(statement(&ir, ops[0]), |_| true)
        .unwrap();
    assert!(
        !text.contains("returndata") && !text.contains("success"),
        "{text}"
    );
    assert!(
        index
            .render_statement_row_with_dependencies(statement(&ir, ops[1]), false, |_| true)
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_data_record_cannot_relabel_a_copy_with_a_later_call_epoch() {
    let (ir, ops) = view(&[0xf4, 0x3e, 0xf4, 0x3d]);
    let mut high = ir.lifted.as_ref().unwrap().clone();
    let data = crate::LiftedRevertData {
        id: id(2_000_001),
        execution_domain: crate::ExecutionDomain::Generic,
        execution_fork: None,
        code_hash: None,
        block: ir.blocks[0].id,
        operation: id(2_000_002),
        memory: ir.blocks[0].memory_parameter,
        offset: ir.blocks[0].operations[1].inputs[0],
        size: ir.blocks[0].operations[1].inputs[2],
        content: crate::LiftedRevertDataKind::Segments(vec![crate::LiftedByteSegment::Slice {
            source: crate::LiftedByteSource::ReturnData(ops[2]),
            offset: crate::LiftedDataWord::Constant(crate::U256::ZERO),
            size: crate::LiftedDataWord::Constant(crate::U256::from(32)),
        }]),
        writes: vec![ops[1]],
    };
    // Exercise the derived planner independently of the stronger data replay:
    // a supplied record is not sufficient to name a different source buffer.
    high.revert_data.push(data.clone());
    let plan = super::CallResults::new(&ir, &high, &mut |_| true).unwrap();
    assert_eq!(plan.origin(ops[1]), Some(ops[0]));
    assert!(plan.data_source(&data, ops[0]));
    assert!(!plan.data_source(&data, ops[2]));
    assert!(plan.terminal_uses.is_empty());
}

use super::*;

fn shared_call() -> String {
    let mut runtime = guards::Runtime::default();
    runtime.emit(&[0x5f, 0x5f, 0x5f, 0x5f, 0x60, 1, 0x5a, 0xf4, 0x60, 1, 0x55]);
    runtime.address("first");
    runtime.jump("helper", false);
    runtime.label("first");
    runtime.emit(&[0x5f, 0x52]);
    runtime.emit(&[0x5f, 0x5f, 0x5f, 0x5f, 0x60, 2, 0x5a, 0xf4, 0x60, 2, 0x55]);
    runtime.address("second");
    runtime.jump("helper", false);
    runtime.label("second");
    runtime.emit(&[0x60, 0x20, 0x52, 0x00]);
    runtime.label("helper");
    runtime.emit(&[
        0x3d, 0x80, 0x60, 0x40, 0x52, 0x80, 0x60, 3, 0x55, 0x60, 0x20, 0x60, 0x40, 0xa0, 0x60, 5,
        0x54, 0x60, 1, 0x01, 0x60, 5, 0x55, 0x90, 0x56,
    ]);
    runtime.finish()
}

#[test]
fn shared_word_interfaces_keep_raw_size_bindings_without_exporting_a_buffer() {
    let artifact = artifact_with_readability(
        &shared_call(),
        ReadabilityConfig {
            procedures: ProcedurePreference::Shared,
            ..Default::default()
        },
    );
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let index = StageIndex::new(&stage.ir);
    let renderer = program.render_index(&stage.ir);
    assert!(!program.procedures.is_empty());
    assert!(index.shared.error().is_none(), "{:?}", index.shared.error());
    let normal = high(stage, false);
    let text = copied_text(&normal);
    assert_eq!(text.matches("evm.delegatecall(").count(), 2, "{text}");
    let call = normal
        .iter()
        .find(|line| line.text.contains("evm.delegatecall("))
        .unwrap();
    let statement = index.statement(&stage.ir, call.entity).unwrap();
    assert!(renderer.call_result(statement.operation).is_some());
    let details = crate::decompile::details::build(
        stage,
        &index,
        statement.id,
        &artifact.input.runtime_bytecode,
    );
    assert!(details.lines[0].contains(&call.text), "{:?}", details.lines);
    assert!(
        !text.contains("returndata1") && !text.contains("returndata2"),
        "{text}"
    );
    let retained = normal
        .iter()
        .find(|line| line.text.contains("evm.returndatasize("))
        .unwrap();
    let reader = index.statement(&stage.ir, retained.entity).unwrap();
    let details = crate::decompile::details::build(
        stage,
        &index,
        reader.id,
        &artifact.input.runtime_bytecode,
    );
    assert!(
        details.lines[0].contains(&retained.text),
        "{:?}",
        details.lines
    );
    let value = reader.outputs[0];
    assert!(
        text.contains(&format!("v{} = evm.returndatasize();", value.0)),
        "{text}"
    );
    assert!(
        normal
            .iter()
            .any(|line| line.text.starts_with("yield (")
                && line.text.contains(&format!("v{}", value.0))),
        "{text}"
    );
    for invocation in &program.invocations {
        let expanded = structured::lifted_lines_expanded(
            stage,
            &index,
            Default::default(),
            &BTreeSet::from([invocation.id]),
        );
        let expanded = copied_text(&expanded);
        assert!(
            !expanded.contains("unavailable_") && !expanded.contains("returndata(op"),
            "{expanded}"
        );
    }
}

#[test]
fn hidden_size_rows_participate_in_typed_branch_layout_without_empty_arms() {
    let mut runtime = guards::Runtime::default();
    runtime.emit(&[
        0x5f, 0x5f, 0x5f, 0x5f, 0x60, 1, 0x5a, 0xf4, 0x50, 0x3d, 0x5f, 0x5f, 0x3e, 0x5f, 0x35,
    ]);
    runtime.jump("write", true);
    runtime.emit(&[0x3d, 0x50]);
    runtime.jump("end", false);
    runtime.label("write");
    runtime.emit(&[0x60, 0x2a, 0x5f, 0x52]);
    runtime.label("end");
    runtime.emit(&[0x00]);
    let artifact = inline(&runtime.finish());
    let stage = artifact.current().unwrap();
    let text = copied_text(&high(stage, false));
    assert!(
        text.contains("evm.mstore(") && !text.contains("} else {"),
        "{text}"
    );
    assert!(!text.contains("evm.returndatasize("), "{text}");
    assert!(copied_text(&structured::lines(stage)).contains("evm.returndatasize("));
}

#[test]
fn refused_call_projection_keeps_all_primitive_rows_and_inspector_source_links() {
    let artifact = inline(PRECOPY);
    let stage = artifact.current().unwrap();
    let index = StageIndex::with_render_work_limit(&stage.ir, 0);
    let original = structured::lines(stage);
    assert_eq!(
        rows(&structured::lifted_lines_indexed(
            stage,
            &index,
            Default::default()
        )),
        rows(&original)
    );
    let reader = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .find(|op| op.opcode == 0x3d)
        .unwrap();
    let statement = stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| statement.operation == reader.id)
        .unwrap();
    let details = crate::decompile::details::build(
        stage,
        &index,
        statement.id,
        &artifact.input.runtime_bytecode,
    );
    assert!(details.lines[0].contains("unavailable_statement"));
    assert!(
        !details
            .lines
            .iter()
            .any(|line| line.contains("returndata1.size"))
    );
    assert!(details.references.contains(&EntityRef {
        stage: stage.id,
        entity: reader.id
    }));
}

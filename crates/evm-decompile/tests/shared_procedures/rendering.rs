//! Display decisions retain the certified source body's control and effects.
use super::{boundaries, checked, fixtures, oracle};
use evm_decompile::{ExecutionLimits, MemoryHost, StageKind, StructuredKind, U256};
use std::collections::BTreeSet;

#[test]
fn shared_token_only_else_is_hidden_without_state_dependencies() {
    let fixture = fixtures::token_only_else();
    // The browser case uses these same bytes through the real HTTP boundary.
    assert_eq!(
        alloy_primitives::hex::encode(&fixture.input.runtime_bytecode),
        concat!(
            "61000a60003561001d565b60015561001860203561001d565b600255005b80604052",
            "806000558061002f57610041565b8060605260206040a080600355806004555b6001019056"
        )
    );
    let artifact = checked(&fixture.input);
    let stage = artifact.current().unwrap();
    let high = stage.ir.lifted.as_ref().unwrap();
    let index = high.render_index(&stage.ir);
    assert_eq!(high.procedures.len(), 1);
    assert_eq!(high.invocations.len(), 2);
    let procedure = &high.procedures[0];
    let node = index.node(procedure.body).unwrap();
    let StructuredKind::If {
        then_branch,
        else_branch,
        ..
    } = &node.kind
    else {
        panic!("fixture must certify the If itself, not merely repeated blocks")
    };
    let mut hidden_sources = BTreeSet::new();
    let mut tokens = Vec::new();
    for child in boundaries::nodes(else_branch) {
        match child.kind {
            StructuredKind::Sequence(_) => {}
            StructuredKind::Block { block } => {
                assert!(high.block(block).unwrap().statements.is_empty());
                hidden_sources.insert(child.id);
            }
            StructuredKind::Transfer { edge } => {
                assert!(high.transfer(edge).unwrap().arguments.is_empty());
                let raw = stage.ir.edge(edge).unwrap();
                let target = stage.ir.block(raw.to).unwrap();
                hidden_sources.insert(edge);
                tokens.extend([
                    (
                        edge,
                        format!(
                            "m{} = m{}; // memory token on e{}",
                            target.memory_parameter.0, raw.memory.0, edge.0
                        ),
                    ),
                    (
                        edge,
                        format!(
                            "q{} = q{}; // effect token on e{}",
                            target.effect_parameter.0, raw.effect.0, edge.0
                        ),
                    ),
                ]);
            }
            ref kind => panic!("unexpected substantive false arm: {kind:?}"),
        }
    }
    assert_eq!(tokens.len(), 4, "both raw false-arm edges carry state");
    let hidden = index
        .render_procedure_body(procedure, false, |_| true)
        .unwrap();
    let shown = index
        .render_procedure_body(procedure, true, |_| true)
        .unwrap();
    assert!(hidden.iter().all(|row| row.text != "} else {"));
    assert!(
        hidden
            .iter()
            .all(|row| !hidden_sources.contains(&row.entity)),
        "an omitted else must also omit its contextual annotations"
    );
    let if_rows: Vec<_> = hidden.iter().filter(|row| row.entity == node.id).collect();
    assert_eq!(if_rows.len(), 2, "retain the whole If and its condition");
    assert!(if_rows[0].text.starts_with("if ("));
    assert_eq!(if_rows[0].depth, 0);
    assert_eq!(if_rows[1].text, "}");
    assert_eq!(if_rows[1].depth, 0);
    assert!(
        shown
            .iter()
            .any(|row| { row.entity == node.id && row.text == "} else {" && row.depth == 0 })
    );
    for (edge, token) in tokens {
        assert!(
            shown
                .iter()
                .any(|row| { row.entity == edge && row.text == token && row.depth == 1 })
        );
    }
    let input = procedure.interface.inputs[0].0;
    let expected = [
        format!("mstore(offset = 0x60, value = v{input});"),
        "log0(offset = 0x40, size = 0x20);".to_owned(),
        format!("sstore(slot = 0x3, value = v{input});"),
        format!("sstore(slot = 0x4, value = v{input});"),
    ];
    let operations: Vec<_> = boundaries::nodes(then_branch)
        .iter()
        .filter_map(|node| match node.kind {
            StructuredKind::Block { block } => Some(block),
            _ => None,
        })
        .flat_map(|block| &high.block(block).unwrap().statements)
        .map(|statement| statement.operation)
        .collect();
    assert_eq!(operations.len(), expected.len());
    let visible: Vec<_> = hidden
        .iter()
        .filter(|row| operations.contains(&row.entity))
        .collect();
    assert_eq!(
        visible.iter().map(|row| &row.text).collect::<Vec<_>>(),
        expected.iter().collect::<Vec<_>>()
    );
    for (row, operation) in visible.iter().zip(&operations) {
        assert_eq!(row.entity, *operation);
        assert_eq!(row.depth, 1);
        assert!(!row.is_structure_annotation);
        let detailed = shown
            .iter()
            .find(|shown| shown.entity == *operation)
            .unwrap();
        assert_eq!(detailed.depth, row.depth);
        assert!(detailed.text.starts_with(&row.text));
    }
    let mut raw = stage.ir.clone();
    raw.lifted = None;
    let source = artifact
        .stages
        .iter()
        .find(|stage| stage.kind == StageKind::Structured)
        .unwrap();
    assert_eq!(
        raw, source.ir,
        "rendering preserves the raw Structured witness"
    );
}

#[test]
fn shared_else_display_retains_observable_effects_and_actual_branch_execution() {
    for observable_else in [false, true] {
        let fixture = if observable_else {
            fixtures::observable_else()
        } else {
            fixtures::token_only_else()
        };
        let artifact = checked(&fixture.input);
        let stage = artifact.current().unwrap();
        let high = stage.ir.lifted.as_ref().unwrap();
        let index = high.render_index(&stage.ir);
        assert_eq!(high.procedures.len(), 1);
        for dependencies in [false, true] {
            let rows = index
                .render_procedure_body(&high.procedures[0], dependencies, |_| true)
                .unwrap();
            assert_eq!(
                rows.iter().filter(|row| row.text == "} else {").count(),
                usize::from(observable_else || dependencies)
            );
            if observable_else {
                assert!(
                    rows.iter()
                        .any(|row| row.text.starts_with("sstore(slot = 0x5,"))
                );
            }
        }
        for arguments in [[0, 7], [7, 0], [0, 0], [4, 9]] {
            let report = oracle::certify(
                &fixture.input,
                &artifact,
                MemoryHost::new(&fixture.input, fixtures::words(&arguments.map(U256::from))),
                &ExecutionLimits::default(),
            );
            assert_eq!(
                report.calls.len(),
                2,
                "both real calls execute the definition"
            );
            assert_eq!(
                report.host.state.storage[&U256::from(1)],
                U256::from(arguments[0] + 1)
            );
            assert_eq!(
                report.host.state.storage[&U256::from(2)],
                U256::from(arguments[1] + 1)
            );
            assert_eq!(
                report.host.state.logs.len(),
                arguments.iter().filter(|word| **word != 0).count()
            );
            if observable_else && arguments.contains(&0) {
                assert_eq!(report.host.state.storage[&U256::from(5)], U256::ZERO);
            }
        }
    }
}

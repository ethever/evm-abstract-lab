//! Display decisions retain the certified source body's control and effects.
use super::{boundaries, checked, fixtures, oracle};
use evm_decompile::{ExecutionLimits, MemoryHost, StageKind, StructuredKind, U256};
use std::collections::BTreeSet;

#[test]
fn terminal_names_stay_visible_in_callers_and_never_enter_shared_bodies() {
    for (opcode, name, implicit) in [
        (0x00, "stop", false),
        (0xf3, "return", false),
        (0xfd, "revert", false),
        (0xff, "selfdestruct", false),
        (0x00, "stop", true),
    ] {
        let mut fixture = fixtures::repeated_tail(opcode);
        if implicit {
            assert_eq!(fixture.input.runtime_bytecode.pop(), Some(0x00));
        }
        let artifact = checked(&fixture.input);
        let original = artifact.clone();
        let ir = &artifact.current().unwrap().ir;
        let program = ir.lifted.as_ref().unwrap();
        let renderer = program.render_index(ir);
        assert!(
            !program.procedures.is_empty(),
            "the nonterminal repeated prefix still shares"
        );
        let mut ordinary = artifact.clone();
        let high = ordinary
            .stages
            .last_mut()
            .unwrap()
            .ir
            .lifted
            .as_mut()
            .unwrap();
        high.procedures.clear();
        high.invocations.clear();
        let source = evm_decompile::render_text(&ordinary);
        let exported = evm_decompile::render_text(&artifact);
        let prefix = format!("evm.{name}(");
        assert!(
            exported.contains(&prefix),
            "terminal disappeared: {name}, implicit={implicit}"
        );
        assert_eq!(
            exported.matches(&prefix).count(),
            source.matches(&prefix).count()
        );
        assert!(!exported.contains("evm.evm."));
        for procedure in &program.procedures {
            if let Some(root) = renderer.node(procedure.body) {
                assert!(boundaries::nodes(root).iter().all(|node| !matches!(
                    node.kind,
                    StructuredKind::Exit { .. } | StructuredKind::PartialExit { .. }
                )));
            }
            for dependencies in [false, true] {
                let rows = renderer
                    .render_procedure_body(procedure, dependencies, |_| true)
                    .unwrap();
                assert!(
                    rows.iter().all(|row| !row.text.contains(&prefix)),
                    "terminal entered procedure: {rows:#?}"
                );
                assert!(rows.iter().all(|row| !row.text.contains("evm.evm.")));
            }
        }
        for line in exported
            .lines()
            .filter(|line| line.trim_start().starts_with(&prefix))
        {
            assert!(
                source.contains(line.trim_start()),
                "ordinary/shared terminal style differs: {line}"
            );
        }
        assert_eq!(artifact, original);
    }
}

#[test]
fn shared_token_only_else_keeps_one_layout_across_dependency_modes() {
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
    assert!(shown.iter().all(|row| row.text != "} else {"));
    assert!(
        shown
            .iter()
            .all(|row| !hidden_sources.contains(&row.entity))
    );
    for (edge, token) in tokens {
        assert!(
            shown
                .iter()
                .all(|row| row.entity != edge || row.text != token),
            "context-only rows cannot restore an omitted source arm"
        );
        assert!(
            stage.ir.edge(edge).is_some(),
            "the original state transfer remains inspectable"
        );
    }
    let input = procedure.interface.inputs[0].0;
    let expected = [
        format!("evm.mstore(offset = 0x60, value = v{input});"),
        "evm.log0(offset = 0x40, size = 0x20);".to_owned(),
        format!("evm.sstore(slot = 0x3, value = v{input});"),
        format!("evm.sstore(slot = 0x4, value = v{input});"),
    ];
    let operations: Vec<_> = boundaries::nodes(then_branch)
        .iter()
        .filter_map(|node| match node.kind {
            StructuredKind::Block { block } => Some(block),
            _ => None,
        })
        .flat_map(|block| &high.block(block).unwrap().statements)
        .map(|statement| statement.id)
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
                usize::from(observable_else)
            );
            if observable_else {
                assert!(
                    rows.iter()
                        .any(|row| row.text.starts_with("evm.sstore(slot = 0x5,"))
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

#[test]
fn failed_if_stays_in_each_caller_while_successful_procedures_execute_fresh() {
    let code = alloy_primitives::hex::decode(concat!(
        "6100095f3561001c565b60015561001760203561001c565b600255005b8061004257",
        "80604052805f5560206040a060055460010160055560065460075560010190565b",
        "604051602a905260205ffd"
    ))
    .unwrap();
    let input = evm_decompile::Input::new(code, evm_decompile::Fork::Osaka);
    let artifact = checked(&input);
    let ir = &artifact.current().unwrap().ir;
    let high = ir.lifted.as_ref().unwrap();
    let index = high.render_index(ir);
    assert!(!high.procedures.is_empty());
    let failed: BTreeSet<_> = high
        .blocks
        .iter()
        .filter(|block| {
            ir.block(block.block)
                .unwrap()
                .operations
                .iter()
                .any(|operation| operation.opcode == 0xfd)
        })
        .flat_map(|block| &block.statements)
        .map(|statement| statement.id)
        .collect();
    for procedure in &high.procedures {
        let rows = index
            .render_procedure_body(procedure, false, |_| true)
            .unwrap();
        assert!(rows.iter().all(|row| !failed.contains(&row.entity)
            && !row.text.contains("evm.revert(")
            && !row.text.contains("require(")));
    }
    let text = evm_decompile::render_text(&artifact);
    assert_eq!(text.matches("evm.revert(offset =").count(), 2);
    assert_eq!(text.matches("evm.mload(").count(), 2);
    assert!(!text.contains("require("));
    let successful: BTreeSet<_> = high
        .procedures
        .iter()
        .filter(|procedure| {
            index
                .render_procedure_body(procedure, false, |_| true)
                .unwrap()
                .iter()
                .any(|row| row.text.contains("evm.log0("))
        })
        .map(|procedure| procedure.id)
        .collect();
    assert!(!successful.is_empty());
    for arguments in [[0, 0], [0, 1], [1, 0], [1, 1]] {
        let report = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, fixtures::words(&arguments.map(U256::from))),
            &ExecutionLimits::default(),
        );
        let expected_calls = if arguments[0] != 0 {
            0
        } else if arguments[1] != 0 {
            1
        } else {
            2
        };
        assert_eq!(
            report
                .calls
                .iter()
                .filter(|id| {
                    high.invocations
                        .iter()
                        .any(|call| call.id == **id && successful.contains(&call.procedure))
                })
                .count(),
            expected_calls,
            "only reached successful source occurrences invoke a procedure"
        );
        assert!(
            !report.planned.is_empty(),
            "the independent machine must execute the selected failure-first control plan"
        );
        assert!(
            matches!(report.outcome.unwrap(), evm_decompile::ExecutionOutcome::Exit { kind, .. }
            if kind == if arguments == [0, 0] { evm_decompile::ExitKind::Stop } else { evm_decompile::ExitKind::Revert })
        );
    }
}

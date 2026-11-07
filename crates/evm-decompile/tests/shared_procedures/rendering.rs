//! Display decisions retain the certified source body's control and effects.
use super::{boundaries, checked, fixtures, oracle};
use evm_decompile::{ExecutionLimits, MemoryHost, StageKind, StructuredKind, U256};
use std::collections::BTreeSet;

#[test]
fn terminal_names_match_shared_bodies_and_ordinary_exports() {
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
        let exported = evm_decompile::render_text(&ordinary);
        let prefix = format!("evm.{name}(");
        let mut checked_bodies = 0;
        for procedure in &program.procedures {
            if !boundaries::nodes(renderer.node(procedure.body).unwrap()).iter().any(|node| {
                matches!(node.kind, StructuredKind::Exit { block }
                    if ir.block(block).unwrap().source.is_some_and(|source| source.pc == fixture.labels["tail"]))
            }) {
                continue;
            }
            for dependencies in [false, true] {
                let rows = renderer
                    .render_procedure_body(procedure, dependencies, |_| true)
                    .unwrap();
                let terminals: Vec<_> = rows
                    .iter()
                    .filter(|row| row.text.starts_with(&prefix))
                    .collect();
                assert_eq!(
                    terminals.len(),
                    1,
                    "terminal must print once: {name}, implicit={implicit}"
                );
                let invocation = terminals[0].text.split(" //").next().unwrap();
                assert!(
                    exported.contains(invocation),
                    "ordinary/shared spelling differs: {invocation}\n{exported}"
                );
                assert!(!rows.iter().any(|row| row.text.contains("evm.evm.")));
                assert!(
                    !rows
                        .iter()
                        .any(|row| row.text.starts_with(&format!("{name}(")))
                );
            }
            checked_bodies += 1;
        }
        assert!(
            checked_bodies > 0,
            "fixture must put {name} inside a certified shared body"
        );
        assert_eq!(artifact, original);
    }
}

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
                usize::from(observable_else || dependencies)
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
fn shared_failed_if_reorders_the_canonical_body_and_executes_each_binding() {
    // Same fixture is exercised through the browser renderer and HTTP smoke.
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
    let procedure = high
        .procedures
        .iter()
        .find(|p| matches!(index.node(p.body).unwrap().kind, StructuredKind::If { .. }))
        .expect("failure If is a real canonical shared definition");
    let rows = index
        .render_procedure_body(procedure, false, |_| true)
        .unwrap();
    assert!(
        rows.iter()
            .any(|r| r.depth == 0 && r.text.starts_with("if ("))
    );
    assert!(
        !rows
            .iter()
            .any(|r| r.text.contains("require(") || r.text == "} else {")
    );
    let failed: Vec<_> = rows
        .iter()
        .filter(|r| r.depth == 1 && !r.is_structure_annotation)
        .collect();
    for primitive in ["evm.mload(", "evm.mstore(", "evm.revert("] {
        assert_eq!(
            failed.iter().filter(|r| r.text.contains(primitive)).count(),
            1,
            "{rows:#?}"
        );
    }
    assert!(
        failed
            .windows(2)
            .all(|pair| pair[0].entity != pair[1].entity)
    );
    for arguments in [[0, 0], [0, 1], [1, 0], [1, 1]] {
        let report = oracle::certify(
            &input,
            &artifact,
            MemoryHost::new(&input, fixtures::words(&arguments.map(U256::from))),
            &ExecutionLimits::default(),
        );
        assert!(
            !report.planned.is_empty(),
            "execute the new plan, rather than merely the old source oracle"
        );
        assert!(
            matches!(report.outcome.unwrap(), evm_decompile::ExecutionOutcome::Exit { kind, .. }
            if kind == if arguments == [0, 0] { evm_decompile::ExitKind::Stop } else { evm_decompile::ExitKind::Revert })
        );
    }
}

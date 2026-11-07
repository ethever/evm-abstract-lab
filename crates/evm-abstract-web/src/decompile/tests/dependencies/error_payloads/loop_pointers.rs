//! Source-bounded allocation and changing memory pointers cross the existing
//! publication/formatter boundary without consumer-side memory reasoning.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mutation {
    None,
    Alias,
    Wrap,
}

fn runtime(coefficient: u8, step: u8, guard: bool, mutation: Mutation) -> String {
    let mut code = Runtime::default();
    // n = calldata[0]; reject n > 7 || n < 2. The success edge supplies both
    // unsigned bounds; the loop itself reloads n through its allocated header.
    code.emit(&[
        0x5f, 0x35, 0x80, 0x60, 0x07, 0x90, 0x11, 0x81, 0x60, 0x02, 0x90, 0x10, 0x17,
    ]);
    code.jump("reject", true);
    // B = 0xe0 + 0x30*n; memory[B] = n. Keep n and B on the stack.
    code.emit(&[0x80, 0x60, 0x30, 0x02, 0x60, 0xe0, 0x01, 0x81, 0x81, 0x52]);
    if mutation == Mutation::Wrap {
        // For admitted n=2, the first update reaches 0x47: a payload/output
        // frame can overwrite the pointer cell. A finite n bound is insufficient.
        code.emit(&[
            0x81, 0x60, 0x10, 0x02, 0x60, 0x40, 0x5f, 0x03, 0x01, 0x60, 0x40, 0x52,
        ]);
    } else {
        // F0 = B + 0x40 + coefficient*n, rather than a fixed allocation base.
        code.emit(&[
            0x81,
            0x60,
            coefficient,
            0x02,
            0x81,
            0x01,
            0x60,
            0x40,
            0x01,
            0x60,
            0x40,
            0x52,
        ]);
    }
    code.emit(&[0x5f]); // i = 0
    code.label("header");
    // Continue while i < memory[B] - 1.
    code.emit(&[0x60, 0x01, 0x82, 0x51, 0x03, 0x81, 0x10, 0x15]);
    code.jump("exit", true);
    // The two writes share the saved iteration-start F, not the first update.
    code.emit(&[
        0x60, 0x40, 0x51, 0x80, 0x60, 0x2b, 0x01, 0x60, 0x40, 0x52, 0x60, 0x37, 0x81, 0x60, 0x20,
        0x01, 0x52, 0x60, step, 0x01, 0x60, 0x40, 0x52,
    ]);
    // memory[B + 32*(i+1)] = 42, then a 64-byte STATICCALL output at the new F.
    code.emit(&[
        0x60, 0x2a, 0x82, 0x60, 0x20, 0x83, 0x60, 0x01, 0x01, 0x02, 0x01, 0x52, 0x60, 0x40, 0x60,
        0x40, 0x51, 0x5f, 0x5f, 0x60, 0x01, 0x5a, 0xfa, 0x50,
    ]);
    if mutation == Mutation::Alias {
        code.emit(&[0x5f, 0x60, 0x40, 0x35, 0x52]);
    }
    if guard {
        // Two pass predecessors retain a real word phi: the message guard
        // passes i, while the alternate branch passes i+1 to the same SSTORE.
        code.emit(&[0x80, 0x60, 0x60, 0x35]);
        code.jump("other-pass", true);
        code.emit(&[0x60, 0x20, 0x35]);
        code.jump("pass", true);
        code.emit(&error_tail(MESSAGE, &[], false));
        code.label("other-pass");
        code.emit(&[0x60, 0x01, 0x01]);
        code.jump("pass", false);
        code.label("pass");
        code.emit(&[0x5f, 0x55]);
    }
    code.emit(&[0x60, 0x01, 0x01]);
    code.jump("header", false);
    code.label("exit");
    if guard {
        code.emit(&[0x50, 0x50, 0x50, 0x00]);
    } else {
        // Keep an observable prefix before the capsule: this exit must remain
        // its original constructor and terminal on the loop exit.
        code.emit(&[0x80, 0x5f, 0x55]);
        code.emit(&error_tail(MESSAGE, &[], false));
    }
    code.label("reject");
    code.emit(&[0x5f, 0x5f, 0xfd]);
    code.finish()
}

#[test]
fn bounded_loop_body_guard_keeps_fp_updates_phi_and_pass_order() {
    assert_bounded_loop(0x60, 0x67, true);
}

#[test]
fn bounded_loop_exit_revert_keeps_fp_updates_phi_and_all_display_modes() {
    assert_bounded_loop(0x70, 0x59, false);
}

fn assert_bounded_loop(coefficient: u8, step: u8, guard: bool) {
    let artifact = artifact(&runtime(coefficient, step, guard, Mutation::None));
    // Exercise the actual wire round trip as well as the scheduler boundary.
    let encoded = serde_json::to_vec(&artifact).unwrap();
    let artifact: DecompileArtifact = serde_json::from_slice(&encoded).unwrap();
    model::check_artifact(&artifact).unwrap();
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(
        program.payloads.len(),
        1,
        "bounded c={coefficient}, step={step}, guard={guard} must certify its payload:\n{}",
        copied_text(&high(stage, false))
    );
    assert_eq!(program.payloads[0].bytes, abi(MESSAGE));
    let [plan] = program.constructions.as_slice() else {
        panic!("bounded c={coefficient}, step={step}, guard={guard} needs its constructor")
    };
    assert_eq!(program.reverts.len(), 1);
    let block = program.block(plan.block).unwrap();
    let elided: BTreeSet<_> = block.statements[plan.start..plan.end]
        .iter()
        .map(|statement| statement.id)
        .collect();
    let operations: std::collections::BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .map(|operation| (operation.id, operation))
        .collect();
    let retained: Vec<_> = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .filter(|statement| {
            matches!(operations[&statement.operation].opcode, 0x52 | 0xfa)
                && !elided.contains(&statement.id)
        })
        .collect();
    assert!(
        retained.len() >= 7,
        "must retain allocation, array writes and call"
    );
    let raw = rows(&structured::lines(stage));
    assert_only_dependency_metadata_changes(stage);
    let mut loops = 0;
    let mut if_nodes = BTreeSet::new();
    structured::visit(&stage.ir.structured.as_ref().unwrap().root, &mut |node| {
        loops += usize::from(matches!(node.kind, StructuredKind::While { .. }));
        if matches!(node.kind, StructuredKind::If { .. }) {
            if_nodes.insert(node.id);
        }
    });
    assert_eq!(loops, 1);
    let error_guard = program.controls.iter().find(|control| {
        if_nodes.contains(&control.node)
            && control.guard.as_ref().is_some_and(|proof| {
                proof.payload == LiftedGuardPayload::ErrorString(program.payloads[0].id)
            })
    });
    assert_eq!(error_guard.is_some(), guard);
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = rows_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            let text = copied_text(&lines);
            assert!(
                text.contains("while (") && text.contains("evm.staticcall("),
                "{text}"
            );
            assert_eq!(text.contains("evm.revert(offset ="), !guard, "{text}");
            assert_eq!(
                text.matches("evm.mstore(offset = 0x40,").count(),
                3,
                "{text}"
            );
            assert!(
                elided
                    .iter()
                    .all(|entity| lines.iter().any(|line| line.entity == *entity) != guard),
                "{text}"
            );
            for statement in &retained {
                assert!(
                    lines.iter().any(|line| line.entity == statement.id),
                    "{text}"
                );
            }
            for transfer in &program.transfers {
                if let Some(tuple) = program.render_transfer(transfer, &stage.ir) {
                    assert!(
                        lines
                            .iter()
                            .any(|line| line.entity == transfer.id && line.text == tuple)
                    );
                }
            }
            if let Some(control) = error_guard {
                let proof = control.guard.as_ref().unwrap();
                assert!(proof.pass_nonzero);
                let transfer = program.transfer(proof.pass_edge).unwrap();
                assert!(
                    !transfer.arguments.is_empty(),
                    "the live counter must cross the pass edge"
                );
                let tuple = program.render_transfer(transfer, &stage.ir).unwrap();
                let failed_if = lines
                    .iter()
                    .position(|line| line.entity == control.id)
                    .unwrap();
                let passed = lines
                    .iter()
                    .position(|line| line.entity == transfer.id && line.text == tuple)
                    .unwrap();
                let stored = lines
                    .iter()
                    .enumerate()
                    .skip(passed + 1)
                    .find(|(_, line)| line.text.contains("evm.sstore("))
                    .map(|(position, _)| position)
                    .unwrap();
                assert!(failed_if < passed && passed < stored, "{text}");
                assert!(lines[failed_if].text.starts_with("require("));
            } else {
                assert!(text.contains("evm.revert(offset ="), "{text}");
                let terminal = block
                    .statements
                    .iter()
                    .find(|statement| statement.operation == program.reverts[0].operation)
                    .unwrap();
                let exit = lines
                    .iter()
                    .position(|line| line.entity == terminal.id)
                    .unwrap();
                let stored = lines
                    .iter()
                    .position(|line| line.text.contains("evm.sstore("))
                    .unwrap();
                assert!(stored < exit, "{text}");
            }
        }
    }
    let normal = high(stage, false);
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    let debug = high(stage, true);
    assert_painted(
        &click(&ctx, &mut workspace, "Show state dependencies"),
        &debug,
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&debug)]
    );
    for entity in [plan.id, program.reverts[0].id, program.payloads[0].id] {
        let at = EntityRef {
            stage: stage.id,
            entity,
        };
        assert!(workspace.state.index.contains(at));
        let details = workspace.state.details(&artifact, at);
        assert!(details.lines[0].starts_with("Certified"));
        assert!(
            details
                .references
                .iter()
                .all(|at| workspace.state.index.contains(*at))
        );
        let source = artifact.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
    }
    let statement = &block.statements[plan.start];
    let at = EntityRef {
        stage: stage.id,
        entity: statement.id,
    };
    assert!(workspace.state.navigate(&artifact, at));
    frame(&ctx, &mut workspace, vec![]);
    assert_eq!(
        workspace.views.route_label(),
        if guard {
            "IR"
        } else {
            "Simplified structured code"
        }
    );
    assert_eq!(rows(&structured::lines(stage)), raw);
    assert_eq!(artifact, original);
}

#[test]
fn loop_alias_and_wrap_keep_raw_error_construction() {
    for mutation in [Mutation::Alias, Mutation::Wrap] {
        let artifact = artifact(&runtime(0x60, 0x67, true, mutation));
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        assert!(
            program.payloads.is_empty() && program.constructions.is_empty(),
            "{mutation:?}: {program:?}"
        );
        for dependencies in [false, true] {
            let text = copied_text(&high(stage, dependencies));
            assert!(
                text.contains("while (")
                    && text.contains("evm.mstore(")
                    && text.contains("evm.revert(offset ="),
                "{text}"
            );
            assert!(!text.contains(&format!(", \"{MESSAGE}\");")), "{text}");
            assert!(
                !text.contains(&format!("evm.revert(\"{MESSAGE}\");")),
                "{text}"
            );
        }
    }
}

//! Consumer regressions use actual scheduler publications and original source
//! occurrences. The ABI oracle assembles bytes independently of certification.
use super::*;
use crate::decompile::{index::StageIndex, structured::HighDisplayOptions};
use evm_abstract_protocol::decompile_model::{LiftedGuardPayload, ValidationStatus};
use guards::{Runtime, click_code, rows_with};

const MESSAGE: &str = "ds-math-mul-overflow";

mod loop_pointers;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Shape {
    Nonzero,
    Zero,
    Nested,
    SeparateConstructors,
    Standalone,
    Join,
}

fn abi(message: &str) -> Vec<u8> {
    let mut bytes = vec![0x08, 0xc3, 0x79, 0xa0];
    let mut offset = [0; 32];
    offset[31] = 32;
    bytes.extend(offset);
    let mut length = [0; 32];
    length[24..].copy_from_slice(&(message.len() as u64).to_be_bytes());
    bytes.extend(length);
    bytes.extend_from_slice(message.as_bytes());
    bytes.resize(68 + message.len().div_ceil(32) * 32, 0);
    bytes
}

fn error_tail(message: &str, after: &[u8], padding: bool) -> Vec<u8> {
    assert!(!message.is_empty() && message.len() < 128);
    let mut code = vec![0x60, 0x40, 0x51, 0x7f];
    let mut selector = [0; 32];
    selector[..4].copy_from_slice(&[0x08, 0xc3, 0x79, 0xa0]);
    code.extend(selector);
    code.extend([0x81, 0x52]);
    code.extend([0x60, 0x20, 0x81, 0x60, 0x04, 0x01, 0x52]);
    code.extend([0x60, message.len() as u8, 0x81, 0x60, 0x24, 0x01, 0x52]);
    for (position, chunk) in message.as_bytes().chunks(32).enumerate() {
        let mut word = [0; 32];
        word[..chunk.len()].copy_from_slice(chunk);
        if padding && chunk.len() < 32 {
            word[31] = 1;
        }
        code.push(0x7f);
        code.extend(word);
        code.extend([0x81, 0x60, (68 + position * 32) as u8, 0x01, 0x52]);
    }
    code.extend(after);
    code.extend([0x50, 0x60, abi(message).len() as u8, 0x60, 0x40, 0x51, 0xfd]);
    code
}

fn runtime(shape: Shape, message: &str, after: &[u8], padding: bool) -> String {
    let mut code = Runtime::default();
    code.emit(&[0x60, 0x80, 0x60, 0x40, 0x52]);
    if shape == Shape::Standalone {
        code.emit(&error_tail(message, after, padding));
        return code.finish();
    }
    code.emit(&[0x5f, 0x35]);
    match shape {
        Shape::Nonzero => {
            code.jump("pass", true);
            code.emit(&error_tail(message, after, padding));
            code.label("pass");
            code.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
        }
        Shape::Zero => {
            code.jump("failure", true);
            code.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
            code.label("failure");
            code.emit(&error_tail(message, after, padding));
        }
        Shape::Nested => {
            code.jump("failure", true);
            code.emit(&[0x60, 0x20, 0x35, 0x15]);
            code.jump("failure", true);
            code.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
            code.label("failure");
            code.emit(&error_tail(message, after, padding));
        }
        Shape::SeparateConstructors => {
            code.jump("first-pass", true);
            code.emit(&error_tail(message, after, padding));
            code.label("first-pass");
            code.emit(&[0x60, 0x20, 0x35, 0x15]);
            code.jump("second-pass", true);
            code.emit(&error_tail(message, after, padding));
            code.label("second-pass");
            code.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
        }
        Shape::Join => {
            code.jump("pass", true);
            code.jump("failure", false);
            code.label("pass");
            code.emit(&[0x60, 0x2a, 0x5f, 0x55]);
            code.label("failure");
            code.emit(&error_tail(message, after, padding));
        }
        Shape::Standalone => unreachable!(),
    }
    code.finish()
}

#[test]
fn certified_error_strings_use_require_and_retain_pass_reachable_join_occurrences() {
    for shape in [
        Shape::Nonzero,
        Shape::Zero,
        Shape::Nested,
        Shape::SeparateConstructors,
        Shape::Standalone,
        Shape::Join,
    ] {
        let artifact = artifact(&runtime(shape, MESSAGE, &[], false));
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let [payload] = program.payloads.as_slice() else {
            panic!("{program:?}")
        };
        assert_eq!(payload.message, MESSAGE);
        assert_eq!(payload.bytes, abi(MESSAGE));
        let constructors = if shape == Shape::SeparateConstructors {
            2
        } else {
            1
        };
        assert_eq!(program.constructions.len(), constructors, "{shape:?}");
        assert_eq!(program.reverts.len(), constructors);
        let renderer = program.render_index(&stage.ir);
        let controls: Vec<_> = program
            .controls
            .iter()
            .filter(|c| c.guard.is_some())
            .collect();
        assert_eq!(
            controls.len(),
            match shape {
                Shape::Nested | Shape::SeparateConstructors => 2,
                Shape::Standalone => 0,
                _ => 1,
            }
        );
        let mut requirements = Vec::new();
        for control in &controls {
            let guard = control.guard.as_ref().unwrap();
            if matches!(shape, Shape::Nonzero | Shape::Zero) {
                assert_eq!(guard.pass_nonzero, shape == Shape::Nonzero);
            }
            assert_eq!(guard.payload, LiftedGuardPayload::ErrorString(payload.id));
            let condition = renderer
                .render_condition(control.condition, !guard.pass_nonzero, |_| true)
                .unwrap();
            let expected = format!("require({condition}, \"{MESSAGE}\");");
            assert_eq!(
                renderer.render_guard(control, |_| true).unwrap().unwrap(),
                expected
            );
            requirements.push((control.id, expected));
        }
        let raw = rows(&structured::lines(stage));
        let expected = rows(&high(stage, false));
        let retained = matches!(shape, Shape::Standalone | Shape::Join);
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
                assert_eq!(
                    lines
                        .iter()
                        .filter(|line| line.text.starts_with("require("))
                        .count(),
                    controls.len(),
                    "{text}"
                );
                assert!(!text.contains("if ("), "{text}");
                for (entity, requirement) in &requirements {
                    assert!(
                        lines.iter().any(|line| line.entity == *entity
                            && line.indent == 0
                            && line.text == *requirement),
                        "{text}"
                    );
                }
                for plan in &program.constructions {
                    let block = program.block(plan.block).unwrap();
                    let mut after = 0;
                    for statement in &block.statements[plan.start..plan.end] {
                        let found = lines
                            .iter()
                            .enumerate()
                            .skip(after)
                            .find(|(_, line)| line.entity == statement.id);
                        assert_eq!(found.is_some(), retained, "{shape:?}: {text}");
                        if let Some((position, row)) = found {
                            assert_eq!(row.indent, 0, "{text}");
                            after = position + 1;
                        }
                    }
                }
                assert_eq!(
                    text.matches("evm.revert(offset =").count(),
                    usize::from(retained),
                    "{text}"
                );
                if shape == Shape::Join {
                    assert!(
                        controls[0]
                            .guard
                            .as_ref()
                            .unwrap()
                            .continuation
                            .as_ref()
                            .unwrap()
                            .pass_falls_through
                    );
                    assert!(text.find("require(").unwrap() < text.find("evm.sstore(").unwrap());
                    assert!(
                        text.find("evm.sstore(").unwrap()
                            < text.find("evm.revert(offset =").unwrap()
                    );
                }
            }
        }
        assert_eq!(rows(&high(stage, false)), expected);
        assert_eq!(rows(&structured::lines(stage)), raw);
        assert_eq!(artifact, original);
    }
}

#[test]
fn one_artifact_mixes_require_explicit_error_construction_and_invalid_with_source_identity() {
    let mut runtime = Runtime::default();
    runtime.emit(&[0x60, 0x80, 0x60, 0x40, 0x52, 0x5f, 0x35]);
    runtime.jump("empty-pass", true);
    runtime.emit(&[0x5f, 0x5f, 0xfd]);
    runtime.label("empty-pass");
    runtime.emit(&[0x60, 0x20, 0x35]);
    runtime.jump("payload-pass", true);
    // Byte reconstruction cannot absorb this failure-only observation.
    runtime.emit(&error_tail(MESSAGE, &[0x33, 0x50], false));
    runtime.label("payload-pass");
    runtime.emit(&[0x60, 0x40, 0x35]);
    runtime.jump("normal", true);
    runtime.emit(&[0xfe]);
    runtime.label("normal");
    runtime.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
    let artifact = artifact(&runtime.finish());
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let guard = program
        .controls
        .iter()
        .find(|control| control.guard.is_some())
        .unwrap();
    assert_eq!(
        program
            .controls
            .iter()
            .filter(|control| control.guard.is_some())
            .count(),
        1
    );
    let operations: std::collections::BTreeMap<_, _> = stage
        .ir
        .blocks
        .iter()
        .flat_map(|block| &block.operations)
        .map(|operation| (operation.id, operation))
        .collect();
    let failure = program
        .blocks
        .iter()
        .find(|block| {
            block
                .statements
                .iter()
                .any(|statement| operations[&statement.operation].opcode == 0x33)
        })
        .unwrap();
    let retained: Vec<_> = failure
        .statements
        .iter()
        .filter(|statement| {
            matches!(
                operations[&statement.operation].opcode,
                0x51 | 0x52 | 0x33 | 0xfd
            )
        })
        .collect();
    assert!(retained.len() >= 7);
    let terminal = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| operations[&statement.operation].opcode == 0xfe)
        .unwrap();
    let normal = high(stage, false);
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
            assert_eq!(text.matches("require(").count(), 1, "{text}");
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.text.starts_with("if ("))
                    .count(),
                2,
                "{text}"
            );
            assert_eq!(text.matches("evm.revert(offset =").count(), 1, "{text}");
            assert_eq!(text.matches("evm.invalid();").count(), 1, "{text}");
            assert!(
                !text.contains("exception(")
                    && !text.contains("} else {")
                    && !text.contains("!(!("),
                "{text}"
            );
            let mut previous = 0;
            for statement in &retained {
                let (position, line) = lines
                    .iter()
                    .enumerate()
                    .skip(previous)
                    .find(|(_, line)| line.entity == statement.id && !line.text.starts_with("//"))
                    .unwrap();
                assert_eq!(line.indent, 1, "{text}");
                previous = position + 1;
            }
            let store = lines
                .iter()
                .find(|line| line.text.contains("evm.sstore("))
                .unwrap();
            assert_eq!(store.indent, 0, "{text}");
            assert!(lines.iter().any(|line| line.entity == terminal.id
                && line.text == "evm.invalid();"
                && line.indent == 1));
        }
    }
    let (ctx, mut workspace) = setup(artifact.clone());
    assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&normal)]
    );
    for statement in retained.into_iter().chain(std::iter::once(terminal)) {
        let at = EntityRef {
            stage: stage.id,
            entity: statement.id,
        };
        let source = artifact.origin(at).unwrap().source[0].pc;
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        assert!(workspace.state.navigate(&artifact, at));
        let frame = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert!(
            frame
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
        );
    }
    let at = EntityRef {
        stage: stage.id,
        entity: guard.id,
    };
    assert!(workspace.state.navigate(&artifact, at));
    frame(&ctx, &mut workspace, vec![]);
    let details = workspace.state.details(&artifact, at);
    assert!(details.lines.iter().any(|line| line.contains("require(")));
    let failure = guard.guard.as_ref().unwrap().failure_node;
    assert!(details.references.contains(&EntityRef {
        stage: stage.id,
        entity: failure
    }));
    assert!(workspace.state.navigate(
        &artifact,
        EntityRef {
            stage: stage.id,
            entity: failure
        }
    ));
    frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Structured code");
    assert_eq!(artifact, original);
}

#[test]
fn constructors_have_complete_inspector_links_clipboard_and_source_navigation() {
    for shape in [Shape::Nonzero, Shape::Standalone] {
        let artifact = artifact(&runtime(shape, MESSAGE, &[], false));
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let plan = &program.constructions[0];
        let revert = &program.reverts[0];
        let payload = &program.payloads[0];
        let block = program.block(plan.block).unwrap();
        let normal = high(stage, false);
        let (ctx, mut workspace) = setup(artifact.clone());
        assert_painted(&frame(&ctx, &mut workspace, vec![]), &normal);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&normal)]
        );
        let visible = normal
            .iter()
            .find(|line| {
                line.text.starts_with("require(") || line.text.starts_with("evm.revert(offset =")
            })
            .unwrap();
        let label = labels(&frame(&ctx, &mut workspace, vec![]), "decompile-code")
            .into_iter()
            .find(|label| label.ends_with(&visible.text))
            .unwrap();
        click_code(&ctx, &mut workspace, &label);
        assert_eq!(
            workspace.state.selected.as_ref().unwrap().at.entity,
            visible.entity
        );
        for entity in [plan.id, revert.id, payload.id] {
            let at = EntityRef {
                stage: stage.id,
                entity,
            };
            assert!(workspace.state.index.contains(at));
            assert!(
                !artifact
                    .origin(at)
                    .unwrap_or_else(|| panic!("missing origin for {at:?}"))
                    .source
                    .is_empty()
            );
            let details = workspace.state.details(&artifact, at);
            assert!(details.lines[0].starts_with("Certified"));
            assert!(
                details
                    .references
                    .iter()
                    .all(|at| workspace.state.index.contains(*at))
            );
        }
        let at = EntityRef {
            stage: stage.id,
            entity: plan.id,
        };
        let details = workspace.state.details(&artifact, at);
        assert!(
            details
                .lines
                .iter()
                .any(|line| line.contains("exceptional faults"))
        );
        for statement in &block.statements[plan.start..plan.end] {
            for entity in [statement.id, statement.operation] {
                assert!(details.references.contains(&EntityRef {
                    stage: stage.id,
                    entity
                }));
            }
            let at = EntityRef {
                stage: stage.id,
                entity: statement.id,
            };
            let source = artifact.origin(at).unwrap().source[0].pc;
            workspace.state.select_source(&artifact, source);
            assert!(
                workspace
                    .state
                    .source_descendants(&artifact, source)
                    .contains(&at)
            );
            assert!(workspace.state.navigate(&artifact, at));
            let result = frame(&ctx, &mut workspace, vec![]);
            assert_eq!(
                workspace.views.route_label(),
                if shape == Shape::Nonzero {
                    "IR"
                } else {
                    "Simplified structured code"
                }
            );
            if shape == Shape::Standalone {
                assert!(
                    result
                        .controls
                        .iter()
                        .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
                );
            }
        }
        assert!(workspace.state.navigate(&artifact, at));
        frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "IR");
        let raw = structured::lines(stage);
        click(&ctx, &mut workspace, "Structured code");
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&raw)]
        );
        assert!(raw.iter().any(|line| line.text.contains("evm.mstore(")));
    }
}

#[test]
fn missing_capsule_and_alias_padding_or_observing_failure_keep_primitive_behavior() {
    for (after, padding) in [
        (&[0x5f, 0x5f, 0x35, 0x52][..], false),
        (&[][..], true),
        (&[0x60, 0x2a, 0x5f, 0x55][..], false),
        (&[0x33, 0x50][..], false),
    ] {
        let artifact = artifact(&runtime(Shape::Nonzero, MESSAGE, after, padding));
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        assert!(program.controls.iter().all(|c| c.guard.is_none()));
        for dependencies in [false, true] {
            let text = copied_text(&high(stage, dependencies));
            assert!(
                text.contains("if (")
                    && text.contains("evm.mstore(")
                    && text.contains("evm.revert(offset ="),
                "{text}"
            );
            assert!(
                !text.contains("require(") && !text.contains("evm.revert(\""),
                "{text}"
            );
            if after.first() == Some(&0x33) {
                assert!(text.contains("evm.caller("));
            }
            if after.contains(&0x55) {
                assert_eq!(text.matches("evm.sstore(").count(), 2);
            }
        }
    }
    let mut artifact = artifact(&runtime(Shape::Standalone, MESSAGE, &[], false));
    let current = artifact.current_stage.unwrap();
    let stage = artifact
        .stages
        .iter_mut()
        .find(|s| s.id == current)
        .unwrap();
    stage.ir.lifted.as_mut().unwrap().constructions.clear();
    let text = copied_text(&high(stage, false));
    assert!(
        text.contains("evm.mstore(") && text.contains("evm.revert(offset ="),
        "{text}"
    );
    assert!(!text.contains("evm.revert(\""));
    let index = StageIndex::new(&stage.ir);
    let revert = stage.ir.lifted.as_ref().unwrap().reverts[0].id;
    let details = crate::decompile::details::build(stage, &index, revert, &[]);
    assert!(
        details
            .lines
            .iter()
            .any(|line| line.contains("construction proof unavailable"))
    );
}

#[test]
fn exact_escaping_and_live_operations_before_the_capsule_remain_visible() {
    let escaped = "quote\" slash\\ line\n雪\0";
    let artifact = artifact(&runtime(Shape::Standalone, escaped, &[], false));
    let stage = artifact.current().unwrap();
    assert_eq!(
        stage.ir.lifted.as_ref().unwrap().payloads[0].message,
        escaped
    );
    assert!(
        high(stage, false)
            .iter()
            .all(|line| !line.text.contains(['\n', '\0']))
    );
    assert_eq!(
        stage.ir.lifted.as_ref().unwrap().payloads[0].bytes,
        abi(escaped)
    );

    let mut code = Runtime::default();
    code.emit(&[0x60, 0x80, 0x60, 0x40, 0x52]);
    code.emit(&[0x60, 0x2a, 0x61, 0x02, 0x00, 0x52]);
    code.emit(&[0x61, 0x02, 0x00, 0x51, 0x5f, 0x55]);
    code.emit(&error_tail(MESSAGE, &[], false));
    let observed = super::artifact(&code.finish());
    let stage = observed.current().unwrap();
    for dependencies in [false, true] {
        let text = copied_text(&high(stage, dependencies));
        assert!(
            text.contains("evm.mstore(offset = 0x200, value = 0x2a)"),
            "{text}"
        );
        assert!(
            text.contains("evm.mload(offset = 0x200)") && text.contains("evm.sstore("),
            "{text}"
        );
        assert!(text.contains("evm.revert(offset ="), "{text}");
        assert!(text.find("evm.mstore(").unwrap() < text.find("evm.mload(").unwrap());
        assert!(text.find("evm.sstore(").unwrap() < text.find("evm.revert(offset =").unwrap());
    }
}

#[test]
fn one_word_and_multiword_messages_use_the_same_consumer_plan() {
    for message in [
        "TransferHelper: TRANSFER_FAILED",
        "a multiword error message with a second ABI string word",
    ] {
        let artifact = artifact(&runtime(Shape::Standalone, message, &[], false));
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        assert_eq!(program.payloads[0].bytes, abi(message));
        for dependencies in [false, true] {
            let lines = high(stage, dependencies);
            assert!(
                lines
                    .iter()
                    .any(|line| line.text.contains("evm.revert(offset ="))
            );
            assert!(lines.iter().any(|line| line.text.contains("evm.mstore(")));
            assert!(
                !lines
                    .iter()
                    .any(|line| line.text.starts_with("evm.revert(\""))
            );
        }
    }
}

#[test]
fn forged_construction_sources_quarantine_the_entire_overlay_even_on_failed_snapshots() {
    let base = artifact(&runtime(Shape::Zero, MESSAGE, &[], false));
    for field in ["block", "revert", "start", "end"] {
        for status in [ValidationStatus::Passed, ValidationStatus::Failed] {
            let mut changed = base.clone();
            let current = changed.current_stage.unwrap();
            let stage = changed.stages.iter_mut().find(|s| s.id == current).unwrap();
            let program = stage.ir.lifted.as_mut().unwrap();
            let other = program
                .controls
                .iter()
                .find_map(|c| c.guard.as_ref())
                .unwrap()
                .block;
            let plan = &mut program.constructions[0];
            match field {
                "block" => plan.block = other,
                "revert" => plan.revert = plan.id,
                "start" => plan.start += 1,
                "end" => plan.end -= 1,
                _ => unreachable!(),
            }
            stage.validation.status = status;
            let text = copied_text(&high(stage, false)); // Deliberately bypass admission.
            assert!(
                text.contains("if (")
                    && text.contains("evm.mstore(")
                    && text.contains("evm.revert(offset ="),
                "{field}: {text}"
            );
            assert!(
                !text.contains("require(") && !text.contains("evm.revert(\""),
                "{text}"
            );
            assert!(
                model::check_artifact(&changed)
                    .unwrap_err()
                    .contains("guard source topology")
            );
        }
    }
}

#[test]
fn schema_three_empty_guards_remain_readable_and_cannot_advertise_error_payloads() {
    let mut legacy = artifact("5f356008575f5ffd5b602a5f5500");
    legacy.schema_version = 3;
    model::check_artifact(&legacy).unwrap();
    let wire = serde_json::to_value(&legacy).unwrap();
    for stage in wire["stages"].as_array().unwrap() {
        if let Some(program) = stage["ir"].get("lifted").filter(|p| !p.is_null()) {
            assert!(program.get("payloads").is_none());
            assert!(program.get("reverts").is_none());
            assert!(program.get("constructions").is_none());
        }
    }
    let read: DecompileArtifact = serde_json::from_value(wire).unwrap();
    model::check_artifact(&read).unwrap();
    assert_eq!(
        rows(&high(read.current().unwrap(), false)),
        rows(&high(legacy.current().unwrap(), false))
    );
    let mut payload = artifact(&runtime(Shape::Standalone, MESSAGE, &[], false));
    payload.schema_version = 3;
    assert!(model::check_artifact(&payload).is_err());
}

#[test]
fn shared_nested_materializations_keep_one_source_certificate_and_two_guard_occurrences() {
    let artifact = artifact(&runtime(Shape::Nested, MESSAGE, &[], false));
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    assert_eq!(program.constructions.len(), 1);
    let plan = &program.constructions[0];
    let mut bodies = 0;
    structured::visit(&stage.ir.structured.as_ref().unwrap().root, &mut |node| {
        if matches!(node.kind, StructuredKind::Block { block } if block == plan.block) {
            bodies += 1;
        }
    });
    assert!(
        bodies >= 2,
        "exercise actual repeated source materializations"
    );
    assert_eq!(
        program
            .controls
            .iter()
            .filter(|control| control.guard.is_some())
            .count(),
        2,
    );
    for dependencies in [false, true] {
        let text = copied_text(&high(stage, dependencies));
        assert_eq!(text.matches("require(").count(), 2, "{text}");
        assert!(
            !text.contains("if (") && !text.contains("evm.revert(offset ="),
            "{text}"
        );
    }
}

#[test]
fn dispatcher_terminal_keeps_original_constructor_and_terminal_navigation() {
    use evm_abstract_protocol::decompile_model::DispatchCase;
    let mut artifact = artifact(&runtime(Shape::Standalone, MESSAGE, &[], false));
    let current = artifact.current_stage.unwrap();
    let stage = artifact
        .stages
        .iter_mut()
        .find(|stage| stage.id == current)
        .unwrap();
    let plan = stage.ir.lifted.as_ref().unwrap().constructions[0].clone();
    // Exercise the alternate source materialization without fabricating a new
    // payload certificate: the unchanged one-block body terminates the case.
    stage.ir.structured.as_mut().unwrap().root.kind = StructuredKind::Dispatcher {
        entry: plan.block,
        cases: vec![DispatchCase {
            block: plan.block,
            transitions: vec![],
        }],
    };
    let stage = artifact.current().unwrap();
    stage
        .ir
        .lifted
        .as_ref()
        .unwrap()
        .validate_guard_sources(&stage.ir)
        .unwrap();
    model::check_artifact(&artifact).unwrap();
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = rows_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            assert!(
                lines
                    .iter()
                    .any(|line| line.text.contains("dispatch_state_"))
            );
            let terminal = stage
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .block(plan.block)
                .unwrap()
                .statements
                .iter()
                .find(|statement| {
                    stage
                        .ir
                        .blocks
                        .iter()
                        .flat_map(|block| &block.operations)
                        .any(|operation| {
                            operation.id == statement.operation && operation.opcode == 0xfd
                        })
                })
                .unwrap();
            let exits: Vec<_> = lines
                .iter()
                .filter(|line| line.text.contains("evm.revert(offset ="))
                .collect();
            assert_eq!(exits.len(), 1);
            assert_eq!(exits[0].entity, terminal.id);
            for statement in &stage
                .ir
                .lifted
                .as_ref()
                .unwrap()
                .block(plan.block)
                .unwrap()
                .statements[plan.start..plan.end]
            {
                assert!(lines.iter().any(|line| line.entity == statement.id));
            }
        }
    }
    let at = EntityRef {
        stage: stage.id,
        entity: stage
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .block(plan.block)
            .unwrap()
            .statements
            .last()
            .unwrap()
            .id,
    };
    let (ctx, mut workspace) = setup(artifact.clone());
    assert!(workspace.state.navigate(&artifact, at));
    let selected = frame(&ctx, &mut workspace, vec![]);
    assert_eq!(workspace.views.route_label(), "Simplified structured code");
    assert!(
        selected
            .controls
            .iter()
            .any(|row| row["role"] == "decompile-code" && row["selected"] == true)
    );
    assert_eq!(
        click(&ctx, &mut workspace, "Copy structured text").copied,
        [copied_text(&high(stage, false))]
    );
}

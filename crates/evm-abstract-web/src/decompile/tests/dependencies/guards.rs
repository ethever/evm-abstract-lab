//! Guard consumers are exercised through published artifacts, native rows,
//! clipboard output and qualified navigation. Fixture labels only assemble EVM.
use super::*;
use crate::decompile::{index::StageIndex, structured::HighDisplayOptions};

#[derive(Default)]
pub(super) struct Runtime {
    bytes: Vec<u8>,
    labels: std::collections::BTreeMap<&'static str, usize>,
    jumps: Vec<(usize, &'static str)>,
}

impl Runtime {
    pub(super) fn emit(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
    }
    pub(super) fn label(&mut self, name: &'static str) {
        assert!(self.labels.insert(name, self.bytes.len()).is_none());
        self.emit(&[0x5b]);
    }
    pub(super) fn jump(&mut self, name: &'static str, conditional: bool) {
        self.emit(&[0x60, 0, if conditional { 0x57 } else { 0x56 }]);
        self.jumps.push((self.bytes.len() - 2, name));
    }
    pub(super) fn finish(mut self) -> String {
        for (position, name) in self.jumps {
            self.bytes[position] = u8::try_from(self.labels[name]).unwrap();
        }
        self.bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

fn nested_guards() -> String {
    let mut runtime = Runtime::default();
    runtime.emit(&[0x33, 0x50, 0x5f, 0x35]); // caller observation, then condition
    runtime.jump("failure", true);
    runtime.emit(&[0x60, 0x20, 0x35, 0x15]); // opposite failure polarity
    runtime.jump("failure", true);
    runtime.emit(&[0x60, 0x2a, 0x5f, 0x55, 0x00]);
    runtime.label("failure");
    runtime.emit(&[0x5f, 0x5f, 0xfd]);
    runtime.finish()
}

pub(super) fn rows_with(stage: &Stage, display: HighDisplayOptions) -> Vec<structured::Line> {
    structured::lifted_lines_indexed(stage, &StageIndex::new(&stage.ir), display)
}

pub(super) fn click_code(ctx: &Context, workspace: &mut Workspace, label: &str) -> Frame {
    let visible = frame(ctx, workspace, vec![]);
    // Status glyphs expose the same marked label. Click the real code Label,
    // whose interaction region owns selection, rather than its marker widget.
    let pos = center(
        visible
            .controls
            .iter()
            .find(|row| row["role"] == "decompile-code" && row["label"] == label)
            .unwrap(),
    );
    frame(
        ctx,
        workspace,
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    frame(
        ctx,
        workspace,
        vec![Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    )
}

#[test]
fn live_pass_edge_tuple_executes_once_after_require_before_the_joined_store() {
    let mut runtime = Runtime::default();
    runtime.emit(&[0x5f, 0x35]);
    runtime.jump("other", true);
    runtime.emit(&[0x60, 0x20, 0x35, 0x60, 0x40, 0x35]); // value, slot
    runtime.emit(&[0x60, 0x60, 0x35]); // guard condition
    runtime.jump("failure", true);
    runtime.label("join");
    runtime.emit(&[0x55, 0x00]);
    runtime.label("other");
    runtime.emit(&[0x60, 0x2a, 0x60, 0x01]);
    runtime.jump("join", false);
    runtime.label("failure");
    runtime.emit(&[0x5f, 0x5f, 0xfd]);
    let artifact = artifact(&runtime.finish());
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let control = program
        .controls
        .iter()
        .find(|control| control.guard.is_some())
        .unwrap();
    let guard = control.guard.as_ref().unwrap();
    let transfer = program.transfer(guard.pass_edge).unwrap();
    assert_eq!(
        transfer.arguments.len(),
        2,
        "both joined words must remain live"
    );
    let tuple = program
        .render_index(&stage.ir)
        .render_transfer(transfer, |_| true)
        .unwrap()
        .unwrap();
    for dependencies in [false, true] {
        let lines = high(stage, dependencies);
        let require = lines
            .iter()
            .position(|line| line.entity == control.id)
            .unwrap();
        let assigned = lines
            .iter()
            .position(|line| line.entity == transfer.id && line.text == tuple)
            .unwrap();
        let store = lines
            .iter()
            .enumerate()
            .skip(assigned + 1)
            .find(|(_, line)| line.text.contains("evm.sstore("))
            .map(|(position, _)| position)
            .unwrap();
        assert!(
            require < assigned && assigned < store,
            "{}",
            copied_text(&lines)
        );
        assert_eq!(lines[require].indent, lines[assigned].indent);
        let target = program
            .block(stage.ir.edge(guard.pass_edge).unwrap().to)
            .unwrap();
        assert_eq!(
            lines[store].text.split(" //").next().unwrap(),
            format!(
                "evm.sstore(slot = v{}, value = v{});",
                target.parameters[1].0, target.parameters[0].0
            )
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.text.contains("evm.sstore("))
                .count(),
            2,
            "both source occurrences of the joined store remain represented"
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.entity == transfer.id && line.text == tuple)
                .count(),
            1
        );
        assert!(
            lines.iter().any(|line| line.text.starts_with("if (")),
            "the outer ordinary branch must remain"
        );
        let failure = program.transfer(guard.failure_edge).unwrap();
        assert!(!lines.iter().any(|line| line.entity == failure.id));
    }
}

#[test]
fn guards_preserve_loop_continues_and_dispatcher_state_actions() {
    let mut loop_runtime = Runtime::default();
    loop_runtime.emit(&[0x5f]);
    loop_runtime.label("loop");
    loop_runtime.emit(&[0x80, 0x60, 0x03, 0x90, 0x10]);
    loop_runtime.jump("body", true);
    loop_runtime.emit(&[0x5f, 0x55, 0x00]);
    loop_runtime.label("body");
    loop_runtime.emit(&[0x5f, 0x35]);
    loop_runtime.jump("failure", true);
    loop_runtime.emit(&[0x60, 0x01, 0x01]);
    loop_runtime.jump("loop", false);
    loop_runtime.label("failure");
    loop_runtime.emit(&[0x5f, 0x5f, 0xfd]);

    let mut dispatch = Runtime::default();
    dispatch.emit(&[0x5f, 0x35]);
    dispatch.jump("failure", true);
    dispatch.emit(&[0x60, 0x20, 0x35]);
    dispatch.jump("b", true);
    for (name, target) in [("a", "b"), ("b", "a")] {
        dispatch.label(name);
        dispatch.emit(&[0x5f, 0x54, 0x60, 0x01, 0x01, 0x80, 0x5f, 0x55]);
        dispatch.emit(&[0x60, 0x04, 0x90, 0x10]);
        dispatch.jump(target, true);
        dispatch.jump("exit", false);
    }
    dispatch.label("exit");
    dispatch.emit(&[0x00]);
    dispatch.label("failure");
    dispatch.emit(&[0x5f, 0x5f, 0xfd]);

    for (bytecode, is_dispatch) in [(loop_runtime.finish(), false), (dispatch.finish(), true)] {
        let artifact = artifact(&bytecode);
        let stage = artifact.current().unwrap();
        let raw = structured::lines(stage);
        let store_ops: BTreeSet<_> = stage
            .ir
            .blocks
            .iter()
            .flat_map(|block| &block.operations)
            .filter(|operation| operation.opcode == 0x55)
            .map(|operation| operation.id)
            .collect();
        let store_owners: std::collections::BTreeMap<_, _> = stage
            .ir
            .lifted
            .as_ref()
            .unwrap()
            .blocks
            .iter()
            .flat_map(|block| &block.statements)
            .filter(|statement| store_ops.contains(&statement.operation))
            .map(|statement| (statement.id, statement.operation))
            .collect();
        // Structuring can retain multiple source occurrences of a shared SCC;
        // compare that actual occurrence order, not the number of CFG blocks.
        let raw_stores: Vec<_> = raw
            .iter()
            .filter(|line| store_ops.contains(&line.entity))
            .map(|line| line.entity)
            .collect();
        assert!(!raw_stores.is_empty());
        for dependencies in [false, true] {
            let lines = high(stage, dependencies);
            let text = copied_text(&lines);
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.text.starts_with("require("))
                    .count(),
                1,
                "{text}"
            );
            assert_eq!(
                lines
                    .iter()
                    .filter_map(|line| store_owners.get(&line.entity).copied())
                    .collect::<Vec<_>>(),
                raw_stores,
                "{text}"
            );
            assert!(text.contains("while (true)"), "{text}");
            if is_dispatch {
                assert!(
                    text.contains("dispatch_state_") && text.contains("continue 'dispatch"),
                    "{text}"
                );
                assert!(text.contains("break 'dispatch"), "{text}");
            } else {
                let guard = lines
                    .iter()
                    .find(|line| line.text.starts_with("require("))
                    .unwrap();
                let continued = lines.iter().find(|line| line.text == "continue;").unwrap();
                assert_eq!(guard.indent, continued.indent);
                assert!(text.contains(" + "), "{text}");
                assert_eq!(
                    lines.iter().filter(|line| line.text == "break;").count(),
                    raw.iter().filter(|line| line.text == "break;").count(),
                    "preserve the actual source loop exits, including STOP exits"
                );
            }
            assert!(!text.contains("evm.revert("), "{text}");
        }
        assert_eq!(rows(&structured::lines(stage)), rows(&raw));
    }
}

#[test]
fn nested_certified_guards_flatten_at_their_scope_and_keep_observation_effect_order() {
    let artifact = artifact(&nested_guards());
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let normal = high(stage, false);
    let guards: Vec<_> = normal
        .iter()
        .filter(|line| line.text.starts_with("require("))
        .collect();
    assert_eq!(guards.len(), 2, "{}", copied_text(&normal));
    assert!(guards.iter().all(|line| line.indent == 0));
    let caller = normal
        .iter()
        .position(|line| line.text.contains("evm.caller("))
        .unwrap();
    let first = normal
        .iter()
        .position(|line| line.entity == guards[0].entity)
        .unwrap();
    let second = normal
        .iter()
        .position(|line| line.entity == guards[1].entity)
        .unwrap();
    let store = normal
        .iter()
        .position(|line| line.text.contains("evm.sstore("))
        .unwrap();
    assert!(caller < first && first < second && second < store);
    assert_eq!(normal[store].indent, 0);
    assert!(normal[store].text.contains("slot = 0x0, value = 0x2a"));
    assert!(
        normal
            .iter()
            .any(|line| line.indent == 0 && line.text == "evm.stop();")
    );
    assert!(
        !normal
            .iter()
            .any(|line| line.text.contains("evm.revert(") || line.text.starts_with("if ("))
    );
    for dependencies in [false, true] {
        for annotations in [false, true] {
            let lines = rows_with(
                stage,
                HighDisplayOptions {
                    show_state_dependencies: dependencies,
                    show_structure_annotations: annotations,
                },
            );
            assert_eq!(
                lines
                    .iter()
                    .filter(|line| line.text.starts_with("require("))
                    .map(|line| (line.entity, line.indent, &line.text))
                    .collect::<Vec<_>>(),
                guards
                    .iter()
                    .map(|line| (line.entity, line.indent, &line.text))
                    .collect::<Vec<_>>(),
            );
            assert!(!lines.iter().any(|line| line.text.contains("evm.revert(")));
        }
    }
    let raw = structured::lines(stage);
    assert_eq!(
        raw.iter()
            .filter(|line| line.text.starts_with("if ("))
            .count(),
        2
    );
    assert!(raw.iter().any(|line| line.text.contains("evm.revert(")));
    assert_eq!(artifact, original);
}

#[test]
fn pass_that_falls_into_joined_revert_keeps_the_revert_after_its_store() {
    // True writes storage, false skips it, and both paths reach the same REVERT.
    let artifact = artifact("5f35600857600d565b602a5f555b5f5ffd");
    let stage = artifact.current().unwrap();
    let program = stage.ir.lifted.as_ref().unwrap();
    let guard = program
        .controls
        .iter()
        .find_map(|control| control.guard.as_ref())
        .unwrap();
    let continuation = guard
        .continuation
        .as_ref()
        .expect("exercise a real following failure suffix");
    assert!(continuation.pass_falls_through);
    assert_eq!(
        program
            .render_index(&stage.ir)
            .sequence_end(continuation.sequence, continuation.start),
        None
    );
    for dependencies in [false, true] {
        let lines = high(stage, dependencies);
        let guard = lines
            .iter()
            .position(|line| line.text.starts_with("require("))
            .unwrap();
        let store = lines
            .iter()
            .position(|line| line.text.contains("evm.sstore("))
            .unwrap();
        let revert = lines
            .iter()
            .position(|line| line.text.contains("evm.revert("))
            .unwrap();
        assert!(guard < store && store < revert, "{}", copied_text(&lines));
        assert_eq!(
            [
                lines[guard].indent,
                lines[store].indent,
                lines[revert].indent
            ],
            [0, 0, 0]
        );
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.text.contains("evm.revert("))
                .count(),
            1
        );
    }
}

#[test]
fn payload_or_observation_on_failure_keeps_the_conditional_and_its_effects() {
    for failure in [
        vec![0x60, 0x01, 0x5f, 0xfd],                   // nonempty payload
        vec![0x60, 0x2a, 0x5f, 0x55, 0x5f, 0x5f, 0xfd], // SSTORE
        vec![0x33, 0x50, 0x5f, 0x5f, 0xfd],             // retained CALLER observation
    ] {
        let mut runtime = Runtime::default();
        runtime.emit(&[0x5f, 0x35]);
        runtime.jump("pass", true);
        runtime.emit(&failure);
        runtime.label("pass");
        runtime.emit(&[0x00]);
        let artifact = artifact(&runtime.finish());
        let stage = artifact.current().unwrap();
        for dependencies in [false, true] {
            let lines = high(stage, dependencies);
            let text = copied_text(&lines);
            assert!(!text.contains("require("), "{text}");
            assert!(
                text.contains("if (") && text.contains("evm.revert("),
                "{text}"
            );
            let revert = lines
                .iter()
                .position(|line| line.text.contains("evm.revert("))
                .unwrap();
            if failure[0] == 0x33 {
                let caller = lines
                    .iter()
                    .position(|line| line.text.contains("evm.caller("))
                    .unwrap();
                assert!(caller < revert);
                assert_eq!(lines[caller].indent, lines[revert].indent);
            } else if failure.contains(&0x55) {
                let store = lines
                    .iter()
                    .position(|line| line.text.contains("evm.sstore("))
                    .unwrap();
                assert!(store < revert);
                assert_eq!(lines[store].indent, lines[revert].indent);
                assert!(lines[store].text.contains("slot = 0x0, value = 0x2a"));
            } else {
                assert!(lines[revert].text.contains("size = 0x1"));
            }
        }
    }
}

#[test]
fn both_polarities_share_require_rows_clipboard_inspector_and_source_navigation() {
    for (bytecode, pass_nonzero) in [
        ("5f35600a57602a5f55005b5f5ffd", false),
        ("5f356008575f5ffd5b602a5f5500", true),
    ] {
        let artifact = artifact(bytecode);
        let original = artifact.clone();
        let stage = artifact.current().unwrap();
        let program = stage.ir.lifted.as_ref().unwrap();
        let controls: Vec<_> = program
            .controls
            .iter()
            .filter(|control| control.guard.is_some())
            .collect();
        assert_eq!(controls.len(), 1);
        let control = controls[0];
        let guard = control.guard.as_ref().unwrap();
        assert_eq!(guard.pass_nonzero, pass_nonzero);
        let renderer = program.render_index(&stage.ir);
        if pass_nonzero {
            let continuation = guard
                .continuation
                .as_ref()
                .expect("false failure follows the source If in its Sequence");
            assert!(!continuation.pass_falls_through);
            assert_eq!(
                renderer.sequence_end(continuation.sequence, continuation.start),
                Some(continuation.end)
            );
        } else {
            assert!(
                guard.continuation.is_none(),
                "the true failure arm is closed directly"
            );
        }
        let predicate = renderer
            .render_expression(control.condition, |_| true)
            .unwrap();
        let expected = renderer.render_guard(control, |_| true).unwrap().unwrap();
        assert_eq!(
            expected,
            if pass_nonzero {
                format!("require({predicate});")
            } else {
                format!("require(!({predicate}));")
            }
        );
        let normal = high(stage, false);
        assert_eq!(
            normal
                .iter()
                .filter(|line| line.entity == control.id)
                .map(|line| (line.indent, line.text.as_str()))
                .collect::<Vec<_>>(),
            [(0, expected.as_str())]
        );
        let failed_transfer = program.transfer(guard.failure_edge).unwrap();
        for dependencies in [false, true] {
            for annotations in [false, true] {
                let lines = rows_with(
                    stage,
                    HighDisplayOptions {
                        show_state_dependencies: dependencies,
                        show_structure_annotations: annotations,
                    },
                );
                assert!(!lines.iter().any(|line| line.entity == failed_transfer.id));
                assert_eq!(lines.iter().filter(|line| line.text == expected).count(), 1);
            }
        }
        let at = EntityRef {
            stage: stage.id,
            entity: control.id,
        };
        let source = stage
            .ir
            .block(guard.block)
            .unwrap()
            .operations
            .iter()
            .find(|operation| operation.opcode == 0x57)
            .unwrap()
            .source
            .unwrap()
            .pc;
        let failure_block = stage.ir.edge(guard.failure_edge).unwrap().to;
        let absorbed = program
            .block(failure_block)
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
            .expect("retained semantic REVERT witness");
        let (ctx, mut workspace) = setup(artifact.clone());
        let initial = frame(&ctx, &mut workspace, vec![]);
        assert_painted(&initial, &normal);
        let label = labels(&initial, "decompile-code")
            .into_iter()
            .find(|label| label.ends_with(&expected))
            .unwrap();
        let clicked = click_code(&ctx, &mut workspace, &label);
        assert_eq!(
            workspace.state.selected.as_ref().unwrap().at,
            at,
            "{:?}",
            clicked
                .controls
                .iter()
                .filter(|row| row["label"]
                    .as_str()
                    .is_some_and(|label| label.contains("require(")))
                .collect::<Vec<_>>()
        );
        let details = workspace.state.details(&artifact, at);
        assert!(details.lines.iter().any(|line| line.contains(&expected)));
        for entity in [
            control.condition,
            control.node,
            guard.block,
            guard.pass_edge,
            guard.failure_edge,
            guard.pass_node,
            guard.failure_node,
        ] {
            assert!(details.references.contains(&EntityRef {
                stage: stage.id,
                entity
            }));
        }
        assert!(
            details
                .references
                .iter()
                .all(|at| workspace.state.index.contains(*at))
        );
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

        workspace.state.select_source(&artifact, source);
        assert!(
            workspace
                .state
                .source_descendants(&artifact, source)
                .contains(&at)
        );
        frame(&ctx, &mut workspace, vec![]);
        assert!(workspace.state.navigate(&artifact, at));
        let navigated = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Simplified structured code");
        assert!(
            navigated
                .controls
                .iter()
                .any(|row| row["role"] == "decompile-code"
                    && row["selected"] == true
                    && row["label"].as_str().unwrap().ends_with(&expected))
        );

        // An absorbed failure statement has no high row. Navigation must find
        // its own IR witness, not an empty fallback or the guard's row.
        let absorbed_at = EntityRef {
            stage: stage.id,
            entity: absorbed.id,
        };
        assert!(workspace.state.navigate(&artifact, absorbed_at));
        let ir = frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "IR");
        assert!(
            ir.controls
                .iter()
                .any(|row| row["role"] == "decompile-ir" && row["selected"] == true)
        );
        assert_eq!(workspace.state.selected.as_ref().unwrap().at, absorbed_at);
        let raw = structured::lines(stage);
        assert_painted(&click(&ctx, &mut workspace, "Structured code"), &raw);
        assert_eq!(
            click(&ctx, &mut workspace, "Copy structured text").copied,
            [copied_text(&raw)]
        );
        assert!(raw.iter().any(|line| line.text.contains("evm.revert(")));
        assert!(workspace.state.navigate(
            &artifact,
            EntityRef {
                stage: stage.id,
                entity: guard.failure_node
            }
        ));
        frame(&ctx, &mut workspace, vec![]);
        assert_eq!(workspace.views.route_label(), "Structured code");
        assert_eq!(workspace.artifact.as_ref(), Some(&original));
    }
}

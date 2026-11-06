//! Real published artifacts cover branch lowering; isolated typed fixtures cover
//! renderer boundaries that do not require another engine or GUI harness.
use super::{fixture, model};
use crate::decompile::{index::StageIndex, structured};
use evm_abstract_protocol::{
    DecompileArtifact, DecompileRequest, JobState,
    decompile_model::{
        Block, Edge, EdgeKind, Effects, EntityId, ExitKind, Ir, LiftedBlock, LiftedControl,
        LiftedExpression, LiftedExpressionKind, LiftedProgram, LiftedStatement, LiftedType,
        Operation, SourceSpan, Stage, StageId, StageKind, StructuredKind, StructuredNode,
        StructuredProgram, Terminator, U256,
    },
};
use evm_abstract_server::jobs::{Config, Pool};
use std::{
    collections::BTreeSet,
    thread,
    time::{Duration, Instant},
};

// Nonzero calldata reverts; the false continuation stores 42 and stops.
const REVERT_GUARD: &str = "5f35600a57602a5f55005b5f5ffd";
// Both arms define a word consumed by the joined SSTORE.
const LIVE_PHI: &str = "5f35600a576001600d565b60025b5f5500";

type Row = (EntityId, usize, String);

fn row(entity: u64, indent: usize, text: impl Into<String>) -> Row {
    (EntityId(entity), indent, text.into())
}

fn rows(lines: Vec<structured::Line>) -> Vec<Row> {
    lines
        .into_iter()
        .map(|line| (line.entity, line.indent, line.text))
        .collect()
}

fn high(stage: &Stage, dependencies: bool) -> Vec<Row> {
    rows(structured::lifted_lines_indexed(
        stage,
        &StageIndex::new(&stage.ir),
        structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            show_structure_annotations: true,
        },
    ))
}

fn clean(stage: &Stage, dependencies: bool) -> Vec<Row> {
    rows(structured::lifted_lines_indexed(
        stage,
        &StageIndex::new(&stage.ir),
        structured::HighDisplayOptions {
            show_state_dependencies: dependencies,
            ..structured::HighDisplayOptions::default()
        },
    ))
}

fn artifact(bytecode: &str) -> DecompileArtifact {
    let pool = Pool::new(Config {
        workers: 1,
        queue_capacity: 1,
        retained_jobs: 1,
    })
    .unwrap();
    let job = pool
        .submit_decompile(DecompileRequest {
            bytecode: bytecode.into(),
            ..DecompileRequest::default()
        })
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let snapshot = pool.decompile_status(job.id).unwrap();
        if snapshot.state == JobState::Completed {
            break;
        }
        assert!(Instant::now() < deadline, "{snapshot:?}");
        thread::sleep(Duration::from_millis(2));
    }
    let artifact = pool
        .decompile_result(job.id)
        .unwrap()
        .artifact
        .as_ref()
        .clone();
    pool.shutdown().unwrap();
    model::check_artifact(&artifact).unwrap();
    assert_eq!(artifact.current().unwrap().kind, StageKind::Lifted);
    artifact
}

fn only_if(stage: &Stage) -> EntityId {
    let mut found = Vec::new();
    structured::visit(&stage.ir.structured.as_ref().unwrap().root, &mut |node| {
        if matches!(node.kind, StructuredKind::If { .. }) {
            found.push(node.id);
        }
    });
    assert_eq!(found.len(), 1, "exercise exactly one real conditional");
    found[0]
}

// Derive removable token rows from actual edge witnesses. The remaining tuples
// compare every semantic row's text, identity, indentation and order.
fn without_dependencies(stage: &Stage, lines: Vec<Row>) -> Vec<Row> {
    let program = stage.ir.lifted.as_ref().unwrap();
    let tokens: BTreeSet<_> = program
        .transfers
        .iter()
        .flat_map(|transfer| {
            let edge = stage.ir.edge(transfer.edge).unwrap();
            let target = stage.ir.block(edge.to).unwrap();
            [
                (
                    transfer.id,
                    format!(
                        "m{} = m{}; // memory token on e{}",
                        target.memory_parameter.0, edge.memory.0, edge.id.0
                    ),
                ),
                (
                    transfer.id,
                    format!(
                        "q{} = q{}; // effect token on e{}",
                        target.effect_parameter.0, edge.effect.0, edge.id.0
                    ),
                ),
            ]
        })
        .collect();
    lines
        .into_iter()
        .filter(|(entity, _, text)| !tokens.contains(&(*entity, text.clone())))
        .map(|(entity, indent, text)| {
            let semantic = text
                .split_once(" // memory")
                .or_else(|| text.split_once(" // effect"))
                .map_or(text.as_str(), |(semantic, _)| semantic);
            (entity, indent, semantic.to_owned())
        })
        .collect()
}

#[test]
fn real_revert_guard_replaces_conditional_and_preserves_pass_continuation() {
    let artifact = artifact(REVERT_GUARD);
    let original = artifact.clone();
    let stage = artifact.current().unwrap();
    let index = StageIndex::new(&stage.ir);
    let branch = index.node(&stage.ir, only_if(stage)).unwrap();
    let StructuredKind::If { else_branch, .. } = &branch.kind else {
        unreachable!()
    };
    let StructuredKind::Transfer { edge } = else_branch.kind else {
        panic!("expected the terminal-true-arm lowering's empty continuation transfer")
    };
    let program = stage.ir.lifted.as_ref().unwrap();
    let control = program.control(branch.id).unwrap();
    let transfer = program.transfer(edge).unwrap();
    assert!(transfer.arguments.is_empty());
    assert!(program.render_transfer(transfer, &stage.ir).is_none());

    let raw = rows(structured::lines(stage));
    let debug = high(stage, true);
    let normal = high(stage, false);
    let guard = control
        .guard
        .as_ref()
        .expect("actual empty-REVERT certificate");
    let predicate = program
        .render_index(&stage.ir)
        .render_guard(control, |_| true)
        .unwrap()
        .unwrap();
    let control_rows: Vec<_> = normal.iter().filter(|line| line.0 == control.id).collect();
    assert_eq!(
        control_rows,
        [&(control.id, 0, predicate)],
        "the shared formatter owns polarity and the existing control owns the row"
    );
    let guard_position = debug.iter().position(|line| line.0 == control.id).unwrap();
    assert_eq!(debug[guard_position + 1].0, transfer.id);
    assert_eq!(debug[guard_position + 1].1, 0);
    assert_eq!(debug[guard_position + 2].0, transfer.id);
    assert_eq!(debug[guard_position + 2].1, 0);
    let failure_transfer = program.transfer(guard.failure_edge).unwrap().id;
    assert!(!debug.iter().any(|line| line.0 == failure_transfer));
    assert!(!debug.iter().any(|line| line.2 == "} else {"));
    assert_eq!(normal, without_dependencies(stage, debug.clone()));
    assert_eq!(
        normal
            .iter()
            .filter(|line| line.2.contains("evm.revert("))
            .count(),
        0,
        "the certified failure is represented by require, independent of metadata options"
    );
    let store = program
        .blocks
        .iter()
        .flat_map(|block| &block.statements)
        .find(|statement| {
            index
                .operation(&stage.ir, statement.operation)
                .unwrap()
                .opcode
                == 0x55
        })
        .unwrap();
    let store_position = normal.iter().position(|line| line.0 == store.id).unwrap();
    assert_eq!(
        normal[store_position].1, 0,
        "pass continuation stays at the guard's indentation"
    );
    assert!(normal.iter().position(|line| line.0 == control.id).unwrap() < store_position);
    assert!(
        normal[store_position]
            .2
            .contains("sstore(slot = 0x0, value = 0x2a)")
    );
    assert!(
        normal
            .iter()
            .any(|line| line.1 == 0 && line.2 == "evm.stop();")
    );

    let raw_else = raw.iter().position(|line| line.2 == "} else {").unwrap();
    assert_eq!(
        raw.iter()
            .filter(|line| line.2.contains("evm.revert("))
            .count(),
        1
    );
    let edge = stage.ir.edge(edge).unwrap();
    let target = stage.ir.block(edge.to).unwrap();
    assert_eq!(
        &raw[raw_else..raw_else + 4],
        &[
            (branch.id, 0, "} else {".into()),
            (
                else_branch.id,
                1,
                format!(
                    "m{} = m{}; // memory token on e{}",
                    target.memory_parameter.0, edge.memory.0, edge.id.0
                )
            ),
            (
                else_branch.id,
                1,
                format!(
                    "q{} = q{}; // effect token on e{}",
                    target.effect_parameter.0, edge.effect.0, edge.id.0
                )
            ),
            (branch.id, 0, "}".into()),
        ]
    );
    assert_eq!(high(stage, true), debug, "dependency toggle round trip");
    assert_eq!(high(stage, false), normal);
    assert_eq!(rows(structured::lines(stage)), raw);
    assert_eq!(
        artifact, original,
        "rendering must preserve the complete artifact"
    );
}

#[test]
fn real_live_phi_else_survives_with_dependencies_hidden_or_shown() {
    let artifact = artifact(LIVE_PHI);
    let stage = artifact.current().unwrap();
    let index = StageIndex::new(&stage.ir);
    let branch = index.node(&stage.ir, only_if(stage)).unwrap();
    let StructuredKind::If { else_branch, .. } = &branch.kind else {
        unreachable!()
    };
    let program = stage.ir.lifted.as_ref().unwrap();
    let control = program.control(branch.id).unwrap();
    let mut live_else = Vec::new();
    structured::visit(else_branch, &mut |node| {
        if let StructuredKind::Transfer { edge } = node.kind {
            let transfer = program.transfer(edge).unwrap();
            if let Some(text) = program.render_transfer(transfer, &stage.ir) {
                live_else.push((transfer.id, 1, text));
            }
        }
    });
    assert!(
        !live_else.is_empty(),
        "exercise a live phi transfer in the else arm"
    );
    let raw = rows(structured::lines(stage));
    for dependencies in [false, true] {
        let lines = high(stage, dependencies);
        let otherwise = lines
            .iter()
            .position(|line| *line == (control.id, 0, "} else {".into()))
            .unwrap();
        let end = lines
            .iter()
            .rposition(|line| *line == (control.id, 0, "}".into()))
            .unwrap();
        for assignment in &live_else {
            assert!(lines[otherwise + 1..end].contains(assignment), "{lines:#?}");
        }
        assert!(lines.contains(&(
            control.id,
            0,
            format!("if ({}) {{", program.render_expression(control.condition))
        )));
    }
    assert_eq!(
        high(stage, false),
        without_dependencies(stage, high(stage, true))
    );
    assert_eq!(rows(structured::lines(stage)), raw);
}

fn node(id: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode {
        id: EntityId(id),
        kind,
    }
}

// These are deliberately renderer-only stages, not claimed to be complete
// engine artifacts. Explicit witness holes below exercise diagnostic rendering.
fn renderer_stage(otherwise: StructuredNode) -> Stage {
    Stage {
        id: StageId(1),
        parent: None,
        kind: StageKind::Lifted,
        validation: fixture::passed(),
        ir: Ir {
            blocks: vec![Block {
                id: EntityId(1),
                source: None,
                parameters: vec![],
                memory_parameter: EntityId(3),
                effect_parameter: EntityId(4),
                operations: vec![],
                terminator: Terminator::Pending,
            }],
            structured: Some(StructuredProgram {
                root: node(
                    10,
                    StructuredKind::If {
                        condition: EntityId(2),
                        then_branch: Box::new(node(11, StructuredKind::Sequence(vec![]))),
                        else_branch: Box::new(otherwise),
                    },
                ),
            }),
            lifted: Some(LiftedProgram {
                expressions: vec![
                    LiftedExpression {
                        id: EntityId(101),
                        block: EntityId(1),
                        ty: LiftedType::Word256,
                        kind: LiftedExpressionKind::Value(EntityId(2)),
                    },
                    LiftedExpression {
                        id: EntityId(102),
                        block: EntityId(1),
                        ty: LiftedType::Predicate,
                        kind: LiftedExpressionKind::NonZero(EntityId(101)),
                    },
                ],
                blocks: vec![LiftedBlock {
                    id: EntityId(104),
                    block: EntityId(1),
                    parameters: vec![],
                    statements: vec![],
                    condition: Some(EntityId(102)),
                    target: Some(EntityId(101)),
                    exit_values: vec![],
                }],
                controls: vec![LiftedControl {
                    id: EntityId(103),
                    node: EntityId(10),
                    condition: EntityId(102),
                    guard: None,
                }],
                ..LiftedProgram::default()
            }),
            ..Ir::default()
        },
    }
}

fn with_else(body: Vec<Row>) -> Vec<Row> {
    let mut expected = vec![
        row(103, 0, "if ((v2 != 0)) {"),
        row(11, 1, "// sequence ast11 · 0 children"),
        row(103, 0, "} else {"),
    ];
    expected.extend(body);
    expected.push(row(103, 0, "}"));
    expected
}

fn with_clean_else(body: Vec<Row>) -> Vec<Row> {
    let mut expected = vec![row(103, 0, "if ((v2 != 0)) {"), row(103, 0, "} else {")];
    expected.extend(body);
    expected.push(row(103, 0, "}"));
    expected
}

#[test]
fn empty_and_nested_context_only_else_omit_their_rows_but_keep_both_empty_if() {
    for (otherwise, raw_body) in [
        (
            node(12, StructuredKind::Sequence(vec![])),
            vec![row(12, 1, "// sequence ast12 · 0 children")],
        ),
        (
            node(
                12,
                StructuredKind::Sequence(vec![node(
                    13,
                    StructuredKind::Sequence(vec![
                        node(14, StructuredKind::Block { block: EntityId(1) }),
                        node(15, StructuredKind::Sequence(vec![])),
                    ]),
                )]),
            ),
            vec![
                row(12, 1, "// sequence ast12 · 1 children"),
                row(13, 1, "// sequence ast13 · 2 children"),
                row(14, 1, "// b1"),
                row(15, 1, "// sequence ast15 · 0 children"),
            ],
        ),
    ] {
        let stage = renderer_stage(otherwise);
        let mut expected_raw = vec![
            row(10, 0, "if (v2 != 0) {"),
            row(11, 1, "// sequence ast11 · 0 children"),
            row(10, 0, "} else {"),
        ];
        expected_raw.extend(raw_body);
        expected_raw.push(row(10, 0, "}"));
        for dependencies in [false, true] {
            assert_eq!(
                high(&stage, dependencies),
                vec![
                    row(103, 0, "if ((v2 != 0)) {"),
                    row(11, 1, "// sequence ast11 · 0 children"),
                    row(103, 0, "}")
                ]
            );
            assert_eq!(rows(structured::lines(&stage)), expected_raw);
            assert_eq!(
                clean(&stage, dependencies),
                vec![row(103, 0, "if ((v2 != 0)) {"), row(103, 0, "}")]
            );
        }
    }
}

#[test]
fn an_inner_if_remains_substantive_after_its_own_empty_else_is_omitted() {
    let mut stage = renderer_stage(node(
        12,
        StructuredKind::Sequence(vec![node(
            20,
            StructuredKind::If {
                condition: EntityId(2),
                then_branch: Box::new(node(21, StructuredKind::Sequence(vec![]))),
                else_branch: Box::new(node(22, StructuredKind::Sequence(vec![]))),
            },
        )]),
    ));
    stage
        .ir
        .lifted
        .as_mut()
        .unwrap()
        .controls
        .push(LiftedControl {
            id: EntityId(106),
            node: EntityId(20),
            condition: EntityId(102),
            guard: None,
        });
    for dependencies in [false, true] {
        assert_eq!(
            high(&stage, dependencies),
            with_else(vec![
                row(12, 1, "// sequence ast12 · 1 children"),
                row(106, 1, "if ((v2 != 0)) {"),
                row(21, 2, "// sequence ast21 · 0 children"),
                row(106, 1, "}"),
            ])
        );
    }
}

#[test]
fn break_and_continue_else_keep_loop_indentation_and_do_not_invert_empty_then() {
    for (kind, text) in [
        (StructuredKind::Break, "break;"),
        (StructuredKind::Continue, "continue;"),
    ] {
        let mut stage = renderer_stage(node(12, kind));
        let program = stage.ir.structured.as_mut().unwrap();
        program.root = node(
            20,
            StructuredKind::While {
                header: Box::new(node(21, StructuredKind::Sequence(vec![]))),
                condition: None,
                condition_nonzero: true,
                body: Box::new(program.root.clone()),
                exit_edge: None,
            },
        );
        for dependencies in [false, true] {
            assert_eq!(
                high(&stage, dependencies),
                vec![
                    row(20, 0, "'loop20: while (true) {"),
                    row(21, 1, "// sequence ast21 · 0 children"),
                    row(103, 1, "if ((v2 != 0)) {"),
                    row(11, 2, "// sequence ast11 · 0 children"),
                    row(103, 1, "} else {"),
                    row(12, 2, text),
                    row(103, 1, "}"),
                    row(20, 0, "}"),
                ]
            );
        }
    }
}

#[test]
fn partial_invalid_exceptional_and_normal_exits_are_substantive_else_content() {
    for (kind, terminator, text) in [
        (
            StructuredKind::PartialExit { block: EntityId(1) },
            Terminator::Pending,
            "partial_exit(\"recovery pending\");",
        ),
        (
            StructuredKind::PartialExit { block: EntityId(1) },
            Terminator::Jump {
                target: EntityId(2),
                edges: vec![],
                unresolved: true,
                invalid_targets: vec![],
            },
            "jump v2 via [] + unresolved_target (partial);",
        ),
        (
            StructuredKind::Exit { block: EntityId(1) },
            Terminator::Jump {
                target: EntityId(2),
                edges: vec![],
                unresolved: false,
                invalid_targets: vec![U256::from(255)],
            },
            "jump v2 via [] + exceptional_invalid_target;",
        ),
        (
            StructuredKind::Exit { block: EntityId(1) },
            Terminator::Trap {
                reason: "invalid opcode".into(),
            },
            "exception(\"invalid opcode\");",
        ),
        (
            StructuredKind::Exit { block: EntityId(1) },
            Terminator::Exit {
                kind: ExitKind::Stop,
                values: vec![],
            },
            "evm.stop();",
        ),
        (
            StructuredKind::Exit { block: EntityId(1) },
            Terminator::Exit {
                kind: ExitKind::EndOfCode,
                values: vec![],
            },
            "evm.stop(); // end of code",
        ),
    ] {
        let mut stage = renderer_stage(node(12, kind));
        stage.ir.blocks[0].terminator = terminator;
        for dependencies in [false, true] {
            assert_eq!(
                high(&stage, dependencies),
                with_else(vec![row(104, 1, text)])
            );
            assert_eq!(
                clean(&stage, dependencies),
                with_clean_else(vec![row(104, 1, text)])
            );
            let raw = rows(structured::lines(&stage));
            assert!(raw.contains(&row(10, 0, "} else {")));
            assert!(raw.contains(&row(12, 1, text)));
        }
    }
}

#[test]
fn a_fault_comment_alone_keeps_else_and_its_source_identity() {
    let mut stage = renderer_stage(node(12, StructuredKind::Block { block: EntityId(1) }));
    stage.ir.blocks[0].operations.push(Operation {
        id: EntityId(40),
        source: Some(SourceSpan { pc: 7, end_pc: 8 }),
        opcode: 0x55,
        name: "SSTORE".into(),
        inputs: vec![],
        outputs: vec![],
        effects: Effects {
            storage_write: true,
            ..Effects::default()
        },
        memory_input: None,
        memory_output: None,
        replacement: None,
        effect_input: None,
        effect_output: None,
    });
    stage.ir.blocks[0].terminator = Terminator::Trap {
        reason: "stack underflow".into(),
    };
    stage.ir.lifted.as_mut().unwrap().blocks[0]
        .statements
        .push(LiftedStatement {
            id: EntityId(105),
            operation: EntityId(40),
            inputs: vec![],
            outputs: vec![],
        });
    let fault = "// fault o40 SSTORE at source 0x0007: stack underflow (not completed)";
    for dependencies in [false, true] {
        assert_eq!(
            high(&stage, dependencies),
            with_else(vec![row(104, 1, "// b1"), row(105, 1, fault)])
        );
        assert_eq!(
            clean(&stage, dependencies),
            with_clean_else(vec![row(105, 1, fault)])
        );
        assert_eq!(
            rows(structured::lines(&stage)),
            vec![
                row(10, 0, "if (v2 != 0) {"),
                row(11, 1, "// sequence ast11 · 0 children"),
                row(10, 0, "} else {"),
                row(12, 1, "// b1"),
                row(40, 1, fault),
                row(10, 0, "}"),
            ]
        );
    }
}

#[test]
fn unavailable_block_statement_and_exit_diagnostics_keep_else() {
    let mut missing_block = renderer_stage(node(12, StructuredKind::Block { block: EntityId(1) }));
    missing_block.ir.lifted.as_mut().unwrap().blocks.clear();
    let mut missing_statement =
        renderer_stage(node(12, StructuredKind::Block { block: EntityId(1) }));
    missing_statement.ir.lifted.as_mut().unwrap().blocks[0]
        .statements
        .push(LiftedStatement {
            id: EntityId(105),
            operation: EntityId(999),
            inputs: vec![],
            outputs: vec![],
        });
    for (stage, body) in [
        (
            missing_block,
            vec![
                row(12, 1, "// b1"),
                row(12, 1, "unavailable_high_block(b1);"),
            ],
        ),
        (
            missing_statement,
            vec![
                row(104, 1, "// b1"),
                row(
                    105,
                    1,
                    "unavailable_statement(h105: lifted rendering refused: MissingWitness(EntityId(999)));",
                ),
            ],
        ),
        (
            renderer_stage(node(
                12,
                StructuredKind::Exit {
                    block: EntityId(999),
                },
            )),
            vec![row(12, 1, "unavailable_exit(b999);")],
        ),
    ] {
        for dependencies in [false, true] {
            assert_eq!(high(&stage, dependencies), with_else(body.clone()));
            assert_eq!(
                clean(&stage, dependencies),
                with_clean_else(vec![body.last().unwrap().clone()])
            );
        }
    }
}

#[test]
fn unavailable_live_transfer_keeps_else_even_without_dependency_rows() {
    let mut stage = renderer_stage(node(12, StructuredKind::Transfer { edge: EntityId(9) }));
    stage.ir.edges.push(Edge {
        id: EntityId(9),
        from: EntityId(1),
        to: EntityId(1),
        kind: EdgeKind::Fallthrough,
        arguments: vec![],
        memory: EntityId(3),
        effect: EntityId(4),
    });
    for dependencies in [false, true] {
        let mut body = vec![row(12, 1, "unavailable_live_transfer(e9);")];
        if dependencies {
            body.extend([
                row(12, 1, "m3 = m3; // memory token on e9"),
                row(12, 1, "q4 = q4; // effect token on e9"),
            ]);
        }
        assert_eq!(high(&stage, dependencies), with_else(body.clone()));
        assert_eq!(clean(&stage, dependencies), with_clean_else(body));
    }
}

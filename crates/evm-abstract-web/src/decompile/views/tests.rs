use super::{Route, Views, graph};
use crate::{
    controls,
    decompile::{
        diff::Differences,
        index::StageIndex,
        model::{Publication, ViewState},
        tests::fixture,
    },
};
use egui::{Context, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::{
    JobId,
    decompile_model::{
        DispatchCase, DispatchTransition, EntityId, EntityRef, StageId, StructuredKind,
        StructuredNode, StructuredProgram,
    },
};

fn frame(
    ctx: &Context,
    views: &mut Views,
    artifact: &evm_abstract_protocol::DecompileArtifact,
    state: &mut ViewState,
    size: Vec2,
) -> Vec<serde_json::Value> {
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
            ..RawInput::default()
        },
        |ui| {
            controls::begin_observation(ui.ctx(), true);
            crate::decompile::rows::begin_frame(ui.ctx());
            views.show(ui, artifact, state);
        },
    );
    output.textures_delta.clear();
    serde_json::from_str(&controls::observed(ctx).unwrap()).unwrap()
}

fn selected_visible(controls: &[serde_json::Value], role: &str, needle: &str, height: f32) {
    let target = controls
        .iter()
        .find(|control| {
            control["role"] == role
                && control["selected"] == true
                && control["label"].as_str().unwrap().contains(needle)
        })
        .unwrap_or_else(|| panic!("no visible {role} {needle}: {controls:?}"));
    assert!(target["rect"][1].as_f64().unwrap() >= 0.0);
    assert!(target["rect"][3].as_f64().unwrap() <= f64::from(height));
}

#[test]
fn late_ir_value_source_and_code_navigation_exposes_the_exact_row_and_history() {
    let artifact = fixture::large(1500);
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut views = Views::default();
    let mut state = ViewState::default();
    state.install(
        &artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    for height in [900.0, 300.0] {
        let size = Vec2::new(1440.0, height);
        let operation = EntityRef {
            stage: StageId(2),
            entity: EntityId(3994),
        };
        views.route = Route::Graphs;
        assert!(state.navigate(&artifact, operation));
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-ir", "o3994:", height);
        assert!(!state.entity_navigation);
        let value = EntityRef {
            stage: StageId(2),
            entity: EntityId(101497),
        };
        views.route = Route::Graphs;
        state.navigate(&artifact, value);
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-ir", "v101497:", height);
        state.history(&artifact, false);
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-ir", "o3994:", height);
        state.history(&artifact, true);
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-ir", "v101497:", height);
        // Structured navigation anchors include non-textual Sequence nodes.
        state.navigate(
            &artifact,
            EntityRef {
                stage: StageId(2),
                entity: EntityId(30),
            },
        );
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-code", "sequence ast30", height);
        state.select(&artifact, operation);
        state.entity_navigation = true;
        views.route = Route::Structured;
        // Explicit code-route navigation to an operation, independent of the
        // generic links' preferred IR destination.
        let mut output = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                views.code(ui, &artifact, &mut state);
            },
        );
        output.textures_delta.clear();
        let visible: Vec<serde_json::Value> =
            serde_json::from_str(&controls::observed(&ctx).unwrap()).unwrap();
        selected_visible(&visible, "decompile-code", "v101497 =", height);
        views.route = Route::Source;
        state.select_source(&artifact, 4491);
        state.source_navigation = true;
        let visible = frame(&ctx, &mut views, &artifact, &mut state, size);
        selected_visible(&visible, "decompile-source", "Source 0x118b:", height);
        assert!(!state.source_navigation);
    }
}

#[test]
fn dispatcher_transition_and_edge_links_open_actual_semantic_content() {
    let mut artifact = fixture::large(2);
    artifact.stages[2].ir.structured = Some(StructuredProgram {
        root: StructuredNode {
            id: EntityId(40),
            kind: StructuredKind::Dispatcher {
                entry: EntityId(1),
                cases: vec![DispatchCase {
                    block: EntityId(1),
                    transitions: vec![DispatchTransition {
                        id: EntityId(41),
                        edge: EntityId(9),
                        target: EntityId(1),
                    }],
                }],
            },
        },
    });
    let template = fixture::artifact(evm_abstract_protocol::DecompileStrategy::Dispatcher);
    let mut edge = template.stages[0].ir.edges[0].clone();
    edge.to = EntityId(1);
    edge.memory = EntityId(5);
    edge.effect = EntityId(12);
    artifact.stages[2].ir.edges.push(edge);
    artifact.stages[2].ir.blocks[0].terminator =
        evm_abstract_protocol::decompile_model::Terminator::Fallthrough { edge: EntityId(9) };
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut state = ViewState::default();
    state.install(
        &artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    let mut views = Views {
        route: Route::Source,
        ..Views::default()
    };
    let transition = EntityRef {
        stage: StageId(2),
        entity: EntityId(41),
    };
    state.navigate(&artifact, transition);
    let visible = frame(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        Vec2::new(1440.0, 900.0),
    );
    selected_visible(&visible, "decompile-code", "on e9", 900.0);
    assert!(
        state
            .details(&artifact, transition)
            .lines
            .iter()
            .any(|line| line.contains("dispatch_state = b1"))
    );
    views.route = Route::Graphs;
    state.navigate(
        &artifact,
        EntityRef {
            stage: StageId(2),
            entity: EntityId(9),
        },
    );
    let visible = frame(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        Vec2::new(1440.0, 900.0),
    );
    selected_visible(&visible, "decompile-ir", "e9: b1 → b1", 900.0);
}

#[test]
fn large_varied_push_graph_preparation_only_looks_up_the_bounded_preview() {
    let artifact = fixture::large(10_000);
    let stage = &artifact.stages[1];
    let index = StageIndex::new(&stage.ir);
    let scene = graph::build(
        stage,
        &index,
        &Differences::default(),
        false,
        &artifact.input.runtime_bytecode,
    );
    assert_eq!(scene.cards.len(), 1);
    assert_eq!(scene.cards[0].lines.len(), crate::graph_view::PREVIEW_LINES);
    assert_eq!(scene.cards[0].total_lines, 20_001);
    assert!(index.value_reads.get() <= crate::graph_view::PREVIEW_LINES);
    assert!(scene.cards[0].lines[0].contains("0x0"));
    assert!(scene.cards[0].lines[2].contains("0x1"));
    assert!(scene.cards[0].lines[4].contains("0x2"));
}

#[test]
fn large_changes_prepare_only_visible_rows_after_cached_typed_diff() {
    let mut artifact = fixture::large(1500);
    for op in &mut artifact.stages[2].ir.blocks[0].operations {
        if op.opcode == 0x60 {
            op.replacement = Some(evm_abstract_protocol::decompile_model::U256::from(
                (op.source.unwrap().pc / 3) % 256,
            ));
        }
    }
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut state = ViewState::default();
    state.install(
        &artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    let mut views = Views {
        route: Route::Changes,
        ..Views::default()
    };
    let visible = frame(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        Vec2::new(1440.0, 600.0),
    );
    assert!(views.differences.rows.len() >= 3000);
    assert!(crate::decompile::rows::prepared(&ctx) < 40);
    let rows = visible
        .iter()
        .filter(|control| control["role"] == "decompile-difference")
        .collect::<Vec<_>>();
    assert!(
        !rows.is_empty() && rows.len() < 40,
        "{} visible rows",
        rows.len()
    );
    assert!(
        rows.iter()
            .all(|row| row["label"].as_str().unwrap().contains("Changed"))
    );
}

#[test]
fn source_only_limited_attempt_displays_original_bytes_and_unrecovered_placeholders() {
    let mut artifact = fixture::large(3);
    artifact.stages.truncate(1);
    artifact.current_stage = Some(StageId(0));
    artifact.completion = evm_abstract_protocol::decompile_model::Completion::Limited;
    // Model the intentionally unestablished operands/results at decode stage.
    for op in &mut artifact.stages[0].ir.blocks[0].operations {
        op.inputs.clear();
    }
    for value in &mut artifact.stages[0].ir.values {
        value.fact = evm_abstract_protocol::decompile_model::ValueFact::Bottom;
    }
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut views = Views::default();
    let mut state = ViewState::default();
    state.install(
        &artifact,
        Publication {
            job: JobId(1),
            version: fixture::version(1),
        },
    );
    let visible = frame(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        Vec2::new(1440.0, 600.0),
    );
    assert!(visible.iter().any(|row| row["role"] == "decompile-source"
        && row["label"].as_str().unwrap().contains("PUSH1 6000")));
    views.route = Route::Ir;
    let visible = frame(
        &ctx,
        &mut views,
        &artifact,
        &mut state,
        Vec2::new(1440.0, 600.0),
    );
    assert!(
        visible
            .iter()
            .filter(|row| row["role"] == "decompile-ir")
            .all(|row| !row["label"].as_str().unwrap().contains(" = "))
    );
    let op = EntityRef {
        stage: StageId(0),
        entity: EntityId(1000),
    };
    assert!(
        state
            .details(&artifact, op)
            .lines
            .iter()
            .any(|line| line.contains("unrecovered operands/results"))
    );
}

#[test]
fn if_and_while_descendants_open_the_guard_and_semantic_fields() {
    for is_loop in [false, true] {
        let mut artifact = fixture::large(2);
        let leaf = |id, kind| StructuredNode {
            id: EntityId(id),
            kind,
        };
        let kind = if is_loop {
            StructuredKind::While {
                header: Box::new(leaf(41, StructuredKind::Block { block: EntityId(1) })),
                condition: Some(EntityId(100000)),
                condition_nonzero: true,
                body: Box::new(leaf(42, StructuredKind::Break)),
                exit_edge: None,
            }
        } else {
            StructuredKind::If {
                condition: EntityId(100000),
                then_branch: Box::new(leaf(41, StructuredKind::Block { block: EntityId(1) })),
                else_branch: Box::new(leaf(42, StructuredKind::Exit { block: EntityId(1) })),
            }
        };
        artifact.stages[2].ir.structured = Some(StructuredProgram {
            root: leaf(40, kind),
        });
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut state = ViewState::default();
        state.install(
            &artifact,
            Publication {
                job: JobId(1),
                version: fixture::version(1),
            },
        );
        let mut views = Views {
            route: Route::Source,
            ..Views::default()
        };
        let at = EntityRef {
            stage: StageId(2),
            entity: EntityId(40),
        };
        assert!(state.navigate(&artifact, at));
        let visible = frame(
            &ctx,
            &mut views,
            &artifact,
            &mut state,
            Vec2::new(1440.0, 600.0),
        );
        selected_visible(
            &visible,
            "decompile-code",
            if is_loop { "while (" } else { "if (v100000" },
            600.0,
        );
        let details = state.details(&artifact, at);
        assert!(details.lines[0].contains(if is_loop { "While ast40" } else { "If ast40" }));
        assert!(details.references.contains(&EntityRef {
            stage: StageId(2),
            entity: EntityId(100000)
        }));
    }
}

#[test]
fn fault_context_is_visible_in_painted_graph_ir_and_selected_inspector_without_completing_the_op() {
    use evm_abstract_protocol::decompile_model::{
        Effects, Terminator, Value, ValueFact, ValueKind,
    };
    for (opcode, name) in [(0x01, "ADD"), (0x55, "SSTORE")] {
        let mut artifact = fixture::artifact(evm_abstract_protocol::DecompileStrategy::Dispatcher);
        let stage = &mut artifact.stages[1];
        let successful = stage.ir.blocks[0].operations[0].clone();
        let mut fault = successful.clone();
        fault.id = EntityId(50);
        fault.opcode = opcode;
        fault.name = name.into();
        fault.inputs.clear();
        fault.outputs = if opcode == 0x01 {
            vec![EntityId(70)]
        } else {
            vec![]
        };
        fault.effects = Effects {
            storage_write: opcode == 0x55,
            ..Effects::default()
        };
        stage.ir.blocks[0].operations.push(fault.clone());
        stage.ir.blocks[0].terminator = Terminator::Trap {
            reason: "stack underflow".into(),
        };
        if opcode == 0x01 {
            stage.ir.values.push(Value {
                id: EntityId(70),
                kind: ValueKind::Operation {
                    operation: fault.id,
                    index: 0,
                },
                fact: ValueFact::Bottom,
            });
        }
        // Same source PC in another context must remain a normal operation.
        let mut normal = successful.clone();
        normal.id = EntityId(51);
        normal.opcode = opcode;
        normal.name = name.into();
        normal.effects = fault.effects.clone();
        stage.ir.blocks[1].operations.push(normal.clone());
        let index = StageIndex::new(&stage.ir);
        let normal_text = crate::decompile::structured::operation(&normal, &stage.ir, &index);
        assert!(
            normal_text.contains(&format!("evm.{}(", name.to_lowercase()))
                && !normal_text.contains("fault")
        );
        assert_eq!(stage.ir.blocks[0].operations[0], successful);
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut state = ViewState::default();
        state.install(
            &artifact,
            Publication {
                job: JobId(1),
                version: fixture::version(1),
            },
        );
        let at = EntityRef {
            stage: StageId(1),
            entity: fault.id,
        };
        assert!(state.select(&artifact, at));
        let mut graph = graph::Graph::default();
        let mut painted = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                graph.show(
                    ui,
                    &artifact,
                    &artifact.stages[1],
                    &mut state,
                    &Differences::default(),
                    false,
                );
            },
        );
        painted.textures_delta.clear();
        let graph_controls: Vec<serde_json::Value> =
            serde_json::from_str(&controls::observed(&ctx).unwrap()).unwrap();
        let preview = graph_controls
            .iter()
            .find(|control| {
                control["role"] == "graph-preview"
                    && control["label"]
                        .as_str()
                        .unwrap()
                        .contains(&format!("fault o50 {name}"))
            })
            .unwrap_or_else(|| panic!("fault was not actually painted: {graph_controls:?}"));
        let label = preview["label"].as_str().unwrap();
        assert!(
            label.contains("source 0x0002")
                && label.contains("not completed")
                && label.contains("raw opcode")
        );
        assert!(!label.contains("evm.add(") && !label.contains("evm.sstore("));
        assert!(graph_controls.iter().any(|control| {
            control["role"] == "graph-preview"
                && control["label"]
                    .as_str()
                    .unwrap()
                    .contains("evm.sstore(v3, v3)")
        }));
        let mut views = Views {
            route: Route::Ir,
            ..Views::default()
        };
        let visible = frame(
            &ctx,
            &mut views,
            &artifact,
            &mut state,
            Vec2::new(1440.0, 900.0),
        );
        selected_visible(
            &visible,
            "decompile-ir",
            &format!("fault o50 {name}"),
            900.0,
        );
        let mut inspected = ctx.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))),
                ..RawInput::default()
            },
            |ui| {
                controls::begin_observation(ui.ctx(), true);
                crate::decompile::inspector::show(ui, &artifact, &mut state);
            },
        );
        inspected.textures_delta.clear();
        let details: Vec<serde_json::Value> =
            serde_json::from_str(&controls::observed(&ctx).unwrap()).unwrap();
        assert!(details.iter().any(|control| {
            control["role"] == "decompile-entity-detail"
                && control["label"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("fault o50 {name} at source 0x0002"))
        }));
        assert!(details.iter().any(|control| {
            control["role"] == "decompile-entity-detail"
                && control["label"]
                    .as_str()
                    .unwrap()
                    .contains("Declared primitive effects (not completed)")
        }));
        assert_eq!(artifact.stages[1].ir.blocks[0].operations[1], fault);
    }
}

#[test]
fn bounded_graph_preview_exposes_a_late_fault_without_formatting_the_whole_body() {
    let mut artifact = fixture::large(1500);
    artifact.stages[1].ir.blocks[0].terminator =
        evm_abstract_protocol::decompile_model::Terminator::Trap {
            reason: "retained final fault".into(),
        };
    let stage = &artifact.stages[1];
    let index = StageIndex::new(&stage.ir);
    let scene = graph::build(
        stage,
        &index,
        &Differences::default(),
        false,
        &artifact.input.runtime_bytecode,
    );
    assert_eq!(scene.cards[0].lines.len(), crate::graph_view::PREVIEW_LINES);
    let last = stage.ir.blocks[0].operations.last().unwrap();
    assert!(
        scene.cards[0]
            .lines
            .last()
            .unwrap()
            .contains(&format!("fault o{}", last.id.0))
    );
    assert!(index.value_reads.get() <= crate::graph_view::PREVIEW_LINES);
}

//! Executable equivalent AST fixtures isolate residual dispatcher text routing.
use evm_decompile::{
    Artifact, Config, DispatchCase, DispatchTransition, EntityId, ExecutionLimits, Fork, Input,
    MemoryHost, StructuredKind, StructuredNode, StructuredProgram, ValidationStatus, decompile,
    execute_bytecode, execute_structured, render_text, validate_stage,
};

const DISPATCH: EntityId = EntityId(1_000_000);
const LOOP: EntityId = EntityId(1_000_001);
fn node(id: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode {
        id: EntityId(id),
        kind,
    }
}
fn dispatcher(artifact: &Artifact, block: EntityId) -> StructuredNode {
    let ir = &artifact.current().unwrap().ir;
    let transitions = ir
        .block(block)
        .unwrap()
        .terminator
        .edges()
        .into_iter()
        .enumerate()
        .map(|(index, edge)| DispatchTransition {
            id: EntityId(1_000_100 + index as u64),
            edge,
            target: ir.edge(edge).unwrap().to,
        })
        .collect();
    StructuredNode {
        id: DISPATCH,
        kind: StructuredKind::Dispatcher {
            entry: block,
            cases: vec![DispatchCase { block, transitions }],
        },
    }
}
fn install(artifact: &mut Artifact, root: StructuredNode) {
    let current = artifact.current_stage.unwrap();
    let stage = artifact
        .stages
        .iter_mut()
        .find(|stage| stage.id == current)
        .unwrap();
    stage.ir.structured = Some(StructuredProgram { root });
    let validation = validate_stage(stage);
    assert_eq!(
        validation.status,
        ValidationStatus::Passed,
        "{validation:?}"
    );
}
fn compare(input: &Input, artifact: &Artifact) {
    let mut raw_host = MemoryHost::new(input, Vec::new());
    let raw = execute_bytecode(input, &mut raw_host, &ExecutionLimits::default()).unwrap();
    let mut host = MemoryHost::new(input, Vec::new());
    let actual = execute_structured(
        artifact.current().unwrap(),
        &mut host,
        &ExecutionLimits::default(),
    )
    .unwrap();
    assert_eq!(raw.outcome, actual.outcome);
    assert_eq!(raw.observations(), actual.observations());
    assert_eq!(raw_host, host);
}

#[test]
fn dispatcher_external_edge_assigns_phi_before_leaving_for_following_region() {
    let input = Input::new(
        vec![0x60, 9, 0x60, 5, 0x56, 0x5b, 0x5f, 0x55, 0x00],
        Fork::Osaka,
    );
    let mut artifact = decompile(&input, &Config::default());
    let ir = &artifact.current().unwrap().ir;
    assert_eq!(ir.blocks.len(), 2);
    let entry = ir.entry.unwrap();
    let edge = ir
        .edge(ir.block(entry).unwrap().terminator.edges()[0])
        .unwrap();
    let target = edge.to;
    let parameter = ir.block(target).unwrap().parameters[0];
    let dispatch = dispatcher(&artifact, entry);
    install(
        &mut artifact,
        node(
            1_000_010,
            StructuredKind::Sequence(vec![
                dispatch,
                node(1_000_011, StructuredKind::Block { block: target }),
                node(1_000_012, StructuredKind::Exit { block: target }),
            ]),
        ),
    );
    compare(&input, &artifact);
    let text = render_text(&artifact);
    let phi = text.find(&format!("(v{}) = (0x9);", parameter.0)).unwrap();
    let escape = text
        .find(&format!("break 'dispatch{};", DISPATCH.0))
        .expect("external case target must leave its dispatcher");
    assert!(
        phi < escape && escape < text.find("sstore(").unwrap(),
        "{text}"
    );
    assert!(!text.contains(&format!(
        "state_d{} = b{}; continue 'dispatch{};",
        DISPATCH.0, target.0, DISPATCH.0
    )));
    eprintln!("{text}");
}

#[test]
fn dispatcher_external_edges_preserve_enclosing_loop_continue_and_break() {
    // The header increments storage[0]. The dispatcher returns to the header
    // once, then escapes to STOP after the counter reaches two.
    let input = Input::new(
        vec![
            0x5b, 0x5f, 0x54, 0x60, 1, 0x01, 0x80, 0x5f, 0x55, 0x60, 2, 0x90, 0x10, 0x60, 16, 0x56,
            0x5b, 0x5f, 0x57, 0x00,
        ],
        Fork::Osaka,
    );
    let mut artifact = decompile(&input, &Config::default());
    let ir = &artifact.current().unwrap().ir;
    let header = ir.entry.unwrap();
    let edge = ir.block(header).unwrap().terminator.edges()[0];
    let middle = ir.edge(edge).unwrap().to;
    let exit = ir
        .blocks
        .iter()
        .find(|block| block.source.is_some_and(|span| span.pc == 19))
        .unwrap()
        .id;
    let dispatch = dispatcher(&artifact, middle);
    install(
        &mut artifact,
        node(
            1_000_010,
            StructuredKind::Sequence(vec![
                StructuredNode {
                    id: LOOP,
                    kind: StructuredKind::While {
                        header: Box::new(node(1_000_011, StructuredKind::Block { block: header })),
                        condition: None,
                        condition_nonzero: true,
                        body: Box::new(node(
                            1_000_012,
                            StructuredKind::Sequence(vec![
                                node(1_000_013, StructuredKind::Transfer { edge }),
                                dispatch,
                            ]),
                        )),
                        exit_edge: None,
                    },
                },
                node(1_000_014, StructuredKind::Block { block: exit }),
                node(1_000_015, StructuredKind::Exit { block: exit }),
            ]),
        ),
    );
    compare(&input, &artifact);
    let text = render_text(&artifact);
    assert!(
        text.contains(&format!("continue 'loop{};", LOOP.0)),
        "{text}"
    );
    assert!(text.contains(&format!("break 'loop{};", LOOP.0)), "{text}");
    eprintln!("{text}");
}

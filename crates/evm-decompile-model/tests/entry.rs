//! Continuation anchors are shared by execution, text and interactive consumers.
use evm_decompile_model::{Edge, EdgeKind, EntityId, Ir, StructuredKind, StructuredNode};

fn node(id: u64, kind: StructuredKind) -> StructuredNode {
    StructuredNode {
        id: EntityId(id),
        kind,
    }
}

#[test]
fn indexed_and_owned_queries_use_transfer_sources_and_skip_unanchored_syntax() {
    let edge = Edge {
        id: EntityId(10),
        from: EntityId(3),
        to: EntityId(7),
        kind: EdgeKind::Jump,
        arguments: Vec::new(),
        memory: EntityId(11),
        effect: EntityId(12),
    };
    let ir = Ir {
        edges: vec![edge],
        ..Ir::default()
    };
    let syntax = node(
        20,
        StructuredKind::Sequence(vec![
            node(21, StructuredKind::Sequence(Vec::new())),
            node(22, StructuredKind::Transfer { edge: EntityId(10) }),
            node(23, StructuredKind::Block { block: EntityId(7) }),
        ]),
    );
    assert_eq!(syntax.entry_block(&ir), Some(EntityId(3)));
    assert_eq!(
        syntax.entry_block_with(|id| (id == EntityId(10)).then_some(EntityId(3))),
        Some(EntityId(3))
    );
    assert_eq!(
        node(24, StructuredKind::Transfer { edge: EntityId(99) }).entry_block(&ir),
        None
    );
    let conditional = node(
        25,
        StructuredKind::If {
            condition: EntityId(26),
            then_branch: Box::new(node(27, StructuredKind::Block { block: EntityId(3) })),
            else_branch: Box::new(node(28, StructuredKind::Block { block: EntityId(7) })),
        },
    );
    assert_eq!(conditional.entry_block(&ir), None);
    let loop_node = node(
        29,
        StructuredKind::While {
            header: Box::new(node(30, StructuredKind::Sequence(Vec::new()))),
            condition: None,
            condition_nonzero: true,
            body: Box::new(syntax),
            exit_edge: None,
        },
    );
    assert_eq!(loop_node.entry_block(&ir), Some(EntityId(3)));
}

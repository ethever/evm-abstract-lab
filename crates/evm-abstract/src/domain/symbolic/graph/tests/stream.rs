use super::{
    AbstractValue, ByteStateId, ExprId, ExprLimits, GraphLimits, SlotStateId, SourceId, Symbol,
    SymbolicGraph, U256, c,
};
use crate::domain::symbolic::{ByteStateKind, SlotStateKind, StateBinding};

#[test]
fn wide_observations_share_children_across_walk_rename_and_export() {
    const WIDTH: usize = 4096;
    let token = ExprId::fresh().unwrap();
    let cell = ExprId::fresh().unwrap();
    let length = ExprId::input(u64::MAX, Symbol::CalldataLength);
    let source = SourceId::scoped(token.clone(), U256::from(0x123));
    let numeric = AbstractValue::top().numeric().clone();
    let limits = ExprLimits {
        max_nodes: 5 + WIDTH * (1 + numeric.work_size()),
        ..ExprLimits::default()
    };
    let bindings: Vec<_> = (0..WIDTH)
        .map(|index| StateBinding {
            index: U256::from(index),
            value: cell.clone(),
            numeric: numeric.clone(),
        })
        .collect();
    let bytes = ByteStateId::observed(
        source.clone(),
        length.clone(),
        bindings.clone(),
        limits.into(),
    )
    .unwrap();
    let slots = SlotStateId::observed(source, bindings, limits.into()).unwrap();
    assert_eq!(bytes.nodes(), 3 + WIDTH * (1 + numeric.work_size()));
    assert_eq!(slots.nodes(), bytes.nodes() - 1);
    let offset = c(0);
    let read = ExprId::byte_read(bytes.clone(), offset.clone(), limits).unwrap();
    let mut visited = Vec::new();
    read.visit(&mut |expression| visited.push(expression.clone()));
    assert_eq!(
        visited,
        [
            read.clone(),
            offset,
            cell.clone(),
            length.clone(),
            token.clone()
        ]
    );

    let graph = SymbolicGraph::bytes(&bytes, GraphLimits::default()).unwrap();
    assert_eq!((graph.root, graph.nodes.len()), (3, 4));
    let super::super::SymbolicGraphNode::BytesInput {
        source,
        length: size,
        bindings,
    } = &graph.nodes[3]
    else {
        panic!("observed bytes");
    };
    assert_eq!(
        (source.token, source.namespace, *size),
        (0, U256::from(0x123), 1)
    );
    assert_eq!(bindings.len(), WIDTH);
    assert!(bindings.iter().enumerate().all(|(index, binding)| {
        binding.index == U256::from(index) && binding.value == 2 && binding.numeric == numeric
    }));
    let graph = SymbolicGraph::slots(&slots, GraphLimits::default()).unwrap();
    assert_eq!((graph.root, graph.nodes.len()), (2, 3));

    let new_token = ExprId::fresh().unwrap();
    let new_cell = ExprId::fresh().unwrap();
    let replacements = [(token, new_token.clone()), (cell, new_cell.clone())]
        .into_iter()
        .collect();
    let bytes = bytes.rename_fresh(&replacements, limits).unwrap();
    let slots = slots.rename_fresh(&replacements, limits).unwrap();
    let ByteStateKind::Input {
        source,
        length: size,
        bindings,
    } = bytes.kind()
    else {
        panic!("renamed bytes");
    };
    assert_eq!(
        (source.token(), source.namespace(), size),
        (&new_token, U256::from(0x123), &length)
    );
    assert!(
        bindings
            .iter()
            .all(|binding| binding.value == new_cell && binding.numeric == numeric)
    );
    let SlotStateKind::Input { source, bindings } = slots.kind() else {
        panic!("renamed slots");
    };
    assert_eq!(source.token(), &new_token);
    assert_eq!(bindings.len(), WIDTH);
    assert!(
        bindings
            .iter()
            .all(|binding| binding.value == new_cell && binding.numeric == numeric)
    );
    assert_eq!(
        bytes.fresh_leaves(),
        [new_token.clone(), new_cell.clone()].into_iter().collect()
    );
    assert_eq!(
        slots.fresh_leaves(),
        [new_token, new_cell].into_iter().collect()
    );
}

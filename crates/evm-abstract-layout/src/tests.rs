use crate::{Edge, EdgeKind, Input, Label, Node, Rect, Size, layout};

fn node(id: usize) -> Node {
    Node {
        id,
        size: Size::new(140.0, 80.0),
        program: Some(0),
        block: id,
        frame_depth: Some(1),
        boundary: false,
    }
}

fn edge(id: usize, from: usize, to: usize) -> Edge {
    Edge {
        id,
        from,
        to,
        kind: EdgeKind::Jump,
        label: Label {
            text: format!("edge {id}"),
            size: Size::new(72.0, 16.0),
        },
    }
}

fn intersects(a: Rect, b: Rect) -> bool {
    a.origin.x < b.right()
        && b.origin.x < a.right()
        && a.origin.y < b.bottom()
        && b.origin.y < a.bottom()
}

#[test]
fn shortcut_label_is_between_its_real_endpoints_without_a_false_back_route() {
    let input = Input {
        nodes: vec![node(7), node(91), node(usize::MAX)],
        edges: vec![
            edge(13, 7, 91),
            edge(17, 7, usize::MAX),
            edge(usize::MAX, 91, usize::MAX),
        ],
    };
    let geometry = layout(&input).unwrap();
    let a = geometry.nodes[&91];
    let b = geometry.nodes[&usize::MAX];
    let route = &geometry.edges[&usize::MAX];
    assert!(a.bottom() < b.origin.y);
    assert!(route.label.origin.y >= a.bottom());
    assert!(route.label.bottom() <= b.origin.y);
    assert_eq!(
        (route.from, route.to, route.kind),
        (91, usize::MAX, EdgeKind::Jump)
    );
    assert!(
        route
            .path
            .iter()
            .all(|point| { point.y >= a.bottom() && point.y <= b.origin.y })
    );
    assert!(!intersects(route.label, a) && !intersects(route.label, b));
}

#[test]
fn same_graph_and_measured_content_always_produce_the_same_geometry() {
    let input = Input {
        nodes: vec![node(3), node(19), node(104)],
        edges: vec![edge(71, 3, 19), edge(89, 19, 104)],
    };
    let first = layout(&input).unwrap();
    assert_eq!(first, layout(&input).unwrap());
    assert_eq!(
        first.nodes.keys().copied().collect::<Vec<_>>(),
        [3, 19, 104]
    );
    assert_eq!(first.edges.keys().copied().collect::<Vec<_>>(), [71, 89]);
    assert!(!first.groups.is_empty());
}

#[test]
fn empty_graph_has_no_phantom_group_label_or_edge() {
    let geometry = layout(&Input::default()).unwrap();
    assert!(geometry.nodes.is_empty() && geometry.edges.is_empty() && geometry.groups.is_empty());
    assert!(geometry.bounds.is_none());
}

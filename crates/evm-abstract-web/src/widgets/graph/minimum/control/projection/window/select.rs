//! Window policy selects existing cards only. Entry is a forward explanation;
//! Nearby deliberately explores neighbors, and Full includes disconnected facts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::super::super::scope::{CardKind, GraphProjection, GraphWindow};

pub(super) fn point(graph: &GraphProjection, id: usize) -> bool {
    matches!(
        graph.cards[id].kind,
        CardKind::Source { .. } | CardKind::Receipt
    )
}

pub(super) fn core(graph: &GraphProjection) -> BTreeSet<usize> {
    let anchor = graph
        .anchor
        .filter(|&id| point(graph, id))
        .or_else(|| (0..graph.cards.len()).find(|&id| point(graph, id)));
    let Some(anchor) = anchor else {
        return BTreeSet::new();
    };
    let mut core = BTreeSet::from([anchor]);
    let immediate: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| edge.from == anchor)
        .map(|edge| edge.to)
        .collect();
    core.extend(immediate.iter().copied());
    // A call portal represents this actual call site. Its actual delivered
    // caller continuation is an immediate source alternative, not a callee BFS.
    for boundary in immediate.into_iter().filter(|&id| !point(graph, id)) {
        core.extend(
            graph
                .edges
                .iter()
                .filter(|edge| edge.from == boundary && point(graph, edge.to))
                .map(|edge| edge.to),
        );
    }
    core
}

pub(super) fn cards(
    graph: &GraphProjection,
    choice: GraphWindow,
    core: &BTreeSet<usize>,
) -> BTreeSet<usize> {
    match choice {
        GraphWindow::Entry => core.clone(),
        GraphWindow::FullSource => (0..graph.cards.len()).collect(),
        GraphWindow::Nearby { point_limit } => nearby(graph, point_limit.max(1)),
    }
}

fn nearby(graph: &GraphProjection, limit: usize) -> BTreeSet<usize> {
    let mut outgoing: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut incoming: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for edge in &graph.edges {
        outgoing.entry(edge.from).or_default().push(edge.to);
        incoming.entry(edge.to).or_default().push(edge.from);
    }
    let mut selected = BTreeSet::new();
    let mut queue: VecDeque<_> = graph
        .anchor
        .or_else(|| (0..graph.cards.len()).find(|&id| point(graph, id)))
        .into_iter()
        .collect();
    let mut points = 0;
    while let Some(id) = queue.pop_front() {
        if selected.contains(&id) || (point(graph, id) && points >= limit) {
            continue;
        }
        selected.insert(id);
        points += usize::from(point(graph, id));
        queue.extend(
            outgoing
                .get(&id)
                .into_iter()
                .flatten()
                .copied()
                .filter(|&next| point(graph, id) || point(graph, next)),
        );
        if point(graph, id) {
            queue.extend(
                incoming
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|&next| point(graph, next)),
            );
        }
    }
    // Keep real boundaries attached to selected points, never synthetic window
    // portals. Crossings beyond them remain exact neutral frontier records.
    let boundaries: Vec<_> = graph
        .edges
        .iter()
        .filter(|edge| selected.contains(&edge.from) && !point(graph, edge.to))
        .map(|edge| edge.to)
        .collect();
    selected.extend(boundaries);
    selected
}

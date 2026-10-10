//! Spatial containers over the complete displayed graph, without rewriting it.
//!
//! Source-block aggregates may connect edges from different native instances.
//! A displayed chain is therefore a layout group, not proof of a composable
//! native execution path. Every original leaf and edge remains independent.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Group, GroupId, GroupKind, Input, LayoutError};

#[cfg(test)]
mod tests;

struct Graph {
    incoming: Vec<Vec<usize>>,
    outgoing: Vec<Vec<usize>>,
    endpoints: Vec<(usize, usize)>,
    local_forward: Vec<Vec<usize>>,
    local_reverse: Vec<Vec<usize>>,
    self_loops: Vec<bool>,
}

/// Validate identities and sizes before any upstream arena or property access.
pub(super) fn identify(input: &Input) -> Result<Vec<Group>, LayoutError> {
    capacity(input.nodes.len(), input.edges.len(), 0)?;
    let graph = validate(input)?;
    let mut groups = Vec::new();
    let program_groups = programs(input, &mut groups);
    let cycles = cycles(&graph);
    let mut cyclic = vec![false; input.nodes.len()];
    for members in cycles {
        for &position in &members {
            cyclic[position] = true;
        }
        let parent = input.nodes[members[0]]
            .program
            .and_then(|program| program_groups.get(&program).copied());
        push_group(
            &mut groups,
            parent,
            GroupKind::Cycle,
            members
                .iter()
                .map(|&position| input.nodes[position].id)
                .collect(),
        );
    }
    chains(input, &graph, &cyclic, &program_groups, &mut groups);
    capacity(input.nodes.len(), input.edges.len(), groups.len())?;
    Ok(groups)
}

fn capacity(nodes: usize, edges: usize, groups: usize) -> Result<(), LayoutError> {
    // ELK has separate node, edge and label arenas. Containers and the root are
    // nodes; each original edge and each container receives one annotation.
    let nodes = nodes
        .checked_add(groups)
        .and_then(|count| count.checked_add(1))
        .ok_or(LayoutError::TooLarge)?;
    let labels = edges.checked_add(groups).ok_or(LayoutError::TooLarge)?;
    for count in [nodes, edges, labels] {
        u32::try_from(count).map_err(|_| LayoutError::TooLarge)?;
    }
    Ok(())
}

fn validate(input: &Input) -> Result<Graph, LayoutError> {
    let mut positions = BTreeMap::new();
    for (position, node) in input.nodes.iter().enumerate() {
        if positions.insert(node.id, position).is_some() {
            return Err(LayoutError::DuplicateNode(node.id));
        }
        if !node.size.is_valid() {
            return Err(LayoutError::InvalidSize {
                element: "node",
                id: node.id,
            });
        }
    }
    let mut graph = Graph {
        incoming: vec![Vec::new(); input.nodes.len()],
        outgoing: vec![Vec::new(); input.nodes.len()],
        endpoints: Vec::with_capacity(input.edges.len()),
        local_forward: vec![Vec::new(); input.nodes.len()],
        local_reverse: vec![Vec::new(); input.nodes.len()],
        self_loops: vec![false; input.nodes.len()],
    };
    let mut identities = BTreeSet::new();
    for (position, edge) in input.edges.iter().enumerate() {
        if !identities.insert(edge.id) {
            return Err(LayoutError::DuplicateEdge(edge.id));
        }
        if !edge.label.size.is_valid() {
            return Err(LayoutError::InvalidSize {
                element: "edge label",
                id: edge.id,
            });
        }
        let endpoint = |node| {
            positions
                .get(&node)
                .copied()
                .ok_or(LayoutError::MissingNode {
                    edge: edge.id,
                    node,
                })
        };
        let from = endpoint(edge.from)?;
        let to = endpoint(edge.to)?;
        graph.endpoints.push((from, to));
        graph.outgoing[from].push(position);
        graph.incoming[to].push(position);
        if edge.kind.local_flow() && input.nodes[from].program == input.nodes[to].program {
            graph.local_forward[from].push(to);
            graph.local_reverse[to].push(from);
            graph.self_loops[from] |= from == to;
        }
    }
    Ok(graph)
}

fn push_group(
    groups: &mut Vec<Group>,
    parent: Option<GroupId>,
    kind: GroupKind,
    members: Vec<usize>,
) -> GroupId {
    let id = GroupId(groups.len());
    groups.push(Group {
        id,
        parent,
        kind,
        members,
    });
    id
}

fn programs(input: &Input, groups: &mut Vec<Group>) -> BTreeMap<usize, GroupId> {
    let mut members = BTreeMap::<usize, Vec<usize>>::new();
    for node in &input.nodes {
        if let Some(program) = node.program {
            members.entry(program).or_default().push(node.id);
        }
    }
    if members.len() < 2 {
        return BTreeMap::new();
    }
    members
        .into_iter()
        .filter_map(|(program, members)| {
            (members.len() >= 2).then(|| {
                (
                    program,
                    push_group(groups, None, GroupKind::Program(program), members),
                )
            })
        })
        .collect()
}

/// Iterative Kosaraju keeps long chains and cycles off the Wasm call stack.
fn cycles(graph: &Graph) -> Vec<Vec<usize>> {
    let mut visited = vec![false; graph.local_forward.len()];
    let mut finished = Vec::with_capacity(visited.len());
    let mut stack = Vec::new();
    for root in 0..visited.len() {
        if visited[root] {
            continue;
        }
        visited[root] = true;
        stack.push((root, 0));
        while let Some((node, next)) = stack.last_mut() {
            if let Some(&target) = graph.local_forward[*node].get(*next) {
                *next += 1;
                if !visited[target] {
                    visited[target] = true;
                    stack.push((target, 0));
                }
            } else {
                finished.push(*node);
                stack.pop();
            }
        }
    }
    visited.fill(false);
    let mut cycles = Vec::new();
    let mut pending = Vec::new();
    for root in finished.into_iter().rev() {
        if visited[root] {
            continue;
        }
        let mut members = Vec::new();
        visited[root] = true;
        pending.push(root);
        while let Some(node) = pending.pop() {
            members.push(node);
            for &target in &graph.local_reverse[node] {
                if !visited[target] {
                    visited[target] = true;
                    pending.push(target);
                }
            }
        }
        if members.len() > 1 || graph.self_loops[root] {
            members.sort_unstable();
            cycles.push(members);
        }
    }
    cycles.sort_unstable_by_key(|members| members[0]);
    cycles
}

fn chains(
    input: &Input,
    graph: &Graph,
    cyclic: &[bool],
    programs: &BTreeMap<usize, GroupId>,
    groups: &mut Vec<Group>,
) {
    let mut successor = vec![None; input.nodes.len()];
    let mut predecessor = vec![None; input.nodes.len()];
    for (edge, &(from, to)) in input.edges.iter().zip(&graph.endpoints) {
        let source = &input.nodes[from];
        let target = &input.nodes[to];
        if from == to
            || cyclic[from]
            || cyclic[to]
            || graph.outgoing[from].len() != 1
            || graph.incoming[to].len() != 1
            || !edge.kind.local_flow()
            || source.program != target.program
            || source.frame_depth.is_none()
            || source.frame_depth != target.frame_depth
            || source.boundary
            || target.boundary
        {
            continue;
        }
        successor[from] = Some(to);
        predecessor[to] = Some(from);
    }
    let mut visited = vec![false; input.nodes.len()];
    for start in 0..input.nodes.len() {
        if visited[start] || predecessor[start].is_some() || successor[start].is_none() {
            continue;
        }
        let mut members = Vec::new();
        let mut current = Some(start);
        while let Some(position) = current {
            if visited[position] {
                break;
            }
            visited[position] = true;
            members.push(input.nodes[position].id);
            current = successor[position];
        }
        if members.len() >= 2 {
            let parent = input.nodes[start]
                .program
                .and_then(|program| programs.get(&program).copied());
            push_group(groups, parent, GroupKind::Chain, members);
        }
    }
}

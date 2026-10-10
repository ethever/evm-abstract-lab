//! Display-only quotient graphs. Members always refer to separate native states;
//! folding a source block neither combines its SSA nor proves a composite path.

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use evm_abstract_protocol::{AnalysisReport, CfgBlock, CfgEdge, EdgeKind};

use super::index::ReportIndex;

const LOCAL_STATE_LIMIT: usize = 200;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GraphMode {
    #[default]
    Blocks,
    States,
    Local,
}

pub(super) struct DisplayNode {
    /// Representative native ID used only as a stable display key.
    pub id: usize,
    pub members: Vec<usize>,
    pub program: Option<usize>,
    pub basic_block: usize,
    pub start_pc: Option<usize>,
}

#[derive(Default)]
pub(super) struct DisplayGraph {
    pub nodes: Vec<DisplayNode>,
    pub edges: Vec<CfgEdge>,
    pub edge_members: BTreeMap<usize, Vec<usize>>,
    pub state_to_node: BTreeMap<usize, usize>,
    pub hidden_states: usize,
    pub boundary_edges: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum GroupKey {
    Block {
        program: usize,
        block: usize,
        pc: usize,
    },
    State(usize),
}

impl GroupKey {
    fn for_state(state: &CfgBlock, mode: GraphMode) -> Self {
        match (mode, state.program, state.start_pc) {
            (GraphMode::Blocks, Some(program), Some(pc)) => Self::Block {
                program,
                block: state.basic_block,
                pc,
            },
            _ => Self::State(state.id),
        }
    }
}

impl DisplayGraph {
    pub(super) fn build(
        report: &AnalysisReport,
        index: &ReportIndex,
        mode: GraphMode,
        program: Option<usize>,
        center: Option<usize>,
        hops: usize,
    ) -> Self {
        // Local explicitly crosses program boundaries: filtering it would hide
        // the CALL/RETURN neighbors it is intended to explain.
        let local = (mode == GraphMode::Local).then(|| neighborhood(report, index, center, hops));
        let mut graph = Self::default();
        let mut groups = BTreeMap::new();
        for state in &report.cfg {
            let visible = match &local {
                Some(states) => states.contains(&state.id),
                None => program.is_none() || state.program == program,
            };
            if !visible {
                continue;
            }
            let position = *groups
                .entry(GroupKey::for_state(state, mode))
                .or_insert_with(|| {
                    let position = graph.nodes.len();
                    graph.nodes.push(DisplayNode {
                        id: state.id,
                        members: Vec::new(),
                        program: state.program,
                        basic_block: state.basic_block,
                        start_pc: state.start_pc,
                    });
                    position
                });
            let node = &mut graph.nodes[position];
            node.members.push(state.id);
            graph.state_to_node.insert(state.id, node.id);
        }
        graph.hidden_states = report.cfg.len().saturating_sub(graph.state_to_node.len());
        let mut edge_groups = BTreeMap::new();
        for edge in &report.edges {
            // Malformed dangling endpoints must not turn into display nodes or
            // be reported as hidden native neighbors.
            if index.cfg(report, edge.from).is_none() || index.cfg(report, edge.to).is_none() {
                continue;
            }
            let from = graph.state_to_node.get(&edge.from).copied();
            let to = graph.state_to_node.get(&edge.to).copied();
            let (Some(from), Some(to)) = (from, to) else {
                graph.boundary_edges += usize::from(from.is_some() != to.is_some());
                continue;
            };
            let representative = if mode == GraphMode::Blocks {
                *edge_groups
                    .entry((from, to, edge_kind_key(edge.kind)))
                    .or_insert(edge.id)
            } else {
                edge.id
            };
            let members = graph.edge_members.entry(representative).or_default();
            if members.is_empty() {
                graph.edges.push(CfgEdge {
                    id: representative,
                    from,
                    to,
                    kind: edge.kind,
                });
            }
            members.push(edge.id);
        }
        graph
    }
}

fn neighborhood(
    report: &AnalysisReport,
    index: &ReportIndex,
    center: Option<usize>,
    hops: usize,
) -> BTreeSet<usize> {
    let center = center
        .filter(|state| index.cfg(report, *state).is_some())
        .or_else(|| report.cfg.first().map(|state| state.id));
    let Some(center) = center else {
        return BTreeSet::new();
    };
    let mut states = BTreeSet::from([center]);
    let mut queue = VecDeque::from([(center, 0)]);
    while let Some((state, depth)) = queue.pop_front() {
        if depth >= hops {
            continue;
        }
        for position in index.outgoing(state).iter().chain(index.incoming(state)) {
            let edge = &report.edges[*position];
            let neighbor = if edge.from == state {
                edge.to
            } else {
                edge.from
            };
            if states.len() == LOCAL_STATE_LIMIT {
                return states;
            }
            if index.cfg(report, neighbor).is_some() && states.insert(neighbor) {
                queue.push_back((neighbor, depth + 1));
            }
        }
    }
    states
}

// The wire enum intentionally has no ordering contract. This exhaustive key
// keeps kinds distinct without grouping by display text or serialized strings.
fn edge_kind_key(kind: EdgeKind) -> u8 {
    match kind {
        EdgeKind::Fallthrough => 0,
        EdgeKind::Jump => 1,
        EdgeKind::BranchTrue => 2,
        EdgeKind::BranchFalse => 3,
        EdgeKind::Call => 4,
        EdgeKind::Return => 5,
        EdgeKind::Failure => 6,
        EdgeKind::Revert => 7,
    }
}

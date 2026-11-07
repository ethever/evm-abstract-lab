//! Stable source-graph facts. No fact allocates an owned syntax occurrence.
use crate::transform::reachable;
use crate::{EntityId, ExitKind, Ir, Progress, Terminator, artifact::Builder};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[cfg(test)]
mod tests;
#[derive(Clone)]
pub(super) struct Loop {
    pub header: EntityId,
    pub nodes: BTreeSet<EntityId>,
    pub exit: Option<EntityId>,
}

pub(super) struct Facts {
    pub postdom: BTreeMap<EntityId, BTreeSet<EntityId>>,
    pub loops: BTreeMap<EntityId, Loop>,
    pub finite: BTreeSet<EntityId>,
    pub failed: BTreeSet<EntityId>,
    pub normal_postdom: BTreeMap<EntityId, BTreeSet<EntityId>>,
    pub blocks: BTreeMap<EntityId, usize>,
    pub edges: BTreeMap<EntityId, usize>,
}
impl Facts {
    pub fn new(
        ir: &Ir,
        dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<Self> {
        let postdom = postdominators(ir, owner)?;
        let loops = natural_loops(ir, dom, &postdom, owner)?;
        if !owner.pay(
            ir.blocks
                .len()
                .saturating_add(ir.edges.len())
                .saturating_mul(16),
        ) {
            return None;
        }
        let blocks: BTreeMap<_, _> = ir
            .blocks
            .iter()
            .enumerate()
            .map(|(n, b)| (b.id, n))
            .collect();
        let edges: BTreeMap<_, _> = ir
            .edges
            .iter()
            .enumerate()
            .map(|(n, e)| (e.id, n))
            .collect();
        let mut successors = vec![Vec::new(); ir.blocks.len()];
        let mut predecessors = vec![Vec::new(); ir.blocks.len()];
        let mut known = vec![false; ir.blocks.len()];
        for (n, b) in ir.blocks.iter().enumerate() {
            known[n] = match &b.terminator {
                Terminator::Exit { .. }
                | Terminator::Trap { .. }
                | Terminator::Fallthrough { .. } => true,
                Terminator::Jump {
                    edges,
                    unresolved: false,
                    invalid_targets,
                    ..
                } => !edges.is_empty() && invalid_targets.is_empty(),
                Terminator::Branch {
                    then_edges,
                    else_edge: Some(_),
                    unresolved: false,
                    invalid_targets,
                    ..
                } => !then_edges.is_empty() && invalid_targets.is_empty(),
                _ => false,
            };
            for id in b.terminator.edges() {
                let Some(edge) = edges.get(&id).map(|i| &ir.edges[*i]) else {
                    known[n] = false;
                    continue;
                };
                let Some(to) = blocks.get(&edge.to) else {
                    known[n] = false;
                    continue;
                };
                if edge.from != b.id {
                    known[n] = false;
                }
                successors[n].push(*to);
                predecessors[*to].push(n);
            }
        }
        // Least well-founded closure: cycles, pending successors and indirect
        // escapes cannot become finite merely because a lexical terminal exists.
        let closure = |failed_only: bool, owner: &mut Builder<_>| -> Option<Vec<Option<usize>>> {
            let mut ranks = vec![None; ir.blocks.len()];
            let mut remaining: Vec<_> = successors.iter().map(Vec::len).collect();
            let mut largest = vec![0_usize; ir.blocks.len()];
            let mut todo = VecDeque::new();
            for (n, b) in ir.blocks.iter().enumerate() {
                let terminal = match b.terminator {
                    Terminator::Trap { .. } => true,
                    Terminator::Exit { kind, .. } => !failed_only || kind == ExitKind::Revert,
                    _ => false,
                };
                if terminal {
                    ranks[n] = Some(0);
                    todo.push_back(n);
                }
            }
            while let Some(done) = todo.pop_front() {
                if !owner.pay(predecessors[done].len().saturating_add(1)) {
                    return None;
                }
                for &p in &predecessors[done] {
                    if ranks[p].is_some() || !known[p] {
                        continue;
                    }
                    remaining[p] -= 1;
                    largest[p] = largest[p].max(ranks[done]?);
                    if remaining[p] == 0 {
                        ranks[p] = Some(largest[p].saturating_add(1));
                        todo.push_back(p);
                    }
                }
            }
            Some(ranks)
        };
        let ranks = closure(false, owner)?;
        let failed_ranks = closure(true, owner)?;
        let finite = ir
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(n, b)| ranks[n].map(|_| b.id))
            .collect();
        let failed: BTreeSet<_> = ir
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(n, b)| failed_ranks[n].map(|_| b.id))
            .collect();
        let mut order: Vec<_> = ir
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(n, b)| ranks[n].map(|rank| (rank, n, b.id)))
            .collect();
        if !owner.pay(
            ir.blocks
                .len()
                .saturating_mul(ir.blocks.len())
                .saturating_mul(8),
        ) {
            return None;
        }
        order.sort_unstable();
        let mut normal_postdom = BTreeMap::<EntityId, BTreeSet<EntityId>>::from([(
            EntityId(0),
            BTreeSet::from([EntityId(0)]),
        )]);
        for (_, n, id) in order {
            if failed.contains(&id) {
                continue;
            }
            let mut normal: Vec<_> = successors[n]
                .iter()
                .map(|to| ir.blocks[*to].id)
                .filter(|to| !failed.contains(to))
                .collect();
            if normal.is_empty() {
                normal.push(EntityId(0));
            }
            let mut post = normal_postdom[&normal[0]].clone();
            for to in normal.iter().skip(1) {
                post = post.intersection(&normal_postdom[to]).copied().collect();
            }
            post.insert(id);
            normal_postdom.insert(id, post);
        }
        Some(Self {
            postdom,
            loops,
            finite,
            failed,
            normal_postdom,
            blocks,
            edges,
        })
    }
}
pub(super) fn natural_loops(
    ir: &Ir,
    dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
    postdom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<BTreeMap<EntityId, Loop>> {
    let graph_work = ir
        .blocks
        .len()
        .saturating_mul(ir.blocks.len().saturating_add(ir.edges.len()))
        .max(1);
    let mut loops: BTreeMap<EntityId, Loop> = BTreeMap::new();
    for edge in &ir.edges {
        if !dom[&edge.from].contains(&edge.to) {
            continue;
        }
        if !owner.pay(graph_work) {
            return None;
        }
        let header = edge.to;
        let mut nodes = BTreeSet::from([header]);
        let mut todo = vec![edge.from];
        while let Some(id) = todo.pop() {
            if !nodes.insert(id) {
                continue;
            }
            for e in ir.edges.iter().filter(|e| e.to == id) {
                todo.push(e.from);
            }
        }
        loops
            .entry(header)
            .or_insert_with(|| Loop {
                header,
                nodes: BTreeSet::new(),
                exit: None,
            })
            .nodes
            .extend(nodes);
    }
    loops.retain(|_, lp| {
        if !owner.pay(graph_work) {
            return false;
        }
        // Terminal tails are part of the loop body: returns/reverts/STOP must
        // remain executable exits rather than forcing a dispatcher around a
        // reducible loop. Acyclic exit bridges can share a single follow.
        loop {
            let exits: BTreeSet<_> = ir
                .edges
                .iter()
                .filter(|e| lp.nodes.contains(&e.from) && !lp.nodes.contains(&e.to))
                .map(|e| e.to)
                .collect();
            let terminal: Vec<_> = exits
                .iter()
                .filter(|id| {
                    ir.block(**id).is_some_and(|b| {
                        b.terminator.edges().is_empty() && !b.terminator.is_partial()
                    })
                })
                .copied()
                .collect();
            if terminal.is_empty() {
                break;
            }
            lp.nodes.extend(terminal);
        }
        let exits: BTreeSet<_> = ir
            .edges
            .iter()
            .filter(|e| lp.nodes.contains(&e.from) && !lp.nodes.contains(&e.to))
            .map(|e| e.to)
            .collect();
        if exits.len() <= 1 {
            lp.exit = exits.first().copied();
            return true;
        }
        let mut common = postdom[exits.first().expect("nonempty exits")].clone();
        for id in exits.iter().skip(1) {
            common = common.intersection(&postdom[id]).copied().collect();
        }
        if !owner.pay(graph_work.saturating_mul(exits.len())) {
            return false;
        }
        let exit_distances: Vec<_> = exits.iter().map(|exit| distances(ir, *exit)).collect();
        let follow = common
            .into_iter()
            .filter(|id| id.0 != 0 && !lp.nodes.contains(id))
            .filter_map(|id| {
                let distance = exit_distances
                    .iter()
                    .map(|d| d.get(&id).copied())
                    .collect::<Option<Vec<_>>>()?;
                Some((distance.into_iter().max().unwrap_or(0), id))
            })
            .min()
            .map(|(_, id)| id);
        let Some(follow) = follow else {
            return false;
        };
        for exit in exits {
            let bridge = reachable(ir, Some(exit), Some(follow));
            if bridge.contains(&lp.header) {
                return false;
            }
            lp.nodes.extend(bridge);
        }
        lp.exit = Some(follow);
        true
    });
    (!owner.stopped).then_some(loops)
}
pub(super) fn postdominators(
    ir: &Ir,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<BTreeMap<EntityId, BTreeSet<EntityId>>> {
    if !owner.pay(
        ir.blocks
            .len()
            .saturating_mul(ir.blocks.len())
            .saturating_mul(16),
    ) {
        return None;
    }
    let exit = EntityId(0);
    let mut all: BTreeSet<_> = ir.blocks.iter().map(|b| b.id).collect();
    all.insert(exit);
    let mut post: BTreeMap<_, _> = all
        .iter()
        .map(|id| {
            (
                *id,
                if *id == exit {
                    BTreeSet::from([exit])
                } else {
                    all.clone()
                },
            )
        })
        .collect();
    loop {
        if !owner.pay(
            all.len()
                .saturating_mul(all.len().saturating_add(ir.edges.len()))
                .saturating_add(1),
        ) {
            return None;
        }
        let mut changed = false;
        for b in &ir.blocks {
            let mut successors: Vec<_> = b
                .terminator
                .edges()
                .iter()
                .filter_map(|id| ir.edge(*id).map(|e| e.to))
                .collect();
            let exceptional = match &b.terminator {
                Terminator::Jump {
                    unresolved,
                    invalid_targets,
                    ..
                } => *unresolved || !invalid_targets.is_empty(),
                Terminator::Branch {
                    unresolved,
                    invalid_targets,
                    else_edge,
                    ..
                } => *unresolved || !invalid_targets.is_empty() || else_edge.is_none(),
                _ => false,
            };
            if successors.is_empty() || exceptional {
                successors.push(exit);
            }
            let mut next = post[&successors[0]].clone();
            for id in successors.iter().skip(1) {
                next = next.intersection(&post[id]).copied().collect();
            }
            next.insert(b.id);
            if post[&b.id] != next {
                post.insert(b.id, next);
                changed = true;
            }
        }
        if !changed {
            return Some(post);
        }
    }
}
pub(super) fn distances(ir: &Ir, start: EntityId) -> BTreeMap<EntityId, usize> {
    let mut dist = BTreeMap::from([(start, 0)]);
    let mut queue = VecDeque::from([start]);
    while let Some(id) = queue.pop_front() {
        let depth = dist[&id] + 1;
        for e in ir.edges.iter().filter(|e| e.from == id) {
            if let std::collections::btree_map::Entry::Vacant(slot) = dist.entry(e.to) {
                slot.insert(depth);
                queue.push_back(e.to);
            }
        }
    }
    dist
}
pub(super) fn reaches_cycle_without_natural_header(
    ir: &Ir,
    start: EntityId,
    loops: &BTreeMap<EntityId, Loop>,
    active: Option<&Loop>,
) -> bool {
    // A node lies in a residual cycle if it can return to itself without passing
    // a natural header other than the currently emitted loop's own header.
    if loops.contains_key(&start) {
        return false;
    }
    let mut seen = BTreeSet::new();
    let mut todo: Vec<_> = ir
        .edges
        .iter()
        .filter(|e| e.from == start)
        .map(|e| e.to)
        .collect();
    while let Some(id) = todo.pop() {
        if id == start {
            return true;
        }
        if !seen.insert(id) || loops.contains_key(&id) || active.is_some_and(|lp| id == lp.header) {
            continue;
        }
        todo.extend(ir.edges.iter().filter(|e| e.from == id).map(|e| e.to));
    }
    false
}

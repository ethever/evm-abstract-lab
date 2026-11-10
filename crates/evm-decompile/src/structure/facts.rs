//! Stable source-graph facts. No fact allocates an owned syntax occurrence.
use crate::transform::reachable;
use crate::{EntityId, ExitKind, Ir, Progress, Terminator, artifact::Builder};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
mod region;
pub(super) use region::{NormalRegion, RegionScope};
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
    // Exact terminator successors, shared by all indexed planning queries.
    pub successors: Vec<Vec<usize>>,
    predecessors: Vec<Vec<usize>>,
    known: Vec<bool>,
}
impl Facts {
    pub fn new(
        ir: &Ir,
        dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
        owner: &mut Builder<impl FnMut(Progress)>,
    ) -> Option<Self> {
        let postdom = postdominators(ir, owner)?;
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
        let mut facts = Self {
            postdom,
            loops: BTreeMap::new(),
            finite,
            failed,
            normal_postdom,
            blocks,
            edges,
            successors,
            predecessors,
            known,
        };
        facts.loops = natural_loops(ir, dom, &facts, owner)?;
        Some(facts)
    }

    /// A source continuation outside the cyclic component. This is a safe
    /// boundary candidate, not a claim that the resulting dispatch is minimal.
    /// Strict postdominance keeps failure/unknown exits out of this shortcut.
    pub fn residual_continuation(
        &self,
        ir: &Ir,
        start: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<Option<EntityId>> {
        let distances = self.indexed_distances(start, pay)?;
        if !pay(self.blocks.len().saturating_mul(3)) {
            return None;
        }
        let root = *self.blocks.get(&start)?;
        let mut reaches_entry = vec![false; self.blocks.len()];
        let mut pending = vec![root];
        while let Some(index) = pending.pop() {
            if reaches_entry[index] {
                continue;
            }
            reaches_entry[index] = true;
            if !pay(self.predecessors[index].len().saturating_add(1)) {
                return None;
            }
            pending.extend_from_slice(&self.predecessors[index]);
        }
        // Forward-and-reverse reachability identifies the actual source SCC.
        // A postdominator outside it may still require exit bridges in dispatch.
        let component: Vec<_> = ir
            .blocks
            .iter()
            .enumerate()
            .filter(|(index, _)| distances[*index].is_some() && reaches_entry[*index])
            .map(|(_, block)| block.id)
            .collect();
        let mut candidates = self.postdom[&start].clone();
        for member in &component {
            if !pay(candidates.len().saturating_add(1)) {
                return None;
            }
            candidates.retain(|candidate| self.postdom[member].contains(candidate));
        }
        Some(
            candidates
                .into_iter()
                .filter_map(|candidate| {
                    let index = *self.blocks.get(&candidate)?;
                    (!reaches_entry[index]).then_some((distances[index]?, candidate))
                })
                .min()
                .map(|(_, candidate)| candidate),
        )
    }

    pub fn indexed_distances(
        &self,
        start: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<Vec<Option<usize>>> {
        if !pay(self.successors.len().saturating_mul(2)) {
            return None;
        }
        let mut distances = vec![None; self.successors.len()];
        let Some(&start) = self.blocks.get(&start) else {
            return Some(distances);
        };
        distances[start] = Some(0);
        let mut queue = VecDeque::from([start]);
        while let Some(from) = queue.pop_front() {
            if !pay(self.successors[from].len().saturating_add(1)) {
                return None;
            }
            let next = distances[from].unwrap() + 1;
            for &to in &self.successors[from] {
                if distances[to].is_none() {
                    distances[to] = Some(next);
                    queue.push_back(to);
                }
            }
        }
        Some(distances)
    }

    pub fn indexed_residual_cycle(
        &self,
        ir: &Ir,
        start: EntityId,
        active: Option<EntityId>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<bool> {
        if self.loops.contains_key(&start) {
            return Some(false);
        }
        let Some(&root) = self.blocks.get(&start) else {
            return Some(false);
        };
        if !pay(self
            .successors
            .len()
            .saturating_add(self.successors[root].len()))
        {
            return None;
        }
        let mut seen = vec![false; self.successors.len()];
        let mut todo = self.successors[root].clone();
        while let Some(index) = todo.pop() {
            if !pay(1) {
                return None;
            }
            if index == root {
                return Some(true);
            }
            let id = ir.blocks[index].id;
            if seen[index] || self.loops.contains_key(&id) || active == Some(id) {
                continue;
            }
            seen[index] = true;
            if !pay(self.successors[index].len()) {
                return None;
            }
            todo.extend_from_slice(&self.successors[index]);
        }
        Some(false)
    }
}
pub(super) fn natural_loops(
    ir: &Ir,
    dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
    facts: &Facts,
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
        // Keep executable terminal exits and complete finite failure DAGs in
        // the body. The least failure closure excludes pending/unknown paths
        // and cycles; ordinary finite suffixes stay available as shared follows.
        if !owner.pay(
            ir.blocks
                .len()
                .saturating_add(ir.edges.len())
                .saturating_mul(16),
        ) {
            return false;
        }
        let mut seen = vec![false; ir.blocks.len()];
        let mut exits = BTreeSet::new();
        for id in &lp.nodes {
            let index = facts.blocks[id];
            seen[index] = true;
            exits.extend(
                facts.successors[index]
                    .iter()
                    .copied()
                    .filter(|to| !lp.nodes.contains(&ir.blocks[*to].id)),
            );
        }
        let mut todo = Vec::new();
        for index in exits {
            let block = &ir.blocks[index];
            if facts.successors[index].is_empty() && !block.terminator.is_partial() {
                lp.nodes.insert(block.id);
                seen[index] = true;
            } else {
                todo.push(index);
            }
        }
        let mut failed_tails = Some(todo);
        loop {
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
            let Some(follow) = common_loop_follow(ir, lp, &exits, facts, graph_work, owner) else {
                return false;
            };
            if let Some(follow) = follow {
                for exit in exits {
                    let bridge = reachable(ir, Some(exit), Some(follow));
                    if bridge.contains(&lp.header) {
                        return false;
                    }
                    lp.nodes.extend(bridge);
                }
                lp.exit = Some(follow);
                return true;
            }
            // Preserve an already valid single/common follow, even when its
            // suffix eventually fails. Expand failure tails only to recover an
            // otherwise rejected loop, then retry the same follow criteria.
            let Some(mut todo) = failed_tails.take() else {
                return false;
            };
            let original_nodes = lp.nodes.len();
            while let Some(index) = todo.pop() {
                if !owner.pay(1) {
                    return false;
                }
                if seen[index] {
                    continue;
                }
                seen[index] = true;
                let block = &ir.blocks[index];
                if !facts.failed.contains(&block.id) {
                    continue;
                }
                if !owner.pay(facts.successors[index].len().saturating_add(1)) {
                    return false;
                }
                lp.nodes.insert(block.id);
                todo.extend_from_slice(&facts.successors[index]);
            }
            if lp.nodes.len() == original_nodes {
                return false;
            }
        }
    });
    (!owner.stopped).then_some(loops)
}

fn common_loop_follow(
    ir: &Ir,
    lp: &Loop,
    exits: &BTreeSet<EntityId>,
    facts: &Facts,
    graph_work: usize,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<Option<EntityId>> {
    if !owner.pay(graph_work.saturating_mul(exits.len())) {
        return None;
    }
    let mut common = facts.postdom[exits.first().expect("nonempty exits")].clone();
    for id in exits.iter().skip(1) {
        common = common.intersection(&facts.postdom[id]).copied().collect();
    }
    let exit_distances: Vec<_> = exits.iter().map(|exit| distances(ir, *exit)).collect();
    Some(
        common
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
            .map(|(_, id)| id),
    )
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

//! Natural if/while regions with executable residual dispatcher lowering.
use crate::{
    Completion, DiagnosticCode, DispatchCase, DispatchTransition, EntityId, IndirectArm, Ir,
    Progress, Severity, StageKind, Strategy, StructuredKind, StructuredNode, StructuredProgram,
    Terminator,
    artifact::Builder,
    transform::{dominators, ir_size, irreducible, reachable},
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Clone)]
struct Loop {
    header: EntityId,
    nodes: BTreeSet<EntityId>,
    exit: Option<EntityId>,
}
pub(crate) fn run(owner: &mut Builder<impl FnMut(Progress)>) {
    if owner.stopped {
        return;
    }
    if !owner.pay(ir_size(&owner.current().ir).saturating_mul(4)) {
        return;
    }
    let ir = owner.current().ir.clone();
    let Some(dom) = dominators(&ir, owner) else {
        return;
    };
    if owner.artifact.config.strategy == Strategy::CodeDuplication
        && irreducible(&ir, &dom).is_some()
    {
        owner.partial();
        owner.diagnostic(DiagnosticCode::Unsupported,Severity::Warning,None,None,"selected code duplication did not discharge irreducibility; prior CFG retained without dispatcher fallback".to_owned());
        return;
    }
    let Some(postdom) = postdominators(&ir, owner) else {
        return;
    };
    let loops = natural_loops(&ir, &dom, &postdom);
    let mut s = Structurer {
        owner,
        ir,
        loops,
        postdom,
        parents: BTreeMap::new(),
    };
    let root = if let Some(entry) = s.ir.entry {
        s.build(entry, None, None, 0, &BTreeSet::new())
    } else {
        s.node(StructuredKind::Sequence(Vec::new()), Vec::new())
    };
    let Some(root) = root else {
        return;
    };
    if ast_depth(&root) > 20 {
        s.owner.stop(
            Completion::Limited,
            DiagnosticCode::Limit,
            "structured AST wire nesting ceiling exceeded; prior validated stage retained",
        );
        return;
    }
    s.ir.structured = Some(StructuredProgram { root });
    let has_dispatch =
        contains_dispatcher(&s.ir.structured.as_ref().expect("constructed AST").root);
    if s.owner.artifact.config.strategy == Strategy::Dispatcher {
        if !s.owner.commit(StageKind::ControlFlow,s.ir.clone(),if has_dispatch{"dispatcher_lowering"}else{"natural_regions"},if has_dispatch{"residual control lowered to explicit state assignments while natural regions retain if/while"}else{"natural conditional and loop regions with explicit parameter transfers"},s.parents.clone(),Vec::new(),Vec::new()){return;}
    }
    s.owner.commit(
        StageKind::Structured,
        s.ir,
        "structure_regions",
        "inspectable executable AST preserves primitive bodies and ordered transfers",
        s.parents,
        Vec::new(),
        Vec::new(),
    );
}
fn contains_dispatcher(n: &StructuredNode) -> bool {
    match &n.kind {
        StructuredKind::Dispatcher { .. } => true,
        StructuredKind::Sequence(v) => v.iter().any(contains_dispatcher),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => contains_dispatcher(then_branch) || contains_dispatcher(else_branch),
        StructuredKind::While { header, body, .. } => {
            contains_dispatcher(header) || contains_dispatcher(body)
        }
        StructuredKind::Indirect { arms, .. } => arms.iter().any(|a| contains_dispatcher(&a.body)),
        _ => false,
    }
}
fn natural_loops(
    ir: &Ir,
    dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
    postdom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
) -> BTreeMap<EntityId, Loop> {
    let mut loops: BTreeMap<EntityId, Loop> = BTreeMap::new();
    for edge in &ir.edges {
        if !dom[&edge.from].contains(&edge.to) {
            continue;
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
        let follow = common
            .into_iter()
            .filter(|id| id.0 != 0 && !lp.nodes.contains(id))
            .filter_map(|id| {
                let distance = exits
                    .iter()
                    .map(|e| distances(ir, *e).get(&id).copied())
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
            if bridge.iter().any(|id| !dom[id].contains(&lp.header)) {
                return false;
            }
            lp.nodes.extend(bridge);
        }
        lp.exit = Some(follow);
        true
    });
    loops
}
fn postdominators(
    ir: &Ir,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<BTreeMap<EntityId, BTreeSet<EntityId>>> {
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
struct Structurer<'a, P: FnMut(Progress)> {
    owner: &'a mut Builder<P>,
    ir: Ir,
    loops: BTreeMap<EntityId, Loop>,
    postdom: BTreeMap<EntityId, BTreeSet<EntityId>>,
    parents: BTreeMap<EntityId, Vec<EntityId>>,
}
impl<P: FnMut(Progress)> Structurer<'_, P> {
    fn node(&mut self, kind: StructuredKind, mut parents: Vec<EntityId>) -> Option<StructuredNode> {
        if !self.owner.pay(1) {
            return None;
        }
        parents.extend(kind_sources(&kind));
        parents.sort();
        parents.dedup();
        let id = self.owner.id();
        self.parents.insert(id, parents);
        Some(StructuredNode { id, kind })
    }
    fn sequence(&mut self, nodes: Vec<StructuredNode>) -> Option<StructuredNode> {
        let mut flat = Vec::new();
        for node in nodes {
            match node.kind {
                StructuredKind::Sequence(children) => flat.extend(children),
                kind => flat.push(StructuredNode { id: node.id, kind }),
            }
        }
        self.node(StructuredKind::Sequence(flat), Vec::new())
    }
    fn build(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<StructuredNode> {
        if Some(start) == stop {
            return self.sequence(Vec::new());
        }
        if depth > self.owner.limits().max_structuring_depth {
            self.owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "structuring depth limit reached; prior validated stage retained",
            );
            return None;
        }
        if path.contains(&start) {
            return self.dispatch(start, stop, lp);
        }
        if let Some(info) = self
            .loops
            .get(&start)
            .cloned()
            .filter(|info| lp.is_none_or(|outer| info.header != outer.header))
        {
            return self.loop_region(info, stop, lp, depth, path);
        }
        self.core(start, stop, lp, depth, path)
    }
    fn core(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<StructuredNode> {
        let block = self.ir.block(start)?.clone();
        let mut visited = path.clone();
        visited.insert(start);
        // A residual cycle must be entered as a state machine before emitting
        // its first body; otherwise the first case's effects would execute twice.
        if reaches_cycle_without_natural_header(&self.ir, start, &self.loops, lp) {
            return self.dispatch(start, stop, lp);
        }
        let body = self.node(StructuredKind::Block { block: start }, vec![start])?;
        let tail = match block.terminator {
            Terminator::Fallthrough { edge } => {
                self.edge_path(edge, stop, lp, depth + 1, &visited)?
            }
            Terminator::Jump {
                edges,
                unresolved,
                invalid_targets,
                ..
            } => {
                if edges.len() == 1 && !unresolved && invalid_targets.is_empty() {
                    self.edge_path(edges[0], stop, lp, depth + 1, &visited)?
                } else {
                    self.indirect(start, &edges, unresolved, stop, lp, depth + 1, &visited)?
                }
            }
            Terminator::Branch {
                condition,
                then_edges,
                else_edge,
                unresolved,
                invalid_targets,
                ..
            } => {
                let simple = then_edges.len() == 1 && !unresolved && invalid_targets.is_empty();
                let join = if simple {
                    else_edge.and_then(|e| {
                        self.common_join(self.ir.edge(then_edges[0])?.to, self.ir.edge(e)?.to, lp)
                    })
                } else {
                    None
                };
                let arm_stop = join.or(stop);
                let then_branch = if simple {
                    self.edge_path(then_edges[0], arm_stop, lp, depth + 1, &visited)?
                } else {
                    self.indirect(
                        start,
                        &then_edges,
                        unresolved,
                        arm_stop,
                        lp,
                        depth + 1,
                        &visited,
                    )?
                };
                let else_branch = if let Some(edge) = else_edge {
                    self.edge_path(edge, arm_stop, lp, depth + 1, &visited)?
                } else {
                    self.node(StructuredKind::Exit { block: start }, vec![start])?
                };
                let branch = self.node(
                    StructuredKind::If {
                        condition,
                        then_branch: Box::new(then_branch),
                        else_branch: Box::new(else_branch),
                    },
                    vec![start, condition],
                )?;
                if let Some(join) = join.filter(|j| Some(*j) != stop) {
                    let follow = self.build(join, stop, lp, depth + 1, &visited)?;
                    self.sequence(vec![branch, follow])?
                } else {
                    branch
                }
            }
            Terminator::Pending => {
                self.node(StructuredKind::PartialExit { block: start }, vec![start])?
            }
            _ => self.node(StructuredKind::Exit { block: start }, vec![start])?,
        };
        self.sequence(vec![body, tail])
    }
    fn edge_path(
        &mut self,
        id: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<StructuredNode> {
        let edge = self.ir.edge(id)?.clone();
        let transfer = self.node(StructuredKind::Transfer { edge: id }, vec![id])?;
        let follow = if let Some(info) = lp {
            if edge.to == info.header {
                self.node(StructuredKind::Continue, vec![edge.from, edge.to])?
            } else if !info.nodes.contains(&edge.to) {
                self.node(StructuredKind::Break, vec![edge.from, edge.to])?
            } else {
                self.build(edge.to, stop, lp, depth, path)?
            }
        } else {
            self.build(edge.to, stop, lp, depth, path)?
        };
        self.sequence(vec![transfer, follow])
    }
    fn indirect(
        &mut self,
        block: EntityId,
        edges: &[EntityId],
        unresolved: bool,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<StructuredNode> {
        if edges.is_empty() {
            return self.node(
                if unresolved {
                    StructuredKind::PartialExit { block }
                } else {
                    StructuredKind::Exit { block }
                },
                vec![block],
            );
        }
        let mut arms = Vec::new();
        for id in edges {
            let target = self.ir.block(self.ir.edge(*id)?.to)?.source?.pc;
            let body = self.edge_path(*id, stop, lp, depth, path)?;
            arms.push(IndirectArm {
                target_pc: target,
                body,
            });
        }
        self.node(
            StructuredKind::Indirect {
                block,
                arms,
                unresolved,
            },
            vec![block],
        )
    }
    fn loop_region(
        &mut self,
        info: Loop,
        stop: Option<EntityId>,
        outer: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<StructuredNode> {
        let block = self.ir.block(info.header)?.clone();
        let mut visited = path.clone();
        visited.insert(info.header);
        let mut simple = None;
        if let Terminator::Branch {
            condition,
            then_edges,
            else_edge: Some(other),
            unresolved: false,
            invalid_targets,
            ..
        } = &block.terminator
        {
            if then_edges.len() == 1 && invalid_targets.is_empty() {
                let first = self.ir.edge(then_edges[0])?;
                let second = self.ir.edge(*other)?;
                if info.nodes.contains(&first.to) != info.nodes.contains(&second.to) {
                    simple = Some((
                        *condition,
                        info.nodes.contains(&first.to),
                        if info.nodes.contains(&first.to) {
                            first.id
                        } else {
                            second.id
                        },
                        if info.nodes.contains(&first.to) {
                            second.id
                        } else {
                            first.id
                        },
                    ));
                }
            }
        }
        let (header, condition, condition_nonzero, body, exit_edge) =
            if let Some((condition, nonzero, inside, outside)) = simple {
                let header = self.node(
                    StructuredKind::Block { block: info.header },
                    vec![info.header],
                )?;
                let body =
                    self.edge_path(inside, Some(info.header), Some(&info), depth + 1, &visited)?;
                (header, Some(condition), nonzero, body, Some(outside))
            } else {
                let header = self.sequence(Vec::new())?;
                let body =
                    self.core(info.header, Some(info.header), Some(&info), depth + 1, path)?;
                (header, None, true, body, None)
            };
        let loop_node = self.node(
            StructuredKind::While {
                header: Box::new(header),
                condition,
                condition_nonzero,
                body: Box::new(body),
                exit_edge,
            },
            vec![info.header],
        )?;
        if let Some(exit) = info.exit.filter(|e| Some(*e) != stop) {
            let follow = if outer.is_some_and(|lp| !lp.nodes.contains(&exit)) {
                self.node(StructuredKind::Break, vec![exit])?
            } else {
                self.build(exit, stop, outer, depth + 1, path)?
            };
            self.sequence(vec![loop_node, follow])
        } else {
            Some(loop_node)
        }
    }
    fn dispatch(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
    ) -> Option<StructuredNode> {
        let mut region = reachable(&self.ir, Some(start), stop);
        if let Some(lp) = lp {
            region.retain(|id| lp.nodes.contains(id) && *id != lp.header);
            region.insert(start);
        }
        let mut cases = Vec::new();
        for id in &region {
            let b = self.ir.block(*id)?.clone();
            let mut transitions = Vec::new();
            for edge in b.terminator.edges() {
                if !self.owner.pay(1) {
                    return None;
                }
                let target = self.ir.edge(edge)?.to;
                let id = self.owner.id();
                self.parents.insert(id, vec![edge]);
                transitions.push(DispatchTransition { id, edge, target });
            }
            cases.push(DispatchCase {
                block: *id,
                transitions,
            });
        }
        self.node(
            StructuredKind::Dispatcher {
                entry: start,
                cases,
            },
            region.into_iter().collect(),
        )
    }
    fn common_join(&self, a: EntityId, b: EntityId, lp: Option<&Loop>) -> Option<EntityId> {
        let da = distances(&self.ir, a);
        let db = distances(&self.ir, b);
        self.postdom[&a]
            .intersection(&self.postdom[&b])
            .copied()
            .filter(|id| id.0 != 0 && lp.is_none_or(|lp| lp.nodes.contains(id)))
            .filter_map(|id| Some(((*da.get(&id)?).max(*db.get(&id)?), id)))
            .min()
            .map(|(_, id)| id)
    }
}
fn distances(ir: &Ir, start: EntityId) -> BTreeMap<EntityId, usize> {
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
fn reaches_cycle_without_natural_header(
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
fn kind_sources(kind: &StructuredKind) -> Vec<EntityId> {
    match kind {
        StructuredKind::Sequence(v) => v.iter().flat_map(|n| kind_sources(&n.kind)).collect(),
        StructuredKind::Block { block }
        | StructuredKind::Exit { block }
        | StructuredKind::PartialExit { block }
        | StructuredKind::Indirect { block, .. } => vec![*block],
        StructuredKind::Transfer { edge } => vec![*edge],
        StructuredKind::If {
            condition,
            then_branch,
            else_branch,
        } => std::iter::once(*condition)
            .chain(kind_sources(&then_branch.kind))
            .chain(kind_sources(&else_branch.kind))
            .collect(),
        StructuredKind::While { header, body, .. } => kind_sources(&header.kind)
            .into_iter()
            .chain(kind_sources(&body.kind))
            .collect(),
        StructuredKind::Dispatcher { cases, .. } => cases.iter().map(|c| c.block).collect(),
        _ => Vec::new(),
    }
}

fn ast_depth(n: &StructuredNode) -> usize {
    1 + match &n.kind {
        StructuredKind::Sequence(v) => v.iter().map(ast_depth).max().unwrap_or(0),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => ast_depth(then_branch).max(ast_depth(else_branch)),
        StructuredKind::While { header, body, .. } => ast_depth(header).max(ast_depth(body)),
        StructuredKind::Indirect { arms, .. } => {
            arms.iter().map(|a| ast_depth(&a.body)).max().unwrap_or(0)
        }
        _ => 0,
    }
}

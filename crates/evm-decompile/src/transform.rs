//! Transactional pure folds and real selective node splitting.
use crate::{
    Completion, DiagnosticCode, EdgeRedirect, EntityId, ExitKind, FactScope, Ir, Progress,
    Severity, StageKind, StructuredKind, StructuredNode, Terminator, U256, ValueFact, ValueKind,
    artifact::Builder,
};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn simplify(owner: &mut Builder<impl FnMut(Progress)>) {
    if !owner.pay(ir_size(&owner.current().ir).saturating_mul(8)) {
        return;
    }
    let mut ir = owner.current().ir.clone();
    if ir.fact_scope != FactScope::CompleteLocalCfg {
        owner.commit(StageKind::Simplified, ir, ("preserve_open_facts","unresolved predecessor coverage keeps numeric facts conditional; no unconditional folding"), BTreeMap::new(), Vec::new(), Vec::new());
        return;
    }
    let facts: BTreeMap<_, _> = ir.values.iter().map(|v| (v.id, v.fact.clone())).collect();
    let value_indices: BTreeMap<_, _> = ir
        .values
        .iter()
        .enumerate()
        .map(|(i, v)| (v.id, i))
        .collect();
    for b in &mut ir.blocks {
        for op in &mut b.operations {
            if !owner.pay(1) {
                return;
            }
            if op.effects.is_pure()
                && evm_bytecode::word::arity(op.opcode).is_some()
                && let Some(word) = op
                    .outputs
                    .first()
                    .and_then(|v| facts.get(v))
                    .and_then(ValueFact::singleton)
            {
                op.replacement = Some(word);
                if let Some(output) = op.outputs.first() {
                    ir.values[value_indices[output]].kind = ValueKind::Constant(word);
                }
            }
        }
        if let Terminator::Branch {
            condition,
            target,
            then_edges,
            else_edge,
            unresolved,
            invalid_targets,
        } = &b.terminator
        {
            let choices = facts.get(condition);
            if choices.and_then(ValueFact::singleton) == Some(U256::ZERO) {
                b.terminator = else_edge
                    .map(|edge| Terminator::Fallthrough { edge })
                    .unwrap_or(Terminator::Exit {
                        kind: ExitKind::EndOfCode,
                        values: Vec::new(),
                    });
            } else if matches!(choices,Some(ValueFact::Constants(c)) if c.iter().all(|v|*v!=U256::ZERO))
            {
                b.terminator = Terminator::Jump {
                    target: *target,
                    edges: then_edges.clone(),
                    unresolved: *unresolved,
                    invalid_targets: invalid_targets.clone(),
                };
            }
        }
    }
    prune(&mut ir);
    owner.commit(StageKind::Simplified, ir, ("pure_folding","shared numeric facts fold pure operations and decided branches; effects remain ordered"), BTreeMap::new(), Vec::new(), Vec::new());
}
pub(crate) fn ir_size(ir: &Ir) -> usize {
    let mut units = ir
        .blocks
        .len()
        .saturating_add(ir.values.len())
        .saturating_add(ir.edges.len());
    for b in &ir.blocks {
        units = units.saturating_add(b.parameters.len());
        for o in &b.operations {
            units = units.saturating_add(1 + o.inputs.len() + o.outputs.len());
        }
    }
    for e in &ir.edges {
        units = units.saturating_add(e.arguments.len());
    }
    for v in &ir.values {
        if let ValueFact::Constants(c) = &v.fact {
            units = units.saturating_add(c.len());
        }
    }
    if let Some(s) = &ir.structured {
        units = units.saturating_add(node_size(&s.root));
    }
    units
}
fn node_size(n: &StructuredNode) -> usize {
    1 + match &n.kind {
        StructuredKind::Sequence(v) => v.iter().map(node_size).sum(),
        StructuredKind::If {
            then_branch,
            else_branch,
            ..
        } => node_size(then_branch) + node_size(else_branch),
        StructuredKind::While { header, body, .. } => node_size(header) + node_size(body),
        StructuredKind::Indirect { arms, .. } => arms.iter().map(|a| node_size(&a.body)).sum(),
        StructuredKind::Dispatcher { cases, .. } => {
            cases.iter().map(|c| 1 + c.transitions.len()).sum()
        }
        _ => 0,
    }
}

fn prune(ir: &mut Ir) {
    let live = reachable(ir, ir.entry, None);
    ir.blocks.retain(|b| live.contains(&b.id));
    let edges: BTreeSet<_> = ir
        .blocks
        .iter()
        .flat_map(|b| b.terminator.edges())
        .collect();
    ir.edges
        .retain(|e| edges.contains(&e.id) && live.contains(&e.to));
    let operations: BTreeSet<_> = ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .map(|o| o.id)
        .collect();
    let constants: BTreeSet<_> = ir
        .blocks
        .iter()
        .flat_map(|b| &b.operations)
        .flat_map(|o| o.outputs.iter().copied())
        .collect();
    ir.values.retain(|v| match v.kind {
        ValueKind::Operation { operation, .. }
        | ValueKind::Effect {
            operation: Some(operation),
        }
        | ValueKind::Memory {
            operation: Some(operation),
        } => operations.contains(&operation),
        ValueKind::Parameter { block, .. }
        | ValueKind::MemoryParameter { block }
        | ValueKind::EffectParameter { block } => live.contains(&block),
        _ => constants.contains(&v.id),
    });
}
pub(crate) fn reachable(
    ir: &Ir,
    start: Option<EntityId>,
    stop: Option<EntityId>,
) -> BTreeSet<EntityId> {
    let mut seen = BTreeSet::new();
    let mut todo: Vec<_> = start.into_iter().collect();
    while let Some(id) = todo.pop() {
        if Some(id) == stop || !seen.insert(id) {
            continue;
        }
        if let Some(b) = ir.block(id) {
            for e in b.terminator.edges() {
                if let Some(e) = ir.edge(e) {
                    todo.push(e.to);
                }
            }
        }
    }
    seen
}
pub(crate) fn dominators(
    ir: &Ir,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<BTreeMap<EntityId, BTreeSet<EntityId>>> {
    let cells = ir.blocks.len().saturating_mul(ir.blocks.len());
    if !owner.pay(cells.saturating_mul(16)) {
        return None;
    }
    let all: BTreeSet<_> = ir.blocks.iter().map(|b| b.id).collect();
    let mut dom: BTreeMap<_, _> = all
        .iter()
        .map(|id| {
            (
                *id,
                if Some(*id) == ir.entry {
                    BTreeSet::from([*id])
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
        for id in &all {
            if Some(*id) == ir.entry {
                continue;
            }
            let preds: Vec<_> = ir
                .edges
                .iter()
                .filter(|e| e.to == *id)
                .map(|e| e.from)
                .collect();
            let mut next = preds.first().map(|p| dom[p].clone()).unwrap_or_default();
            for p in preds.iter().skip(1) {
                next = next.intersection(&dom[p]).copied().collect();
            }
            next.insert(*id);
            if dom[id] != next {
                dom.insert(*id, next);
                changed = true;
            }
        }
        if !changed {
            return Some(dom);
        }
    }
}
// Remove already-natural backedges. Every remaining cyclic SCC needs splitting
// or dispatcher lowering; this detects nested irreducibility inside outer loops.
pub(crate) fn irreducible(
    ir: &Ir,
    dom: &BTreeMap<EntityId, BTreeSet<EntityId>>,
) -> Option<BTreeSet<EntityId>> {
    let adjacency: BTreeMap<_, Vec<_>> = ir
        .blocks
        .iter()
        .map(|b| {
            (
                b.id,
                ir.edges
                    .iter()
                    .filter(|e| e.from == b.id && !dom[&e.from].contains(&e.to))
                    .map(|e| e.to)
                    .collect(),
            )
        })
        .collect();
    let mut seen = BTreeSet::new();
    let mut order = Vec::new();
    for start in adjacency.keys() {
        if seen.contains(start) {
            continue;
        }
        let mut stack = vec![(*start, false)];
        while let Some((id, done)) = stack.pop() {
            if done {
                order.push(id);
                continue;
            }
            if !seen.insert(id) {
                continue;
            }
            stack.push((id, true));
            for n in &adjacency[&id] {
                if !seen.contains(n) {
                    stack.push((*n, false));
                }
            }
        }
    }
    seen.clear();
    while let Some(start) = order.pop() {
        if !seen.insert(start) {
            continue;
        }
        let mut component = BTreeSet::from([start]);
        let mut stack = vec![start];
        while let Some(id) = stack.pop() {
            for (p, targets) in &adjacency {
                if targets.contains(&id) && seen.insert(*p) {
                    component.insert(*p);
                    stack.push(*p);
                }
            }
        }
        if component.len() > 1 || adjacency[&start].contains(&start) {
            return Some(component);
        }
    }
    None
}
pub(crate) fn split_irreducible(owner: &mut Builder<impl FnMut(Progress)>) {
    loop {
        if owner.stopped {
            return;
        }
        if !owner.pay(ir_size(&owner.current().ir).saturating_mul(4)) {
            return;
        }
        let base = owner.current().ir.clone();
        let Some(dom) = dominators(&base, owner) else {
            return;
        };
        let Some(component) = irreducible(&base, &dom) else {
            if owner.current().kind != StageKind::ControlFlow {
                owner.commit(
                    StageKind::ControlFlow,
                    base,
                    (
                        "reducible_cfg",
                        "dominance confirms no irreducible cycle remains",
                    ),
                    BTreeMap::new(),
                    Vec::new(),
                    Vec::new(),
                );
            }
            return;
        };
        let mut entries: BTreeSet<_> = base
            .edges
            .iter()
            .filter(|e| component.contains(&e.to) && !component.contains(&e.from))
            .map(|e| e.to)
            .collect();
        if let Some(entry) = base.entry.filter(|e| component.contains(e)) {
            entries.insert(entry);
        }
        if entries.len() < 2 {
            owner.partial();
            owner.diagnostic(
                DiagnosticCode::Validation,
                Severity::Warning,
                None,
                None,
                "cyclic residual has no selectable multi-entry split; prior CFG retained without dispatcher fallback"
                    .to_owned(),
            );
            return;
        }
        let primary = base
            .entry
            .filter(|e| entries.contains(e))
            .unwrap_or(*entries.first().expect("multi-entry component"));
        let copies_per_entry = component.len() - 1;
        let total = copies_per_entry.saturating_mul(entries.len() - 1);
        if owner
            .artifact
            .statistics
            .cloned_blocks
            .saturating_add(total)
            > owner.limits().max_cloned_blocks
            || base.blocks.len().saturating_add(total) > owner.limits().max_blocks
        {
            owner.stop(
                Completion::Limited,
                DiagnosticCode::Limit,
                "selective node splitting exceeds clone/block limit; prior validated CFG retained",
            );
            return;
        }
        if !owner.pay(
            ir_size(&base)
                .saturating_mul(total.saturating_add(1))
                .saturating_mul(16),
        ) {
            return;
        }
        let selected_blocks: Vec<_> = base
            .blocks
            .iter()
            .filter(|b| component.contains(&b.id) && b.id != primary)
            .collect();
        let planned_operations = selected_blocks
            .iter()
            .map(|b| b.operations.len())
            .sum::<usize>()
            .saturating_mul(entries.len() - 1);
        let planned_values = selected_blocks
            .iter()
            .map(|b| {
                b.parameters.len().saturating_add(2).saturating_add(
                    b.operations
                        .iter()
                        .map(|o| {
                            o.outputs.len()
                                + usize::from(o.memory_output.is_some())
                                + usize::from(o.effect_output.is_some())
                        })
                        .sum::<usize>(),
                )
            })
            .sum::<usize>()
            .saturating_mul(entries.len() - 1);
        if base.values.len().saturating_add(planned_values) > owner.limits().max_values
            || base
                .blocks
                .iter()
                .map(|b| b.operations.len())
                .sum::<usize>()
                .saturating_add(planned_operations)
                > owner.limits().max_operations
        {
            owner.stop(Completion::Limited,DiagnosticCode::Limit,"node split temporary value/operation identities exceed the configured limit; prior validated stage retained");
            return;
        }
        let mut ir = base.clone();
        let mut parents = BTreeMap::new();
        let mut copies = Vec::new();
        let mut redirects = Vec::new();
        for entry in entries.iter().copied().filter(|e| *e != primary) {
            let selected: BTreeSet<_> = component
                .iter()
                .copied()
                .filter(|id| *id != primary)
                .collect();
            let map = copy_nodes(owner, &base, &mut ir, &selected, &mut parents, &mut copies);
            for e in ir.edges.iter_mut().filter(|e| {
                e.to == entry && !component.contains(&e.from) && base.edge(e.id).is_some()
            }) {
                let before = e.to;
                e.to = map[&entry];
                redirects.push(EdgeRedirect {
                    edge: e.id,
                    before,
                    after: e.to,
                });
            }
        }
        // A secondary entry may not reach every selected non-header copy
        // before returning to the primary header. Retain only actual bodies;
        // unreachable temporary copies have no published occurrence or clone.
        prune(&mut ir);
        let old_blocks: BTreeSet<_> = base.blocks.iter().map(|b| b.id).collect();
        let retained_copies = ir
            .blocks
            .iter()
            .filter(|b| !old_blocks.contains(&b.id))
            .count();
        if ir.values.len() > owner.limits().max_values
            || ir.blocks.iter().map(|b| b.operations.len()).sum::<usize>()
                > owner.limits().max_operations
        {
            owner.stop(Completion::Limited,DiagnosticCode::Limit,"split candidate exceeds retained value/operation limit; prior validated CFG retained");
            return;
        }
        if !owner.commit(StageKind::ControlFlow, ir, ("selective_node_split","copy non-header region and redirect secondary entries; all copied definitions and edge assignments receive fresh identities"), parents, copies, redirects){return;}
        owner.artifact.statistics.cloned_blocks += retained_copies;
    }
}
fn copy_nodes(
    owner: &mut Builder<impl FnMut(Progress)>,
    base: &Ir,
    ir: &mut Ir,
    selected: &BTreeSet<EntityId>,
    parents: &mut BTreeMap<EntityId, Vec<EntityId>>,
    copies: &mut Vec<(EntityId, EntityId)>,
) -> BTreeMap<EntityId, EntityId> {
    let mut map = BTreeMap::new();
    for b in base.blocks.iter().filter(|b| selected.contains(&b.id)) {
        for id in std::iter::once(b.id)
            .chain(b.parameters.iter().copied())
            .chain(std::iter::once(b.memory_parameter))
            .chain(std::iter::once(b.effect_parameter))
            .chain(b.operations.iter().flat_map(|o| {
                std::iter::once(o.id)
                    .chain(o.outputs.iter().copied())
                    .chain(o.memory_output)
                    .chain(o.effect_output)
            }))
        {
            map.entry(id).or_insert_with(|| owner.id());
        }
    }
    for e in base.edges.iter().filter(|e| selected.contains(&e.from)) {
        map.insert(e.id, owner.id());
    }
    for (old, new) in &map {
        parents.insert(*new, vec![*old]);
        copies.push((*old, *new));
    }
    let remap = |id: EntityId| map.get(&id).copied().unwrap_or(id);
    for v in base.values.iter().filter(|v| map.contains_key(&v.id)) {
        let mut v = v.clone();
        v.id = remap(v.id);
        match &mut v.kind {
            ValueKind::Operation { operation, .. } => *operation = remap(*operation),
            ValueKind::Memory {
                operation: Some(operation),
            }
            | ValueKind::Effect {
                operation: Some(operation),
            } => *operation = remap(*operation),
            ValueKind::Parameter { block, .. }
            | ValueKind::MemoryParameter { block }
            | ValueKind::EffectParameter { block } => *block = remap(*block),
            _ => {}
        }
        ir.values.push(v);
    }
    for b in base.blocks.iter().filter(|b| selected.contains(&b.id)) {
        let mut b = b.clone();
        b.id = remap(b.id);
        b.memory_parameter = remap(b.memory_parameter);
        b.effect_parameter = remap(b.effect_parameter);
        for v in &mut b.parameters {
            *v = remap(*v);
        }
        for o in &mut b.operations {
            o.id = remap(o.id);
            for v in o.inputs.iter_mut().chain(&mut o.outputs) {
                *v = remap(*v);
            }
            o.memory_input = o.memory_input.map(remap);
            o.memory_output = o.memory_output.map(remap);
            o.effect_input = o.effect_input.map(remap);
            o.effect_output = o.effect_output.map(remap);
        }
        remap_terminator(&mut b.terminator, &remap);
        ir.blocks.push(b);
    }
    for e in base.edges.iter().filter(|e| selected.contains(&e.from)) {
        let mut e = e.clone();
        e.id = remap(e.id);
        e.from = remap(e.from);
        e.to = remap(e.to);
        e.memory = remap(e.memory);
        e.effect = remap(e.effect);
        for v in &mut e.arguments {
            *v = remap(*v);
        }
        ir.edges.push(e);
    }
    map
}
fn remap_terminator(term: &mut Terminator, map: &impl Fn(EntityId) -> EntityId) {
    match term {
        Terminator::Fallthrough { edge } => *edge = map(*edge),
        Terminator::Jump { target, edges, .. } => {
            *target = map(*target);
            for e in edges {
                *e = map(*e);
            }
        }
        Terminator::Branch {
            condition,
            target,
            then_edges,
            else_edge,
            ..
        } => {
            *condition = map(*condition);
            *target = map(*target);
            for e in then_edges {
                *e = map(*e);
            }
            *else_edge = else_edge.map(map);
        }
        Terminator::Exit { values, .. } => {
            for v in values {
                *v = map(*v);
            }
        }
        _ => {}
    }
}

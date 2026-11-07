//! Bounded candidate selection; source admission lives independently in model.
use crate::{
    EntityId, Ir, LiftedProgram, SharedBinding, SharedInvocation, SharedProcedure,
    SharedProcedureError, SharedRegion, SharedSourceIndex, StructuredKind,
};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};

pub(super) struct Plan {
    pub procedures: Vec<SharedProcedure>,
    pub invocations: Vec<SharedInvocation>,
}
fn fingerprint(
    ir: &Ir,
    region: &SharedRegion,
    blocks: &BTreeMap<EntityId, &crate::Block>,
    nodes: &BTreeMap<EntityId, &crate::StructuredNode>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<u64> {
    if !pay(region.entities.len().saturating_mul(2)) {
        return None;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for id in &region.nodes {
        let tag = match &nodes[id].kind {
            StructuredKind::Sequence(v) => {
                v.len().hash(&mut h);
                0
            }
            StructuredKind::Block { .. } => 1,
            StructuredKind::Transfer { .. } => 2,
            StructuredKind::If { .. } => 3,
            StructuredKind::While {
                condition,
                condition_nonzero,
                ..
            } => {
                condition.is_some().hash(&mut h);
                condition_nonzero.hash(&mut h);
                4
            }
            StructuredKind::Break => 5,
            StructuredKind::Continue => 6,
            StructuredKind::Exit { .. } => 7,
            _ => 8,
        };
        tag.hash(&mut h);
    }
    for id in &region.blocks {
        for op in &blocks[id].operations {
            op.opcode.hash(&mut h);
        }
    }
    let _ = ir;
    Some(h.finish())
}
fn visible_cost(
    root: EntityId,
    nodes: &BTreeMap<EntityId, &crate::StructuredNode>,
    render: &crate::LiftedRenderIndex<'_>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<(usize, usize)> {
    let mut bytes = 0_usize;
    let mut primitives = 0_usize;
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if !pay(1) {
            return None;
        }
        let n = nodes[&id];
        match &n.kind {
            StructuredKind::Sequence(children) => {
                let mut selected = Vec::new();
                let mut pos = 0;
                while let Some(n) = children.get(pos) {
                    if !pay(1) {
                        return None;
                    }
                    if let Some(end) = render.sequence_end(id, pos) {
                        pos = end;
                        continue;
                    }
                    selected.push(n.id);
                    pos += 1;
                }
                todo.extend(selected.into_iter().rev());
            }
            StructuredKind::Block { block } => {
                let h = render.block(*block)?;
                let mut pos = 0;
                let compact = render.construction(*block);
                while let Some(statement) = h.statements.get(pos) {
                    if let Some(c) = compact
                        && pos == c.start
                    {
                        let text = render.render_revert(*block, &mut *pay).ok()??;
                        bytes = bytes.saturating_add(text.len() + 5);
                        primitives += 1;
                        pos = c.end;
                        continue;
                    }
                    let text = render
                        .render_statement_with_dependencies(statement, false, &mut *pay)
                        .ok()?;
                    bytes = bytes.saturating_add(text.len() + 17);
                    primitives += 1;
                    pos += 1;
                }
            }
            StructuredKind::Transfer { edge } => {
                if let Some(text) = render
                    .render_transfer(render.transfer(*edge)?, &mut *pay)
                    .ok()?
                {
                    bytes = bytes.saturating_add(text.len() + 5);
                }
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                if let Some(g) = render.guard(id) {
                    let text = render.render_guard(render.control(id)?, &mut *pay).ok()??;
                    bytes = bytes.saturating_add(text.len() + 5);
                    todo.push(g.pass_node);
                } else {
                    let text = render
                        .render_expression(render.control(id)?.condition, &mut *pay)
                        .ok()?;
                    bytes = bytes.saturating_add(text.len() + 28);
                    todo.extend([else_branch.id, then_branch.id]);
                }
            }
            StructuredKind::While { header, body, .. } => {
                bytes = bytes.saturating_add(64);
                todo.extend([body.id, header.id]);
            }
            StructuredKind::Exit { .. } => {}
            StructuredKind::Break | StructuredKind::Continue => bytes = bytes.saturating_add(24),
            _ => return None,
        }
    }
    Some((bytes, primitives))
}
// Profitability uses the default concise view. State annotations remain a
// selectable diagnostic projection and do not turn real code into free work.
fn call_cost(interface: &crate::SharedInterface) -> usize {
    24_usize.saturating_add((interface.inputs.len() + interface.outputs.len()).saturating_mul(9))
}
pub(super) fn discover(
    ir: &Ir,
    program: &LiftedProgram,
    next: &mut u64,
    pay: &mut impl FnMut(usize) -> bool,
) -> Option<Plan> {
    let index = match SharedSourceIndex::for_program(ir, Some(program), pay) {
        Ok(i) => i,
        Err(SharedProcedureError::Limit) => return None,
        Err(_) => {
            return Some(Plan {
                procedures: Vec::new(),
                invocations: Vec::new(),
            });
        }
    };
    if !pay(ir
        .blocks
        .len()
        .saturating_add(crate::transform::ir_size(ir)))
    {
        return None;
    }
    let blocks: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
    let mut nodes = BTreeMap::new();
    let mut pending = vec![&ir.structured.as_ref()?.root];
    while let Some(n) = pending.pop() {
        nodes.insert(n.id, n);
        super::children(n, &mut pending);
    }
    let render = program.render_index_with_budget(ir, &mut *pay);
    if render.validate_guard_sources().is_err() {
        return None;
    }
    // A source occurrence already hidden by a certified guard/suffix cannot
    // contribute a visible call. Keep its witness in a larger visible region,
    // but never emit a standalone unused definition for that hidden subtree.
    let mut visible = BTreeSet::new();
    let mut pending = vec![ir.structured.as_ref()?.root.id];
    while let Some(id) = pending.pop() {
        if !pay(1) {
            return None;
        }
        visible.insert(id);
        match &nodes[&id].kind {
            StructuredKind::Sequence(children) => {
                let mut pos = 0;
                while let Some(n) = children.get(pos) {
                    if !pay(1) {
                        return None;
                    }
                    if let Some(end) = render.sequence_end(id, pos) {
                        pos = end;
                        continue;
                    }
                    pending.push(n.id);
                    pos += 1;
                }
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                if let Some(g) = render.guard(id) {
                    pending.push(g.pass_node);
                } else {
                    pending.extend([then_branch.id, else_branch.id]);
                }
            }
            StructuredKind::While { header, body, .. } => pending.extend([header.id, body.id]),
            _ => {}
        }
    }
    let mut costs = BTreeMap::new();
    let mut groups = BTreeMap::<u64, Vec<(usize, EntityId)>>::new();
    for root in index.node_ids() {
        if !visible.contains(&root) {
            continue;
        }
        let r = match index.region(root, pay) {
            Ok(r) => r,
            Err(SharedProcedureError::Limit) => return None,
            Err(_) => continue,
        };
        // A lone terminal or trivial block is cheaper in place. The heuristic
        // never certifies equivalence; full source checking follows grouping.
        let Some((bytes, retained)) = visible_cost(root, &nodes, &render, pay) else {
            continue;
        };
        if !r.terminal_closed
            || retained < 3
            || r.interface.inputs.len() + r.interface.outputs.len() > 64
        {
            continue;
        }
        costs.insert(root, bytes);
        let hash = fingerprint(ir, &r, &blocks, &nodes, pay)?;
        groups.entry(hash).or_default().push((bytes, root));
    }
    let mut groups: Vec<_> = groups.into_values().filter(|v| v.len() > 1).collect();
    if !pay(groups.len().saturating_mul(16)) {
        return None;
    }
    groups.sort_by_key(|v| std::cmp::Reverse(v.iter().map(|v| v.0).max().unwrap_or(0)));
    let mut plan = Plan {
        procedures: Vec::new(),
        invocations: Vec::new(),
    };
    let mut occupied = BTreeSet::new();
    let mut weight = program.record_count();
    for mut candidates in groups {
        candidates.sort_by_key(|(score, id)| (std::cmp::Reverse(*score), *id));
        let mut cursor = 0;
        while let Some((_, root)) = candidates.get(cursor).copied() {
            cursor += 1;
            if occupied.contains(&root) {
                continue;
            }
            let canonical = match index.region(root, pay) {
                Ok(r) => r,
                Err(SharedProcedureError::Limit) => return None,
                Err(_) => continue,
            };
            if canonical.nodes.iter().any(|id| occupied.contains(id)) {
                continue;
            }
            // Keep only sparse substitutions per site. Retaining full region
            // entity domains for repeated same-source AST copies would multiply
            // transient memory by body size even though wire maps are sparse.
            let mut matches = Vec::<(EntityId, Vec<SharedBinding>)>::new();
            let mut pending_weight = weight
                .saturating_add(canonical.interface.record_count())
                .saturating_add(8);
            for (_, actual) in &candidates[cursor..] {
                if !pay(1) {
                    return None;
                }
                if occupied.contains(actual) {
                    continue;
                }
                let actual = match index.region(*actual, pay) {
                    Ok(r) => r,
                    Err(SharedProcedureError::Limit) => return None,
                    Err(_) => continue,
                };
                if canonical.nodes.iter().any(|id| actual.nodes.contains(id))
                    || actual.nodes.iter().any(|id| occupied.contains(id))
                {
                    continue;
                }
                let bindings = match index.correspondence(&canonical, &actual, pay) {
                    Ok(v) => v,
                    Err(SharedProcedureError::Limit) => return None,
                    Err(_) => continue,
                };
                match index.verify_correspondence(&canonical, &actual, &bindings, pay) {
                    Ok(()) => {}
                    Err(SharedProcedureError::Limit) => return None,
                    Err(_) => continue,
                }
                pending_weight = pending_weight
                    .saturating_add(bindings.len().saturating_mul(2))
                    .saturating_add(6);
                if pending_weight > crate::MAX_LIFTED_RECORDS {
                    return None;
                }
                matches.push((actual.root, bindings));
            }
            if matches.is_empty() {
                continue;
            }
            // Existing capsules/guards count as their actual high rows. Long
            // primitives outweigh call syntax; source instruction counts do
            // not make an already compact require appear profitable.
            let source_cost = matches
                .iter()
                .fold(costs[&root], |n, (r, _)| n.saturating_add(costs[r]));
            let calls_cost = call_cost(&canonical.interface).saturating_mul(matches.len() + 1);
            let definition_cost = costs[&root]
                .saturating_add(call_cost(&canonical.interface))
                .saturating_add(24 + canonical.interface.outputs.len() * 9);
            if source_cost <= definition_cost.saturating_add(calls_cost) {
                continue;
            }
            let pid = EntityId(*next);
            *next += 1;
            let procedure = SharedProcedure {
                id: pid,
                body: root,
                interface: canonical.interface.clone(),
            };
            weight = weight
                .saturating_add(procedure.interface.record_count())
                .saturating_add(2);
            let mut calls = Vec::new();
            let iid = EntityId(*next);
            *next += 1;
            weight = weight.saturating_add(6);
            calls.push(index.invocation(iid, pid, root, Vec::new()).ok()?);
            occupied.extend(canonical.nodes);
            for (root, bindings) in matches {
                let region = match index.region(root, pay) {
                    Ok(r) => r,
                    Err(_) => return None,
                };
                if occupied.contains(&region.root)
                    || region.nodes.iter().any(|id| occupied.contains(id))
                {
                    continue;
                }
                let iid = EntityId(*next);
                *next += 1;
                weight = weight
                    .saturating_add(bindings.len().saturating_mul(2))
                    .saturating_add(6);
                calls.push(index.invocation(iid, pid, region.root, bindings).ok()?);
                occupied.extend(region.nodes);
            }
            if calls.len() < 2 {
                return None;
            }
            if weight > crate::MAX_LIFTED_RECORDS
                || !pay(calls
                    .len()
                    .saturating_add(procedure.interface.record_count()))
            {
                return None;
            }
            plan.procedures.push(procedure);
            plan.invocations.extend(calls);
        }
    }
    Some(plan)
}

#[cfg(test)]
mod tests;

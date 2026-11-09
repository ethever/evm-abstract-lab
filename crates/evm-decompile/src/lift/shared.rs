//! Bounded candidate selection; source admission lives independently in model.
use crate::{
    EntityId, Ir, LiftedIfPlan, LiftedProgram, SharedBinding, SharedInvocation, SharedProcedure,
    SharedProcedureError, SharedSourceIndex, StructuredKind,
};
use std::collections::{BTreeMap, BTreeSet};

mod arithmetic;
mod fingerprint;

#[derive(Debug)]
pub(super) struct Plan {
    pub procedures: Vec<SharedProcedure>,
    pub invocations: Vec<SharedInvocation>,
}
#[derive(Debug, thiserror::Error)]
pub(super) enum DiscoveryError {
    #[error("shared discovery payer refused work")]
    Payment,
    #[error("{0}")]
    Capacity(SharedProcedureError),
    #[error("source region {0:?} is unsupported by optional sharing")]
    Unsupported(EntityId),
    #[error("invalid shared source witness: {0}")]
    Invalid(SharedProcedureError),
    #[error("invalid shared formatting witness: {0}")]
    Formatting(crate::LiftedRenderError),
    #[error("shared publication failed semantic validation")]
    Publication,
}
impl DiscoveryError {
    pub(super) fn source(error: SharedProcedureError) -> Self {
        match error {
            SharedProcedureError::Limit => Self::Payment,
            error @ SharedProcedureError::Capacity { .. } => Self::Capacity(error),
            error => Self::Invalid(error),
        }
    }
    pub fn optional(&self) -> bool {
        matches!(
            self,
            Self::Capacity(_)
                | Self::Unsupported(_)
                | Self::Formatting(crate::LiftedRenderError::Limit)
        )
    }
}
fn rendered(
    result: Result<String, crate::LiftedRenderError>,
) -> Result<Option<String>, DiscoveryError> {
    match result {
        Ok(text) => Ok(Some(text)),
        Err(crate::LiftedRenderError::Limit) => Ok(None),
        Err(error) => Err(DiscoveryError::Formatting(error)),
    }
}
fn selected_children(
    render: &crate::LiftedRenderIndex<'_>,
    sequence: EntityId,
    children: &[crate::StructuredNode],
    range: std::ops::Range<usize>,
    boundary: EntityId,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Vec<EntityId>, DiscoveryError> {
    let mut selected = Vec::new();
    let mut pos = range.start;
    while pos < range.end {
        if !pay(1) {
            return Err(DiscoveryError::Payment);
        }
        if pos > range.start
            && let Some(end) = render.selected_sequence_end(sequence, pos, boundary)
        {
            pos = end;
            continue;
        }
        selected.push(
            children
                .get(pos)
                .ok_or(DiscoveryError::Invalid(SharedProcedureError::Missing(
                    sequence,
                )))?
                .id,
        );
        pos += 1;
    }
    Ok(selected)
}
fn visible_cost(
    root: EntityId,
    nodes: &BTreeMap<EntityId, &crate::StructuredNode>,
    render: &crate::LiftedRenderIndex<'_>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Option<(usize, usize)>, DiscoveryError> {
    let mut bytes = 0_usize;
    let mut primitives = 0_usize;
    let mut todo = vec![root];
    while let Some(id) = todo.pop() {
        if !pay(1) {
            return Err(DiscoveryError::Payment);
        }
        let n = nodes[&id];
        match &n.kind {
            StructuredKind::Sequence(children) => {
                let selected =
                    selected_children(render, id, children, 0..children.len(), root, pay)?;
                todo.extend(selected.into_iter().rev());
            }
            StructuredKind::Block { block } => {
                let h = render.block(*block).ok_or(DiscoveryError::Invalid(
                    SharedProcedureError::Missing(*block),
                ))?;
                for statement in &h.statements {
                    let Some(text) = rendered(
                        render.render_statement_with_dependencies(statement, false, &mut *pay),
                    )?
                    else {
                        return Ok(None);
                    };
                    bytes = bytes.saturating_add(text.len() + 17);
                    primitives += 1;
                }
            }
            StructuredKind::Transfer { edge } => {
                let transfer = render.transfer(*edge).ok_or(DiscoveryError::Invalid(
                    SharedProcedureError::Missing(*edge),
                ))?;
                if let Some(text) = render
                    .render_transfer(transfer, &mut *pay)
                    .map_err(DiscoveryError::Formatting)?
                {
                    bytes = bytes.saturating_add(text.len() + 5);
                }
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => match render.if_plan(id, root) {
                LiftedIfPlan::Require(plan) => {
                    let Some(text) = rendered(render.render_require(plan, &mut *pay))? else {
                        return Ok(None);
                    };
                    bytes = bytes.saturating_add(text.len() + 5);
                    todo.push(plan.success_node());
                }
                LiftedIfPlan::FailureFirst(plan) => {
                    let Some(text) =
                        rendered(render.render_failure_first_condition(plan, &mut *pay))?
                    else {
                        return Ok(None);
                    };
                    bytes = bytes.saturating_add(text.len() + 12);
                    todo.push(plan.success_node());
                    if let Some((sequence, range)) = render.failure_first_suffix_range(plan) {
                        let StructuredKind::Sequence(children) = &nodes[&sequence].kind else {
                            return Err(DiscoveryError::Invalid(SharedProcedureError::Missing(
                                sequence,
                            )));
                        };
                        todo.extend(
                            selected_children(render, sequence, children, range, root, pay)?
                                .into_iter()
                                .rev(),
                        );
                    }
                    todo.push(plan.failure_node());
                }
                LiftedIfPlan::Original => {
                    let control = render
                        .control(id)
                        .ok_or(DiscoveryError::Invalid(SharedProcedureError::Missing(id)))?;
                    let Some(text) =
                        rendered(render.render_condition(control.condition, false, &mut *pay))?
                    else {
                        return Ok(None);
                    };
                    bytes = bytes.saturating_add(text.len() + 28);
                    todo.extend([else_branch.id, then_branch.id]);
                }
            },
            StructuredKind::While { header, body, .. } => {
                bytes = bytes.saturating_add(64);
                todo.extend([body.id, header.id]);
            }
            StructuredKind::Exit { .. } => {}
            StructuredKind::Break | StructuredKind::Continue => bytes = bytes.saturating_add(24),
            _ => return Ok(None),
        }
    }
    Ok(Some((bytes, primitives)))
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
    max_event_slots: usize,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<Plan, DiscoveryError> {
    let index =
        SharedSourceIndex::for_program_with_event_capacity(ir, Some(program), pay, max_event_slots)
            .map_err(|error| match error {
                SharedProcedureError::Region(root) => DiscoveryError::Unsupported(root),
                error => DiscoveryError::source(error),
            })?;
    if !pay(ir
        .blocks
        .len()
        .saturating_add(crate::transform::ir_size(ir)))
    {
        return Err(DiscoveryError::Payment);
    }
    let mut nodes = BTreeMap::new();
    let ast =
        ir.structured
            .as_ref()
            .ok_or(DiscoveryError::Invalid(SharedProcedureError::Missing(
                EntityId(0),
            )))?;
    let mut pending = vec![&ast.root];
    while let Some(n) = pending.pop() {
        nodes.insert(n.id, n);
        super::children(n, &mut pending);
    }
    let render = program.render_index_with_budget(ir, &mut *pay);
    render
        .validate_guard_sources()
        .map_err(DiscoveryError::Formatting)?;
    let fingerprints = fingerprint::Fingerprints::new(ir, program, pay)?;
    // A source occurrence already hidden by a certified guard/suffix cannot
    // contribute a visible call. Keep its witness in a larger visible region,
    // but never emit a standalone unused definition for that hidden subtree.
    let mut visible = BTreeSet::new();
    let boundary = ast.root.id;
    let mut pending = vec![boundary];
    while let Some(id) = pending.pop() {
        if !pay(1) {
            return Err(DiscoveryError::Payment);
        }
        visible.insert(id);
        match &nodes[&id].kind {
            StructuredKind::Sequence(children) => {
                pending.extend(selected_children(
                    &render,
                    id,
                    children,
                    0..children.len(),
                    boundary,
                    pay,
                )?);
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => match render.if_plan(id, boundary) {
                LiftedIfPlan::Require(plan) => pending.push(plan.success_node()),
                LiftedIfPlan::FailureFirst(plan) => {
                    pending.extend([plan.success_node(), plan.failure_node()]);
                    if let Some((sequence, range)) = render.failure_first_suffix_range(plan) {
                        let StructuredKind::Sequence(children) = &nodes[&sequence].kind else {
                            return Err(DiscoveryError::Invalid(SharedProcedureError::Missing(
                                sequence,
                            )));
                        };
                        pending.extend(selected_children(
                            &render, sequence, children, range, boundary, pay,
                        )?);
                    }
                }
                LiftedIfPlan::Original => pending.extend([then_branch.id, else_branch.id]),
            },
            StructuredKind::While { header, body, .. } => pending.extend([header.id, body.id]),
            StructuredKind::Indirect { arms, .. } => {
                pending.extend(arms.iter().map(|arm| arm.body.id));
            }
            _ => {}
        }
    }
    let mut costs = BTreeMap::new();
    let mut input_counts = BTreeMap::new();
    let mut sizes = BTreeMap::new();
    let mut groups = BTreeMap::<u64, Vec<(usize, EntityId)>>::new();
    for root in index.node_ids() {
        if !visible.contains(&root) {
            continue;
        }
        let r = match index.region(root, pay) {
            Ok(r) => r,
            Err(SharedProcedureError::Region(_)) => continue,
            Err(error) => return Err(DiscoveryError::source(error)),
        };
        // A lone terminal or trivial block is cheaper in place. The heuristic
        // never certifies equivalence; full source checking follows grouping.
        let Some((bytes, retained)) = visible_cost(root, &nodes, &render, pay)? else {
            continue;
        };
        let arithmetic_loop = bytes >= 128
            && r.nodes
                .iter()
                .any(|id| matches!(nodes[id].kind, StructuredKind::While { .. }));
        if !r.terminal_closed
            || (retained < 3 && !arithmetic_loop)
            || r.interface.inputs.len() + r.interface.outputs.len() > 64
        {
            continue;
        }
        costs.insert(root, bytes);
        input_counts.insert(root, r.interface.inputs.len());
        sizes.insert(root, r.nodes.len());
        let hash = fingerprints.region(&r, &nodes, pay)?;
        groups.entry(hash).or_default().push((bytes, root));
    }
    let mut groups: Vec<_> = groups.into_values().filter(|v| v.len() > 1).collect();
    if !pay(groups.len().saturating_mul(16)) {
        return Err(DiscoveryError::Payment);
    }
    groups.sort_by_key(|group| {
        std::cmp::Reverse(
            group
                .iter()
                .map(|(bytes, root)| (*bytes, sizes[root]))
                .max()
                .unwrap_or((0, 0)),
        )
    });
    let mut plan = Plan {
        procedures: Vec::new(),
        invocations: Vec::new(),
    };
    let mut occupied = BTreeSet::new();
    let mut hidden = BTreeSet::new();
    let mut weight = program.record_count();
    for mut candidates in groups {
        candidates.sort_by_key(|(score, id)| {
            (
                std::cmp::Reverse(input_counts[id]),
                std::cmp::Reverse(*score),
                *id,
            )
        });
        let mut cursor = 0;
        while let Some((_, root)) = candidates.get(cursor).copied() {
            cursor += 1;
            if occupied.contains(&root) || hidden.contains(&root) {
                continue;
            }
            let canonical = match index.region(root, pay) {
                Ok(r) => r,
                Err(SharedProcedureError::Region(_)) => continue,
                Err(error) => return Err(DiscoveryError::source(error)),
            };
            // Keep only sparse substitutions per site. Retaining full region
            // entity domains for repeated same-source AST copies would multiply
            // transient memory by body size even though wire maps are sparse.
            let mut matches = Vec::<(EntityId, Vec<SharedBinding>)>::new();
            let mut pending_weight = weight
                .saturating_add(canonical.interface.record_count())
                .saturating_add(9);
            for (_, actual) in &candidates {
                if !pay(1) {
                    return Err(DiscoveryError::Payment);
                }
                if *actual == root || occupied.contains(actual) || hidden.contains(actual) {
                    continue;
                }
                let actual = match index.region(*actual, pay) {
                    Ok(r) => r,
                    Err(SharedProcedureError::Region(_)) => continue,
                    Err(error) => return Err(DiscoveryError::source(error)),
                };
                if canonical.nodes.iter().any(|id| actual.nodes.contains(id)) {
                    continue;
                }
                let bindings = match index.correspondence(&canonical, &actual, pay) {
                    Ok(v) => v,
                    Err(
                        SharedProcedureError::Region(_)
                        | SharedProcedureError::Correspondence(_)
                        | SharedProcedureError::Interface(_),
                    ) => continue,
                    Err(error) => return Err(DiscoveryError::source(error)),
                };
                match index.verify_correspondence(&canonical, &actual, &bindings, pay) {
                    Ok(()) => {}
                    Err(
                        SharedProcedureError::Region(_)
                        | SharedProcedureError::Correspondence(_)
                        | SharedProcedureError::Interface(_),
                    ) => continue,
                    Err(error) => return Err(DiscoveryError::source(error)),
                }
                pending_weight = pending_weight
                    .saturating_add(bindings.len().saturating_mul(2))
                    .saturating_add(6);
                if pending_weight > crate::MAX_LIFTED_RECORDS {
                    return Err(DiscoveryError::source(SharedProcedureError::Capacity {
                        resource: crate::SharedCapacity::ProgramRecords,
                        required: pending_weight,
                        limit: crate::MAX_LIFTED_RECORDS,
                    }));
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
                kind: canonical.kind,
                interface: canonical.interface.clone(),
            };
            weight = weight
                .saturating_add(procedure.interface.record_count())
                .saturating_add(3);
            let mut calls = Vec::new();
            let iid = EntityId(*next);
            *next += 1;
            weight = weight.saturating_add(6);
            calls.push(
                index
                    .invocation(iid, pid, root, Vec::new())
                    .map_err(DiscoveryError::source)?,
            );
            // The canonical body remains visible when rendering a definition.
            // Its strict descendants may independently call another helper.
            occupied.insert(canonical.root);
            for (root, bindings) in matches {
                let region = match index.region(root, pay) {
                    Ok(r) => r,
                    Err(error) => return Err(DiscoveryError::source(error)),
                };
                if occupied.contains(&region.root) || hidden.contains(&region.root) {
                    continue;
                }
                let iid = EntityId(*next);
                *next += 1;
                weight = weight
                    .saturating_add(bindings.len().saturating_mul(2))
                    .saturating_add(6);
                calls.push(
                    index
                        .invocation(iid, pid, region.root, bindings)
                        .map_err(DiscoveryError::source)?,
                );
                occupied.insert(region.root);
                hidden.extend(region.nodes);
            }
            if calls.len() < 2 {
                return Err(DiscoveryError::Invalid(SharedProcedureError::Site(root)));
            }
            if weight > crate::MAX_LIFTED_RECORDS {
                return Err(DiscoveryError::source(SharedProcedureError::Capacity {
                    resource: crate::SharedCapacity::ProgramRecords,
                    required: weight,
                    limit: crate::MAX_LIFTED_RECORDS,
                }));
            }
            if !pay(calls
                .len()
                .saturating_add(procedure.interface.record_count()))
            {
                return Err(DiscoveryError::Payment);
            }
            plan.procedures.push(procedure);
            plan.invocations.extend(calls);
        }
    }
    arithmetic::Source {
        program,
        index: &index,
        nodes: &nodes,
        visible: &visible,
        hidden: &hidden,
        render: &render,
        fingerprints: &fingerprints,
    }
    .discover(&mut plan, next, pay)?;
    Ok(plan)
}

#[cfg(test)]
mod tests;

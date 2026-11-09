//! Outline only certified pure DAGs at their existing expression occurrences.
use super::{DiscoveryError, Plan, fingerprint::Fingerprints, rendered};
use crate::{
    EntityId, LiftedProgram, LiftedRenderIndex, SharedBinding, SharedCapacity, SharedProcedure,
    SharedProcedureError as Error, SharedSourceIndex, StructuredKind as K,
};
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Source<'a, 's> {
    pub program: &'a LiftedProgram,
    pub index: &'s SharedSourceIndex<'a>,
    pub nodes: &'s BTreeMap<EntityId, &'a crate::StructuredNode>,
    pub visible: &'s BTreeSet<EntityId>,
    pub hidden: &'s BTreeSet<EntityId>,
    pub render: &'s LiftedRenderIndex<'a>,
    pub fingerprints: &'s Fingerprints<'a>,
}

#[derive(Clone, Copy)]
struct Candidate {
    root: EntityId,
    bytes: usize,
    direct: bool,
    parameters: Option<usize>,
}

fn rejected(error: &Error) -> bool {
    matches!(
        error,
        Error::Region(_) | Error::Correspondence(_) | Error::Interface(_)
    )
}

fn candidate_region(
    result: Result<crate::SharedRegion, Error>,
) -> Result<Option<crate::SharedRegion>, DiscoveryError> {
    match result {
        Ok(region) => Ok(Some(region)),
        Err(Error::Region(_)) => Ok(None),
        Err(error) => Err(DiscoveryError::source(error)),
    }
}

fn call_cost(inputs: usize) -> usize {
    16_usize.saturating_add(inputs.saturating_mul(8))
}

fn block_roots(
    high: &crate::LiftedBlock,
    render: &LiftedRenderIndex<'_>,
    todo: &mut Vec<EntityId>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<(), DiscoveryError> {
    let construction = render.construction(high.block);
    for (position, statement) in high.statements.iter().enumerate() {
        if !pay(statement.inputs.len().saturating_add(1)) {
            return Err(DiscoveryError::Payment);
        }
        if construction.is_none_or(|proof| !(proof.start..proof.end).contains(&position)) {
            todo.extend(statement.inputs.iter().copied());
        }
    }
    if !pay(high.exit_values.len().saturating_add(2)) {
        return Err(DiscoveryError::Payment);
    }
    todo.extend(high.condition);
    todo.extend(high.target);
    if construction.is_none() {
        todo.extend(high.exit_values.iter().copied());
    }
    Ok(())
}

impl Source<'_, '_> {
    fn reachable(
        &self,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(BTreeSet<EntityId>, BTreeSet<EntityId>), DiscoveryError> {
        let blocks: BTreeMap<_, _> = self.program.blocks.iter().map(|b| (b.block, b)).collect();
        let transfers: BTreeMap<_, _> =
            self.program.transfers.iter().map(|t| (t.edge, t)).collect();
        let controls: BTreeMap<_, _> = self.program.controls.iter().map(|c| (c.node, c)).collect();
        let expressions: BTreeMap<_, _> =
            self.program.expressions.iter().map(|e| (e.id, e)).collect();
        let mut todo = Vec::new();
        for id in self.visible {
            if !pay(1) {
                return Err(DiscoveryError::Payment);
            }
            if self.hidden.contains(id) {
                continue;
            }
            match self.nodes[id].kind {
                K::Block { block } | K::PartialExit { block } | K::Indirect { block, .. } => {
                    block_roots(blocks[&block], self.render, &mut todo, pay)?;
                }
                K::Transfer { edge } => todo.extend(transfers[&edge].arguments.iter().copied()),
                K::If { .. } => {
                    todo.push(controls[id].condition);
                }
                K::While {
                    condition,
                    exit_edge,
                    ..
                } => {
                    if condition.is_some() {
                        todo.push(controls[id].condition);
                    }
                    if let Some(edge) = exit_edge {
                        todo.extend(transfers[&edge].arguments.iter().copied());
                    }
                }
                K::Exit { block } => {
                    if self.render.construction(block).is_none() {
                        todo.extend(blocks[&block].exit_values.iter().copied());
                    }
                }
                K::Dispatcher { ref cases, .. } => {
                    for case in cases {
                        block_roots(blocks[&case.block], self.render, &mut todo, pay)?;
                        for transition in &case.transitions {
                            let transfer = transfers[&transition.edge];
                            if !pay(transfer.arguments.len().saturating_add(1)) {
                                return Err(DiscoveryError::Payment);
                            }
                            todo.extend(transfer.arguments.iter().copied());
                        }
                    }
                }
                _ => {}
            }
        }
        let direct: BTreeSet<_> = todo.iter().copied().collect();
        let mut reachable = BTreeSet::new();
        while let Some(id) = todo.pop() {
            if !pay(1) {
                return Err(DiscoveryError::Payment);
            }
            if !reachable.insert(id) {
                continue;
            }
            let expression = expressions
                .get(&id)
                .ok_or(DiscoveryError::Invalid(Error::Missing(id)))?;
            let operands = expression.kind.operands();
            if !pay(operands.len()) {
                return Err(DiscoveryError::Payment);
            }
            todo.extend(operands);
        }
        Ok((reachable, direct))
    }

    pub fn discover(
        &self,
        plan: &mut Plan,
        next: &mut u64,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), DiscoveryError> {
        let (reachable, direct) = self.reachable(pay)?;
        let mut frequencies = BTreeMap::<u64, usize>::new();
        for root in &reachable {
            if !pay(1) {
                return Err(DiscoveryError::Payment);
            }
            if let Some(shape) = self.fingerprints.expression(*root)
                && shape.operations >= 3
            {
                *frequencies.entry(shape.hash).or_default() += 1;
            }
        }
        let mut groups = BTreeMap::<u64, Vec<Candidate>>::new();
        for root in reachable {
            let shape = self
                .fingerprints
                .expression(root)
                .ok_or(DiscoveryError::Invalid(Error::Missing(root)))?;
            if shape.operations < 3 || shape.bytes < 64 {
                continue;
            }
            if frequencies[&shape.hash] < 2 && !direct.contains(&root) {
                continue;
            }
            groups.entry(shape.hash).or_default().push(Candidate {
                root,
                bytes: shape.bytes,
                direct: direct.contains(&root),
                parameters: None,
            });
        }
        let mut groups: Vec<_> = groups.into_values().collect();
        if !pay(groups.len().saturating_mul(16)) {
            return Err(DiscoveryError::Payment);
        }
        groups.sort_by_key(|group| {
            std::cmp::Reverse(
                group
                    .iter()
                    .map(|candidate| candidate.bytes)
                    .max()
                    .unwrap_or(0),
            )
        });
        let mut selected = BTreeSet::new();
        let mut hidden = BTreeSet::new();
        let mut outlined = BTreeSet::new();
        let mut weight = self.program.record_count();
        for procedure in &plan.procedures {
            weight = weight
                .saturating_add(procedure.interface.record_count())
                .saturating_add(3);
        }
        for invocation in &plan.invocations {
            weight = weight
                .saturating_add(invocation.bindings.len().saturating_mul(2))
                .saturating_add(6);
        }
        for mut group in groups {
            group.retain(|candidate| {
                !selected.contains(&candidate.root) && !hidden.contains(&candidate.root)
            });
            if group.is_empty()
                || (group.len() == 1 && (!group[0].direct || outlined.contains(&group[0].root)))
            {
                continue;
            }
            for candidate in &mut group {
                candidate.parameters =
                    candidate_region(self.index.arithmetic_region(candidate.root, pay))?
                        .map(|region| region.interface.inputs.len());
            }
            group.retain(|candidate| candidate.parameters.is_some());
            group.sort_by_key(|candidate| {
                (
                    std::cmp::Reverse(candidate.parameters.unwrap_or(0)),
                    std::cmp::Reverse(candidate.bytes),
                    candidate.root,
                )
            });
            for candidate in &group {
                let root = candidate.root;
                if selected.contains(&root) || hidden.contains(&root) {
                    continue;
                }
                let Some(canonical) = candidate_region(self.index.arithmetic_region(root, pay))?
                else {
                    continue;
                };
                let computations = canonical
                    .entities
                    .len()
                    .saturating_sub(canonical.interface.inputs.len());
                if computations < 3 || canonical.interface.inputs.len() > 32 {
                    continue;
                }
                let bytes = rendered(self.render.render_expression(root, &mut *pay))?
                    .map_or(candidate.bytes, |text| text.len());
                let mut matches = Vec::<(Candidate, Vec<SharedBinding>)>::new();
                let mut pending_weight = weight
                    .saturating_add(canonical.interface.record_count())
                    .saturating_add(9);
                for actual in &group {
                    if actual.root == root
                        || selected.contains(&actual.root)
                        || hidden.contains(&actual.root)
                    {
                        continue;
                    }
                    let Some(actual_region) =
                        candidate_region(self.index.arithmetic_region(actual.root, pay))?
                    else {
                        continue;
                    };
                    // A helper cannot invoke itself through a strict subterm.
                    // Hash collisions remain harmless and pay direct checking.
                    if canonical.entities.contains(&actual.root)
                        || actual_region.entities.contains(&root)
                    {
                        continue;
                    }
                    let bindings = match self.index.correspondence(&canonical, &actual_region, pay)
                    {
                        Ok(bindings) => bindings,
                        Err(error) if rejected(&error) => continue,
                        Err(error) => return Err(DiscoveryError::source(error)),
                    };
                    match self.index.verify_correspondence(
                        &canonical,
                        &actual_region,
                        &bindings,
                        pay,
                    ) {
                        Ok(()) => {
                            pending_weight = pending_weight
                                .saturating_add(bindings.len().saturating_mul(2))
                                .saturating_add(6);
                            if pending_weight > crate::MAX_LIFTED_RECORDS {
                                return Err(DiscoveryError::source(Error::Capacity {
                                    resource: SharedCapacity::ProgramRecords,
                                    required: pending_weight,
                                    limit: crate::MAX_LIFTED_RECORDS,
                                }));
                            }
                            matches.push((*actual, bindings));
                        }
                        Err(error) if rejected(&error) => {}
                        Err(error) => return Err(DiscoveryError::source(error)),
                    }
                }
                let calls = matches.len().saturating_add(1);
                let call = call_cost(canonical.interface.inputs.len());
                let source_cost = matches
                    .iter()
                    .fold(bytes, |n, (actual, _)| n.saturating_add(actual.bytes));
                let shared_cost = bytes
                    .saturating_add(call)
                    .saturating_add(24)
                    .saturating_add(call.saturating_mul(calls));
                // A long single computation is also worth naming: its DAG body
                // is rendered as sequential local bindings, keeping one line
                // from expanding the same subterms many times. Only maximal
                // single-use roots qualify, preventing chains of tiny wrappers.
                let readability = candidate.direct && computations >= 8 && bytes >= 128;
                let repeated_readability = calls > 1 && computations >= 4 && bytes >= 80;
                if (calls == 1 && (!readability || outlined.contains(&root)))
                    || (calls > 1 && source_cost <= shared_cost && !repeated_readability)
                {
                    continue;
                }
                let pid = EntityId(*next);
                *next += 1;
                let procedure = SharedProcedure {
                    id: pid,
                    kind: canonical.kind,
                    body: root,
                    interface: canonical.interface.clone(),
                };
                let iid = EntityId(*next);
                *next += 1;
                let mut invocations = vec![
                    self.index
                        .arithmetic_invocation(iid, pid, root, Vec::new())
                        .map_err(DiscoveryError::source)?,
                ];
                selected.insert(root);
                outlined.extend(canonical.entities.iter().copied().filter(|id| *id != root));
                for (actual, bindings) in matches {
                    if selected.contains(&actual.root) || hidden.contains(&actual.root) {
                        continue;
                    }
                    let Some(region) =
                        candidate_region(self.index.arithmetic_region(actual.root, pay))?
                    else {
                        continue;
                    };
                    let iid = EntityId(*next);
                    *next += 1;
                    invocations.push(
                        self.index
                            .arithmetic_invocation(iid, pid, actual.root, bindings)
                            .map_err(DiscoveryError::source)?,
                    );
                    selected.insert(actual.root);
                    hidden.extend(region.entities);
                }
                weight = weight
                    .saturating_add(procedure.interface.record_count())
                    .saturating_add(3);
                for invocation in &invocations {
                    weight = weight
                        .saturating_add(invocation.bindings.len().saturating_mul(2))
                        .saturating_add(6);
                }
                if weight > crate::MAX_LIFTED_RECORDS {
                    return Err(DiscoveryError::source(Error::Capacity {
                        resource: SharedCapacity::ProgramRecords,
                        required: weight,
                        limit: crate::MAX_LIFTED_RECORDS,
                    }));
                }
                if !pay(procedure
                    .interface
                    .record_count()
                    .saturating_add(invocations.len()))
                {
                    return Err(DiscoveryError::Payment);
                }
                plan.procedures.push(procedure);
                plan.invocations.extend(invocations);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{DiscoveryError, Error, candidate_region};
    use crate::EntityId;

    #[test]
    fn unsupported_arithmetic_candidates_do_not_invalidate_the_base_source() {
        assert!(matches!(
            candidate_region(Err(Error::Region(EntityId(7)))),
            Ok(None)
        ));
        assert!(matches!(
            candidate_region(Err(Error::Missing(EntityId(7)))),
            Err(DiscoveryError::Invalid(Error::Missing(EntityId(7))))
        ));
        assert!(matches!(
            candidate_region(Err(Error::Limit)),
            Err(DiscoveryError::Payment)
        ));
    }
}

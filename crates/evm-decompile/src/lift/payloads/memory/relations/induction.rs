//! Propose recurrences from one iteration, then independently admit them by
//! simultaneous entry/backedge induction over a freshly replayed source region.
use super::{Atom, EntityId, Expr, Flow, MAX_TERMS, Range, Source, State, U256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
#[derive(Clone, Debug, PartialEq, Eq)]
struct Recurrence {
    initial: Expr,
    step: U256,
    range: Range,
}
#[derive(Clone, Debug)]
pub(super) struct Proof {
    header: EntityId,
    scalars: BTreeMap<EntityId, Recurrence>,
    cells: BTreeMap<Expr, Recurrence>,
    bound: U256,
}
#[derive(Clone, Debug)]
struct Candidate {
    proof: Proof,
    // Flat predecessor certificates avoid recursive checking and recursive
    // destruction. Their source SCC order is re-established by the verifier.
    dependencies: BTreeMap<EntityId, Proof>,
}
impl std::ops::Deref for Candidate {
    type Target = Proof;
    fn deref(&self) -> &Proof {
        &self.proof
    }
}
impl std::ops::DerefMut for Candidate {
    fn deref_mut(&mut self) -> &mut Proof {
        &mut self.proof
    }
}
struct Shape {
    header: EntityId,
    order: Vec<EntityId>,
    entries: Vec<EntityId>,
    backedges: Vec<EntityId>,
}
impl<P: FnMut(usize) -> bool> Source<'_, P> {
    fn shape(&mut self, component: &[EntityId]) -> Option<Shape> {
        if !self.charge(component.len().saturating_mul(8).saturating_add(1)) {
            return None;
        }
        let members: BTreeSet<_> = component.iter().copied().collect();
        let mut entries = Vec::new();
        let mut headers = BTreeSet::new();
        for &id in component {
            if Some(id) == self.ir.entry || self.blocks[&id].terminator.is_partial() {
                return None;
            }
            if !self.charge(self.incoming.get(&id).map_or(0, Vec::len).saturating_add(1)) {
                return None;
            }
            let incoming = self.incoming.get(&id).cloned().unwrap_or_default();
            for edge in incoming {
                if !members.contains(&edge.from) {
                    headers.insert(id);
                    entries.push(edge.id);
                }
            }
        }
        if headers.len() != 1 {
            return None;
        }
        let header = *headers.first()?;
        let mut degree = BTreeMap::<_, usize>::new();
        let mut backedges = Vec::new();
        for &id in component {
            if !self.charge(self.outgoing.get(&id).map_or(0, Vec::len).saturating_add(1)) {
                return None;
            }
            let edges = self.outgoing.get(&id).cloned().unwrap_or_default();
            degree.entry(id).or_default();
            for edge in edges {
                if edge.to == header {
                    backedges.push(edge.id);
                } else if members.contains(&edge.to) {
                    *degree.entry(edge.to).or_default() += 1;
                }
            }
        }
        let mut ready: VecDeque<_> = degree
            .iter()
            .filter_map(|(&id, &d)| (d == 0).then_some(id))
            .collect();
        let mut order = Vec::new();
        while let Some(id) = ready.pop_front() {
            if !self.charge(self.outgoing.get(&id).map_or(0, Vec::len).saturating_add(1)) {
                return None;
            }
            order.push(id);
            for edge in self.outgoing.get(&id).into_iter().flatten() {
                if edge.to != header && members.contains(&edge.to) {
                    let count = degree.get_mut(&edge.to)?;
                    *count = count.checked_sub(1)?;
                    if *count == 0 {
                        ready.push_back(edge.to);
                    }
                }
            }
        }
        (order.len() == component.len() && order.first() == Some(&header) && !backedges.is_empty())
            .then_some(Shape {
                header,
                order,
                entries,
                backedges,
            })
    }
    fn entry_state(&mut self, shape: &Shape, flows: &BTreeMap<EntityId, Flow>) -> Option<State> {
        let mut initial = None;
        for id in &shape.entries {
            match flows.get(id)? {
                Flow::Dead => {}
                Flow::Unknown => return None,
                Flow::Known(state) => {
                    if !self.work(state, MAX_TERMS) {
                        return None;
                    }
                    initial = Some(match initial {
                        None => state.clone(),
                        Some(previous) => {
                            if !self.work(&previous, 1) {
                                return None;
                            }
                            self.merge(previous, state, None)
                        }
                    });
                }
            }
        }
        initial
    }
    fn run_region(
        &mut self,
        shape: &Shape,
        initial: State,
        optimistic: bool,
        publish: bool,
    ) -> BTreeMap<EntityId, Flow> {
        let mut result = BTreeMap::new();
        for &id in &shape.order {
            let block = self.blocks[&id];
            let flow = if id == shape.header {
                if !self.work(&initial, 1) {
                    Flow::Unknown
                } else {
                    Flow::Known(initial.clone())
                }
            } else {
                self.join(block, &result, Some(shape.header))
            };
            self.process(
                block,
                flow,
                Some(shape.header),
                optimistic,
                &mut result,
                publish,
            );
        }
        result
    }
    fn propose(&mut self, shape: &Shape, initial: &State) -> Option<Candidate> {
        if !self.work(initial, 1) {
            return None;
        }
        let first = self.run_region(shape, initial.clone(), true, false);
        if !self.charge(self.certificates.len().saturating_add(1)) {
            return None;
        }
        let cost = self.certificates.values().fold(1_usize, |n, p| {
            n.saturating_add(
                p.scalars
                    .len()
                    .saturating_add(p.cells.len())
                    .saturating_mul(MAX_TERMS * 2),
            )
        });
        if !self.charge(cost) {
            return None;
        }
        let mut candidate = Candidate {
            proof: Proof {
                header: shape.header,
                scalars: BTreeMap::new(),
                cells: BTreeMap::new(),
                bound: U256::ZERO,
            },
            dependencies: self.certificates.clone(),
        };
        for (&id, expr) in &initial.values {
            if let Some(step) =
                self.common_step(&first, &shape.backedges, |s| s.values.get(&id), expr)
            {
                candidate.scalars.insert(
                    id,
                    Recurrence {
                        initial: expr.clone(),
                        step,
                        range: initial.range(expr),
                    },
                );
            }
        }
        for (address, expr) in &initial.cells {
            if let Some(step) =
                self.common_step(&first, &shape.backedges, |s| s.cells.get(address), expr)
            {
                candidate.cells.insert(
                    address.clone(),
                    Recurrence {
                        initial: expr.clone(),
                        step,
                        range: initial.range(expr),
                    },
                );
            }
        }
        let seeded = self.seed(initial, &candidate, U256::MAX)?;
        let probe = self.run_region(shape, seeded, true, false);
        let mut bound = U256::ZERO;
        let mut progress = false;
        for edge in &shape.backedges {
            match probe.get(edge)? {
                Flow::Dead => {}
                Flow::Unknown => return None,
                Flow::Known(state) => {
                    let high = state
                        .atoms
                        .get(&Atom::Iteration(shape.header))
                        .copied()
                        .unwrap_or(Range::FULL)
                        .high;
                    let next = high.checked_add(U256::from(1))?;
                    bound = bound.max(next);
                    progress = true;
                }
            }
        }
        if progress && !candidate.scalars.values().any(|r| r.step != U256::ZERO) {
            return None;
        }
        candidate.bound = bound;
        for recurrence in candidate
            .proof
            .scalars
            .values_mut()
            .chain(candidate.proof.cells.values_mut())
        {
            let range = initial.range(&recurrence.initial);
            recurrence.range = if recurrence.step > (U256::MAX >> 1) {
                Range {
                    low: U256::ZERO,
                    high: range.high,
                }
            } else {
                Range {
                    low: range.low,
                    high: range
                        .high
                        .checked_add(recurrence.step.checked_mul(bound)?)?,
                }
            };
        }
        Some(candidate)
    }
    fn common_step<'b>(
        &mut self,
        flows: &'b BTreeMap<EntityId, Flow>,
        edges: &[EntityId],
        get: impl Fn(&'b State) -> Option<&'b Expr>,
        initial: &Expr,
    ) -> Option<U256> {
        let mut common = None;
        for edge in edges {
            if !self.charge(MAX_TERMS * 2) {
                return None;
            }
            match flows.get(edge)? {
                Flow::Dead => {}
                Flow::Unknown => return None,
                Flow::Known(state) => {
                    let step = get(state)?.sub(initial).exact()?;
                    if common.is_some_and(|old| old != step) {
                        return None;
                    }
                    common = Some(step);
                }
            }
        }
        common.or(Some(U256::ZERO))
    }
    fn seed(&mut self, initial: &State, candidate: &Proof, bound: U256) -> Option<State> {
        if !self.work(
            initial,
            candidate
                .scalars
                .len()
                .saturating_add(candidate.cells.len())
                .saturating_mul(MAX_TERMS * 2),
        ) {
            return None;
        }
        let mut result = initial.clone();
        let iteration = Expr::atom(Atom::Iteration(candidate.header));
        result.atoms.insert(
            Atom::Iteration(candidate.header),
            Range {
                low: U256::ZERO,
                high: bound,
            },
        );
        result.values = candidate
            .scalars
            .iter()
            .map(|(&id, r)| (id, r.initial.add(&iteration.scale(r.step))))
            .collect();
        result.cells = candidate
            .cells
            .iter()
            .map(|(address, r)| (address.clone(), r.initial.add(&iteration.scale(r.step))))
            .collect();
        for recurrence in candidate.scalars.values().chain(candidate.cells.values()) {
            // This is a simultaneous induction hypothesis, never a source fact
            // until entry and every backedge below have independently passed.
            if bound == U256::MAX && recurrence.step != U256::ZERO {
                if recurrence.step > (U256::MAX >> 1) {
                    result.real.insert(
                        recurrence.initial.add(&iteration.scale(recurrence.step)),
                        Range {
                            low: U256::ZERO,
                            high: recurrence.range.high,
                        },
                    );
                }
            } else {
                result.real.insert(
                    recurrence.initial.add(&iteration.scale(recurrence.step)),
                    recurrence.range,
                );
            }
        }
        Some(result)
    }
    /// This admission path never calls candidate discovery. The supplied fields
    /// are checked against actual source entries, operations and every backedge.
    /// All observations are transactional until the last obligation succeeds.
    fn verify_candidate(
        &mut self,
        component: &[EntityId],
        candidate: &Candidate,
        _incoming: &BTreeMap<EntityId, Flow>,
    ) -> Option<BTreeMap<EntityId, Flow>> {
        if !self.valid_cfg {
            return None;
        }
        let shape = self.shape(component)?;
        if shape.header != candidate.header || candidate.bound == U256::MAX {
            return None;
        }
        let entry_flows = self.replay_entries(&shape, component, &candidate.dependencies)?;
        let initial = self.entry_state(&shape, &entry_flows)?;
        self.verify_body(&shape, &candidate.proof, initial, true)
    }
    fn verify_body(
        &mut self,
        shape: &Shape,
        candidate: &Proof,
        initial: State,
        publish: bool,
    ) -> Option<BTreeMap<EntityId, Flow>> {
        if shape.header != candidate.header || candidate.bound == U256::MAX {
            return None;
        }
        for (&id, recurrence) in &candidate.scalars {
            if initial.values.get(&id) != Some(&recurrence.initial)
                || !Self::contains(recurrence.range, initial.ordinary(&recurrence.initial))
            {
                return None;
            }
        }
        for (address, recurrence) in &candidate.cells {
            if initial.cells.get(address) != Some(&recurrence.initial)
                || !Self::contains(recurrence.range, initial.ordinary(&recurrence.initial))
            {
                return None;
            }
        }
        for recurrence in candidate.scalars.values().chain(candidate.cells.values()) {
            if recurrence.step != U256::ZERO
                && (recurrence
                    .initial
                    .exact()
                    .is_some_and(|v| v > (U256::MAX >> 1))
                    || !recurrence.initial.integral_add(
                        &Expr::atom(Atom::Iteration(candidate.header)).scale(recurrence.step),
                    ))
            {
                return None;
            }
        }
        let seed = self.seed(&initial, candidate, candidate.bound)?;
        for expr in seed
            .values
            .values()
            .chain(seed.cells.values())
            .chain(seed.cells.keys())
        {
            if !self.charge(MAX_TERMS) || seed.ordinary(expr).is_none() {
                return None;
            }
        }
        let checkpoint = self.interned.len();
        let previous = std::mem::take(&mut self.admitted);
        let result = self.run_region(shape, seed, false, publish);
        let checked = self.check_backedges(shape, candidate, &result);
        let tentative = std::mem::replace(&mut self.admitted, previous);
        let checked = checked && self.charge(tentative.len().saturating_add(1));
        if !checked || self.refused {
            let _ = self.charge(
                self.interned
                    .len()
                    .saturating_sub(checkpoint)
                    .saturating_mul(MAX_TERMS)
                    .saturating_add(1),
            );
            while self.interned.len() > checkpoint {
                if let Some(key) = self.interned.pop() {
                    self.expressions.remove(&key);
                }
            }
            return None;
        }
        self.admitted.extend(tentative);
        Some(result)
    }
    /// Reconstruct the base from raw source and independently supplied earlier
    /// loop certificates. This traversal never discovers candidates or recurses.
    fn replay_entries(
        &mut self,
        shape: &Shape,
        component: &[EntityId],
        proofs: &BTreeMap<EntityId, Proof>,
    ) -> Option<BTreeMap<EntityId, Flow>> {
        if !self.charge(
            component
                .len()
                .saturating_add(self.ir.edges.len().saturating_mul(2))
                .saturating_add(shape.entries.len())
                .saturating_add(1),
        ) {
            return None;
        }
        let forbidden: BTreeSet<_> = component.iter().copied().collect();
        let edge_index: BTreeMap<_, _> = self.ir.edges.iter().map(|e| (e.id, e)).collect();
        let mut pending = shape
            .entries
            .iter()
            .map(|e| edge_index.get(e).map(|e| e.from))
            .collect::<Option<Vec<_>>>()?;
        let mut ancestors = BTreeSet::new();
        while let Some(id) = pending.pop() {
            if !self.charge(self.incoming.get(&id).map_or(0, Vec::len).saturating_add(2))
                || forbidden.contains(&id)
            {
                return None;
            }
            if !ancestors.insert(id) {
                continue;
            }
            for edge in self.incoming.get(&id).into_iter().flatten() {
                pending.push(edge.from);
            }
        }
        if !ancestors.contains(&self.ir.entry?) {
            return None;
        }
        let components = super::super::cycles::components(self.ir, &mut self.pay)?;
        let mut flows = BTreeMap::new();
        for group in components {
            if !self.charge(group.len().saturating_add(1)) {
                return None;
            }
            if !group.iter().any(|id| ancestors.contains(id)) {
                continue;
            }
            if group.iter().any(|id| !ancestors.contains(id)) {
                return None;
            }
            let id = group[0];
            if !self.charge(self.outgoing.get(&id).map_or(0, Vec::len).saturating_add(1)) {
                return None;
            }
            if group.len() > 1
                || self
                    .outgoing
                    .get(&id)
                    .into_iter()
                    .flatten()
                    .any(|e| e.to == id)
            {
                let shape = self.shape(&group)?;
                let proof = proofs.get(&shape.header)?;
                let initial = self.entry_state(&shape, &flows)?;
                let result = self.verify_body(&shape, proof, initial, false)?;
                if !self.charge(result.len().saturating_add(1)) {
                    return None;
                }
                flows.extend(result);
            } else {
                let block = *self.blocks.get(&id)?;
                let flow = if Some(id) == self.ir.entry {
                    Flow::Known(State {
                        memory: Some(block.memory_parameter),
                        effect: Some(block.effect_parameter),
                        ..State::default()
                    })
                } else {
                    self.join(block, &flows, None)
                };
                self.process(block, flow, None, false, &mut flows, false);
            }
        }
        Some(flows)
    }
    fn contains(outer: Range, inner: Option<Range>) -> bool {
        inner.is_some_and(|inner| outer.low <= inner.low && inner.high <= outer.high)
    }
    fn check_backedges(
        &mut self,
        shape: &Shape,
        candidate: &Proof,
        result: &BTreeMap<EntityId, Flow>,
    ) -> bool {
        let next =
            Expr::atom(Atom::Iteration(candidate.header)).add(&Expr::constant(U256::from(1)));
        for edge in &shape.backedges {
            if !self.charge(
                candidate
                    .scalars
                    .len()
                    .saturating_add(candidate.cells.len())
                    .saturating_mul(MAX_TERMS * 2)
                    .saturating_add(1),
            ) {
                return false;
            }
            let Some(flow) = result.get(edge) else {
                return false;
            };
            let state = match flow {
                Flow::Dead => continue,
                Flow::Unknown => return false,
                Flow::Known(state) => state,
            };
            let progress = state
                .atoms
                .get(&Atom::Iteration(candidate.header))
                .copied()
                .unwrap_or(Range::FULL);
            if progress
                .high
                .checked_add(U256::from(1))
                .is_none_or(|v| v > candidate.bound)
            {
                return false;
            }
            for (&id, recurrence) in &candidate.scalars {
                let expected = recurrence.initial.add(&next.scale(recurrence.step));
                if state.values.get(&id) != Some(&expected)
                    || !Self::contains(recurrence.range, state.ordinary(&expected))
                {
                    return false;
                }
            }
            for (address, recurrence) in &candidate.cells {
                let expected = recurrence.initial.add(&next.scale(recurrence.step));
                if state.cells.get(address) != Some(&expected)
                    || !Self::contains(recurrence.range, state.ordinary(&expected))
                {
                    return false;
                }
            }
        }
        true
    }
    pub(super) fn loop_region(
        &mut self,
        component: &[EntityId],
        flows: &mut BTreeMap<EntityId, Flow>,
    ) {
        let result = self.shape(component).and_then(|shape| {
            let initial = self.entry_state(&shape, flows)?;
            let candidate = self.propose(&shape, &initial)?;
            let result = self.verify_candidate(component, &candidate, flows)?;
            let cost = candidate
                .scalars
                .len()
                .saturating_add(candidate.cells.len())
                .saturating_mul(MAX_TERMS * 2)
                .saturating_add(1);
            if !self.charge(cost) {
                return None;
            }
            self.certificates.insert(candidate.header, candidate.proof);
            Some(result)
        });
        if let Some(result) = result {
            if self.charge(result.len().saturating_add(1)) {
                flows.extend(result);
            }
        } else {
            for id in component {
                if !self.charge(self.outgoing.get(id).map_or(0, Vec::len).saturating_add(1)) {
                    return;
                }
                let edges = self.outgoing.get(id).cloned().unwrap_or_default();
                for edge in edges {
                    flows.insert(edge.id, Flow::Unknown);
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/lifted/error_payloads/loops/source_proof.rs"]
mod source_proof;
#[cfg(test)]
mod tests;

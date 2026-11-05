//! Trivial-phi worklist, followed by anchored SCCs around retained join roots.
use super::{Copies, components};
use crate::{EntityId, Ir};
use std::collections::{BTreeMap, VecDeque};

struct Parameters {
    ids: Vec<EntityId>,
    indices: BTreeMap<EntityId, usize>,
    users: Vec<Vec<usize>>,
    membership: Vec<usize>,
    generation: usize,
    queued: Vec<bool>,
    last_root: Vec<EntityId>,
}

impl Copies<'_> {
    /// Only discovery has intermediate alias chains. Compress them here, then
    /// publish a direct canonical map so projection/provenance never walk chains.
    fn resolve(
        &mut self,
        mut value: EntityId,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<EntityId> {
        let mut path = Vec::new();
        while let Some(next) = self.aliases.get(&value).copied() {
            if !pay(2) {
                return None;
            }
            path.push(value);
            value = next;
        }
        for parameter in path {
            self.aliases.insert(parameter, value);
        }
        Some(value)
    }

    pub(super) fn discover(&mut self, ir: &Ir, pay: &mut impl FnMut(usize) -> bool) -> Option<()> {
        let n = self.parameters.len();
        if !pay(n.saturating_mul(12).saturating_add(1)) {
            return None;
        }
        let ids: Vec<_> = self.parameters.keys().copied().collect();
        let indices: BTreeMap<_, _> = ids.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let mut dependencies = vec![Vec::new(); n];
        let mut users = vec![Vec::new(); n];
        for edge in &ir.edges {
            if !pay(edge.arguments.len().saturating_mul(2).saturating_add(1)) {
                return None;
            }
            for (parameter, argument) in ir.blocks[self.blocks[&edge.to]]
                .parameters
                .iter()
                .zip(&edge.arguments)
            {
                if let Some(dependency) = indices.get(argument) {
                    let node = indices[parameter];
                    dependencies[node].push(*dependency);
                    users[*dependency].push(node);
                }
            }
        }
        let mut tasks = components::build(&dependencies, pay)?.members;
        tasks.reverse();
        let mut graph = Parameters {
            last_root: ids.clone(),
            ids,
            indices,
            users,
            membership: vec![0; n],
            generation: 0,
            queued: vec![false; n],
        };
        while let Some(members) = tasks.pop() {
            self.refine(ir, &mut graph, members, &mut tasks, pay)?;
        }
        if !pay(self.aliases.len().saturating_add(1)) {
            return None;
        }
        let aliases: Vec<_> = self.aliases.keys().copied().collect();
        for alias in aliases {
            let root = self.resolve(alias, pay)?;
            self.aliases.insert(alias, root);
        }
        Some(())
    }

    fn refine(
        &mut self,
        ir: &Ir,
        graph: &mut Parameters,
        mut members: Vec<usize>,
        tasks: &mut Vec<Vec<usize>>,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Option<()> {
        if !pay(members.len().saturating_mul(6).saturating_add(1)) {
            return None;
        }
        members.retain(|node| !self.aliases.contains_key(&graph.ids[*node]));
        graph.generation += 1;
        let generation = graph.generation;
        for node in &members {
            graph.membership[*node] = generation;
        }
        let mut has_anchor = false;
        for node in &members {
            let (block, slot) = self.parameters[&graph.ids[*node]];
            if !pay(self.incoming[block].len().saturating_add(1)) {
                return None;
            }
            has_anchor |= !self.eligible(ir, block);
            for edge in &self.incoming[block] {
                let argument = ir.edges[*edge].arguments[slot];
                has_anchor |= graph
                    .indices
                    .get(&argument)
                    .is_none_or(|dependency| graph.membership[*dependency] != generation);
            }
        }
        if !has_anchor {
            return Some(());
        }
        let mut todo: VecDeque<_> = members.iter().copied().collect();
        for node in &members {
            graph.queued[*node] = true;
        }
        while let Some(node) = todo.pop_front() {
            graph.queued[node] = false;
            if !pay(1) {
                return None;
            }
            let parameter = graph.ids[node];
            let mut root = self.resolve(parameter, pay)?;
            let (block, slot) = self.parameters[&parameter];
            if root == parameter && self.eligible(ir, block) {
                let mut candidate = None;
                let mut valid = true;
                if !pay(self.incoming[block].len()) {
                    return None;
                }
                for i in 0..self.incoming[block].len() {
                    let argument = ir.edges[self.incoming[block][i]].arguments[slot];
                    let incoming = self.resolve(argument, pay)?;
                    if incoming != parameter {
                        valid &= candidate.is_none_or(|candidate| candidate == incoming);
                        candidate = Some(incoming);
                    }
                }
                if let Some(candidate) = candidate
                    .filter(|candidate| valid && self.available_at_entry(*candidate, block))
                {
                    self.aliases.insert(parameter, candidate);
                    root = candidate;
                }
            }
            // A changed representative wakes only its direct phi users. Aliased
            // users forward a change once per new representative; unchanged raw
            // cycles do not keep circulating work. No graph search per candidate.
            if root != graph.last_root[node] {
                graph.last_root[node] = root;
                if !pay(graph.users[node].len()) {
                    return None;
                }
                for user in &graph.users[node] {
                    if graph.membership[*user] == generation && !graph.queued[*user] {
                        graph.queued[*user] = true;
                        todo.push_back(*user);
                    }
                }
            }
        }
        members.retain(|node| {
            let keep = !self.aliases.contains_key(&graph.ids[*node]);
            if !keep {
                graph.membership[*node] = 0;
            }
            keep
        });
        let mut candidate = None;
        let mut equivalent = true;
        let mut eligible = true;
        let mut interior = Vec::new();
        for node in &members {
            let (block, slot) = self.parameters[&graph.ids[*node]];
            let allowed = self.eligible(ir, block);
            eligible &= allowed;
            let mut boundary = !allowed;
            if !pay(self.incoming[block].len().saturating_add(1)) {
                return None;
            }
            for i in 0..self.incoming[block].len() {
                let argument = ir.edges[self.incoming[block][i]].arguments[slot];
                let root = self.resolve(argument, pay)?;
                if graph
                    .indices
                    .get(&root)
                    .is_some_and(|i| graph.membership[*i] == generation)
                {
                    continue;
                }
                boundary = true;
                equivalent &= candidate.is_none_or(|candidate| candidate == root);
                candidate = Some(root);
            }
            if !boundary {
                interior.push(*node);
            }
        }
        if let Some(root) = candidate.filter(|_| equivalent && eligible) {
            if !pay(members.len().saturating_mul(2)) {
                return None;
            }
            if members
                .iter()
                .all(|node| self.available_at_entry(root, self.parameters[&graph.ids[*node]].0))
            {
                for node in members {
                    self.aliases.insert(graph.ids[node], root);
                }
                return Some(());
            }
        }
        // A nontrivial boundary phi stays a real definition. Its interior can
        // still contain trivial SCCs anchored by that definition, e.g.
        // p=phi(a,b,q), q=phi(p,r), r=q. Refinement removes at least one boundary
        // each time; tasks hold disjoint subsets and use linear graph storage.
        if interior.is_empty() || interior.len() == members.len() {
            return Some(());
        }
        if !pay(interior.len().saturating_mul(4)) {
            return None;
        }
        let local: BTreeMap<_, _> = interior
            .iter()
            .enumerate()
            .map(|(i, node)| (*node, i))
            .collect();
        let mut dependencies = vec![Vec::new(); interior.len()];
        for (i, node) in interior.iter().enumerate() {
            let (block, slot) = self.parameters[&graph.ids[*node]];
            if !pay(self.incoming[block].len().saturating_add(1)) {
                return None;
            }
            for edge in 0..self.incoming[block].len() {
                let argument = ir.edges[self.incoming[block][edge]].arguments[slot];
                let root = self.resolve(argument, pay)?;
                if let Some(dependency) = graph.indices.get(&root).and_then(|node| local.get(node))
                {
                    dependencies[i].push(*dependency);
                }
            }
        }
        let components = components::build(&dependencies, pay)?;
        for component in components.members.into_iter().rev() {
            if !pay(component.len()) {
                return None;
            }
            tasks.push(component.into_iter().map(|i| interior[i]).collect());
        }
        Some(())
    }
}

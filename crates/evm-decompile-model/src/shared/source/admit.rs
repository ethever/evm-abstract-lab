//! Supplied definition/site admission with exact source ownership.
use super::{Error, SharedSourceIndex, charge};
use crate::EntityId;
use crate::LiftedProgram;
use std::collections::{BTreeMap, BTreeSet};
impl SharedSourceIndex<'_> {
    pub(crate) fn validate(
        &self,
        program: &LiftedProgram,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<(), Error> {
        charge(pay, program.record_count().saturating_mul(8))?;
        program
            .validate()
            .map_err(|_| Error::Correspondence(EntityId(0)))?;
        let mut source_ids: BTreeSet<_> = self
            .nodes
            .keys()
            .chain(self.blocks.keys())
            .chain(self.values.keys())
            .chain(self.edges.keys())
            .chain(self.operations.keys())
            .chain(self.expressions.keys())
            .copied()
            .collect();
        source_ids.extend(
            program
                .blocks
                .iter()
                .flat_map(|b| std::iter::once(b.id).chain(b.statements.iter().map(|s| s.id))),
        );
        source_ids.extend(program.transfers.iter().map(|t| t.id));
        source_ids.extend(program.controls.iter().map(|c| c.id));
        source_ids.extend(program.payloads.iter().map(|p| p.id));
        source_ids.extend(program.reverts.iter().map(|r| r.id));
        source_ids.extend(program.constructions.iter().map(|c| c.id));
        let mut definitions = BTreeMap::new();
        let mut domain_units = 0_usize;
        for p in &program.procedures {
            if source_ids.contains(&p.id) {
                return Err(Error::Correspondence(p.id));
            }
            if definitions.contains_key(&p.id) {
                return Err(Error::Correspondence(p.id));
            }
            let region = self.region(p.body, pay)?;
            if !region.terminal_closed {
                return Err(Error::Region(p.body));
            }
            if region.interface != p.interface {
                return Err(Error::Interface(p.id));
            }
            domain_units = domain_units
                .saturating_add(region.entities.len())
                .saturating_add(region.nodes.len())
                .saturating_add(region.interface.record_count());
            if domain_units > crate::MAX_LIFTED_RECORDS {
                return Err(Error::Limit);
            }
            definitions.insert(p.id, (p, region));
        }
        let mut sites = BTreeSet::new();
        let mut used = BTreeMap::<EntityId, usize>::new();
        let mut intervals = Vec::new();
        for i in &program.invocations {
            if source_ids.contains(&i.id) || definitions.contains_key(&i.id) {
                return Err(Error::Correspondence(i.id));
            }
            let s = self.sites.get(&i.source).ok_or(Error::Missing(i.source))?;
            if (i.owner, i.position, i.continuation) != (s.owner, s.position, s.continuation) {
                return Err(Error::Site(i.id));
            }
            if !sites.insert(i.source) {
                return Err(Error::Overlap(i.source));
            }
            let (p, canonical) = definitions
                .get(&i.procedure)
                .ok_or(Error::Missing(i.procedure))?;
            let actual = self.region(i.source, pay)?;
            self.verify_correspondence(canonical, &actual, &i.bindings, pay)?;
            if p.body == i.source && i.bindings.iter().any(|b| b.formal != b.actual) {
                return Err(Error::Correspondence(i.id));
            }
            intervals.push((self.order[&i.source], s.end, i.source));
            *used.entry(i.procedure).or_default() += 1;
        }
        charge(pay, intervals.len().saturating_mul(8))?;
        intervals.sort_unstable();
        if let Some(w) = intervals.windows(2).find(|w| w[0].1 > w[1].0) {
            return Err(Error::Overlap(w[1].2));
        }
        for (id, (p, _)) in definitions {
            if used.get(&id).copied().unwrap_or(0) < 2 || !sites.contains(&p.body) {
                return Err(Error::Missing(id));
            }
        }
        Ok(())
    }
}

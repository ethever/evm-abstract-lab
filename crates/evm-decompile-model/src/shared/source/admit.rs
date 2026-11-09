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
            let region = self.source_region(p.kind, p.body, pay)?;
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
                return Err(Error::Capacity {
                    resource: crate::SharedCapacity::DefinitionDomains,
                    required: domain_units,
                    limit: crate::MAX_LIFTED_RECORDS,
                });
            }
            definitions.insert(p.id, (p, region));
        }
        let mut sites = BTreeSet::new();
        let mut used = BTreeMap::<EntityId, usize>::new();
        let mut dependencies = BTreeMap::<EntityId, BTreeSet<EntityId>>::new();
        let mut body_owners = BTreeMap::<EntityId, Vec<EntityId>>::new();
        // The aggregate source domain was paid and bounded above. Index each
        // canonical body's actual code once; dependency construction therefore
        // follows retained code size rather than expanding nested definitions.
        charge(pay, domain_units.saturating_mul(4))?;
        for (id, (procedure, region)) in &definitions {
            dependencies.insert(*id, BTreeSet::new());
            for entity in &region.entities {
                if *entity != procedure.body
                    && (self.nodes.contains_key(entity)
                        || (self.expressions.contains_key(entity)
                            && (!region.nodes.is_empty()
                                || region.interface.inputs.binary_search(entity).is_err())))
                {
                    body_owners.entry(*entity).or_default().push(*id);
                }
            }
        }
        for i in &program.invocations {
            if source_ids.contains(&i.id) || definitions.contains_key(&i.id) {
                return Err(Error::Correspondence(i.id));
            }
            if !sites.insert(i.source) {
                return Err(Error::Overlap(i.source));
            }
            let (p, canonical) = definitions
                .get(&i.procedure)
                .ok_or(Error::Missing(i.procedure))?;
            let actual = self.source_region(p.kind, i.source, pay)?;
            let expected = if actual.nodes.is_empty() {
                self.arithmetic_invocation(i.id, i.procedure, i.source, Vec::new())?
            } else {
                self.invocation(i.id, i.procedure, i.source, Vec::new())?
            };
            if (i.owner, i.position, i.continuation)
                != (expected.owner, expected.position, expected.continuation)
            {
                return Err(Error::Site(i.id));
            }
            self.verify_correspondence(canonical, &actual, &i.bindings, pay)?;
            if p.body == i.source && i.bindings.iter().any(|b| b.formal != b.actual) {
                return Err(Error::Correspondence(i.id));
            }
            if let Some(owners) = body_owners.get(&i.source) {
                charge(pay, owners.len())?;
                for owner in owners {
                    dependencies
                        .get_mut(owner)
                        .ok_or(Error::Missing(*owner))?
                        .insert(i.procedure);
                }
            }
            *used.entry(i.procedure).or_default() += 1;
        }
        check_dependencies(&dependencies, pay)?;
        for (id, (p, _)) in definitions {
            let minimum = if self.expressions.contains_key(&p.body) {
                1
            } else {
                2
            };
            if used.get(&id).copied().unwrap_or(0) < minimum || !sites.contains(&p.body) {
                return Err(Error::Missing(id));
            }
        }
        Ok(())
    }
}

/// Calls form a finite syntax dependency DAG. Cyclic definitions would make
/// rendering/execution expand forever even when every source loop is valid.
fn check_dependencies(
    dependencies: &BTreeMap<EntityId, BTreeSet<EntityId>>,
    pay: &mut impl FnMut(usize) -> bool,
) -> Result<(), Error> {
    let slots = dependencies
        .values()
        .fold(dependencies.len(), |n, edges| n.saturating_add(edges.len()));
    charge(pay, slots.saturating_mul(4))?;
    let mut incoming: BTreeMap<_, usize> = dependencies.keys().map(|id| (*id, 0)).collect();
    for children in dependencies.values() {
        for child in children {
            *incoming.get_mut(child).ok_or(Error::Missing(*child))? += 1;
        }
    }
    let mut ready: Vec<_> = incoming
        .iter()
        .filter_map(|(id, count)| (*count == 0).then_some(*id))
        .collect();
    let mut removed = 0_usize;
    while let Some(id) = ready.pop() {
        removed += 1;
        for child in &dependencies[&id] {
            let count = incoming.get_mut(child).ok_or(Error::Missing(*child))?;
            *count -= 1;
            if *count == 0 {
                ready.push(*child);
            }
        }
    }
    if removed != dependencies.len() {
        return Err(Error::Overlap(
            incoming
                .iter()
                .find(|(_, count)| **count > 0)
                .map_or(EntityId(0), |(id, _)| *id),
        ));
    }
    Ok(())
}

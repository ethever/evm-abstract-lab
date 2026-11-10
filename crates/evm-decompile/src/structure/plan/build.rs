//! Build shared, source-scoped region plans before allocating syntax occurrences.
use super::super::facts::{Facts, Loop, NormalRegion, RegionScope};
use super::{MAX_SOURCE_REFERENCES, Plan, PlanError, PlanId, PlanNode, Quote, Region, quote_node};
use crate::{
    ArtifactId, Completion, DiagnosticCode, DispatchCase, DispatchTransition, EntityId,
    IndirectArm, Ir, Progress, ReadabilityMetrics, ReadabilityReason, RegionAnchor, RegionDecision,
    Strategy, StructureForm, StructurePreference, StructuredKind, Terminator, artifact::Builder,
    budget::count,
};
use std::collections::{BTreeMap, BTreeSet};
mod readability;

// Admission policy for retained representation, not a semantic complexity claim.
// Small natural regions retain their existing syntax; large repeated regions
// are compared with their actual source case/transition representation.
const NATURAL_OCCURRENCE_FLOOR: usize = 16_384;
const REGION_EXPANSION_RATIO: usize = 8;
const PLAN_ARENA_RATIO: usize = 8;
const MAX_PLAN_SLOTS: usize = crate::MAX_STRUCTURED_WIRE_NODES * 4;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RegionKey {
    entry: EntityId,
    continuation: Option<EntityId>,
    loop_header: Option<EntityId>,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct JoinKey {
    left: EntityId,
    right: EntityId,
    continuation: Option<EntityId>,
    loop_header: Option<EntityId>,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct CycleKey {
    entry: EntityId,
    loop_header: Option<EntityId>,
}
enum Admission {
    Keep(PlanId, Assessment),
    Compact(BTreeSet<EntityId>, Assessment),
}
struct Assessment {
    natural: Option<ReadabilityMetrics>,
    dispatcher: Option<ReadabilityMetrics>,
    reason: ReadabilityReason,
}
#[derive(Clone, Copy)]
struct Checkpoint {
    nodes: usize,
    regions: usize,
    cache: usize,
    slots: usize,
}

pub(super) struct Planner<'a, P: FnMut(Progress)> {
    owner: &'a mut Builder<P>,
    ir: &'a Ir,
    facts: &'a Facts,
    loops: &'a BTreeMap<EntityId, Loop>,
    postdom: &'a BTreeMap<EntityId, BTreeSet<EntityId>>,
    nodes: Vec<PlanNode>,
    quotes: Vec<Quote>,
    regions: Vec<Region>,
    cache: BTreeMap<RegionKey, PlanId>,
    cache_log: Vec<RegionKey>,
    arena_slots: usize,
    slot_limit: usize,
    capacity_refused: bool,
    // These source-only facts and the pressure decision survive arena rollback.
    // They contain no PlanId that a truncated arena could invalidate.
    join_cache: BTreeMap<JoinKey, Option<EntityId>>,
    cycle_cache: BTreeMap<CycleKey, bool>,
    // A complete scoped proof may contain source sets for many blocks. Retain
    // only one scope; small positive/negative join answers remain independently
    // memoized under their complete boundary even when this proof is replaced.
    normal_region: Option<(RegionScope, NormalRegion)>,
    memo_limit: usize,
    compact_pressure: bool,
    // Failed expansion evidence is local to source blocks and survives rollback.
    // Unlike arena exhaustion, it does not compact unrelated regions.
    expanded_sources: BTreeSet<EntityId>,
    natural_attempts: usize,
    natural_limit: usize,
    metrics: Vec<ReadabilityMetrics>,
    // The exact trigger travels with its dispatcher through cache reuse. Like
    // the node arena, this metadata is truncated when a proposal rolls back.
    dispatch_reasons: Vec<Option<ReadabilityReason>>,
    failed_operations: Vec<u64>,
    preferred: Vec<bool>,
    source: ArtifactId,
    preferences: BTreeMap<RegionKey, StructurePreference>,
}
pub(in crate::structure) fn build(
    ir: &Ir,
    facts: &Facts,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<Plan> {
    build_with_slot_limit(ir, facts, owner, MAX_PLAN_SLOTS)
}
/// A rejected optional shape may still have an exact compact source form. This
/// fallback is independently verified by the caller under the same work payer.
pub(in crate::structure) fn compact_fallback(
    ir: &Ir,
    facts: &Facts,
    owner: &mut Builder<impl FnMut(Progress)>,
) -> Option<Plan> {
    let mut planner = Planner::new(ir, facts, owner, MAX_PLAN_SLOTS)?;
    let entry = ir.entry?;
    let root = planner.dispatch(entry, None, None, ReadabilityReason::ValidationFallback)?;
    let preference = planner
        .preferences
        .get(&RegionKey {
            entry,
            continuation: None,
            loop_header: None,
        })
        .copied()
        .unwrap_or_default();
    let mut metrics = planner.metrics[root.0];
    metrics.entry_values = count(planner.block(entry)?.parameters.len());
    if !planner.owner.pay(64) {
        return None;
    }
    planner.regions.push(Region {
        entry,
        continuation: None,
        loop_header: None,
        body: root,
        decision: Some(RegionDecision {
            region: RegionAnchor {
                source: planner.source,
                entry,
                continuation: None,
                loop_header: None,
            },
            preference,
            selected: StructureForm::Dispatcher,
            reason: ReadabilityReason::ValidationFallback,
            natural: None,
            dispatcher: Some(metrics),
        }),
    });
    Some(Plan {
        nodes: planner.nodes,
        root,
        regions: planner.regions,
    })
}

fn build_with_slot_limit(
    ir: &Ir,
    facts: &Facts,
    owner: &mut Builder<impl FnMut(Progress)>,
    slot_limit: usize,
) -> Option<Plan> {
    let mut p = Planner::new(ir, facts, owner, slot_limit)?;
    let root = if let Some(entry) = ir.entry {
        match p.build(entry, None, None, 0, &BTreeSet::new()) {
            Some(root) => root,
            None => {
                if p.capacity_refused && !p.owner.stopped {
                    p.owner.stop(Completion::Limited,DiagnosticCode::Limit,"compact source region exceeds temporary plan storage admission; prior validated CFG retained");
                }
                return None;
            }
        }
    } else {
        p.sequence(Vec::new())?
    };
    Some(Plan {
        nodes: p.nodes,
        root,
        regions: p.regions,
    })
}
impl<'a, P: FnMut(Progress)> Planner<'a, P> {
    fn new(
        ir: &'a Ir,
        facts: &'a Facts,
        owner: &'a mut Builder<P>,
        slot_limit: usize,
    ) -> Option<Self> {
        if !owner.pay(
            owner
                .artifact
                .input
                .runtime_bytecode
                .len()
                .saturating_mul(4)
                .saturating_add(
                    owner
                        .artifact
                        .config
                        .readability
                        .regions
                        .len()
                        .saturating_mul(128),
                )
                .saturating_add(1024),
        ) {
            return None;
        }
        let source = crate::readability_source(&owner.artifact.input, &owner.artifact.config);
        let mut preferences = BTreeMap::new();
        for preference in &owner.artifact.config.readability.regions {
            if preference.region.source == source {
                let key = RegionKey {
                    entry: preference.region.entry,
                    continuation: preference.region.continuation,
                    loop_header: preference.region.loop_header,
                };
                preferences
                    .entry(key)
                    .and_modify(|current| {
                        if *current != preference.structure {
                            *current = StructurePreference::Auto;
                        }
                    })
                    .or_insert(preference.structure);
            }
        }
        let natural_limit = ir
            .blocks
            .len()
            .saturating_add(ir.edges.len())
            .saturating_mul(PLAN_ARENA_RATIO)
            .max(NATURAL_OCCURRENCE_FLOOR);
        Some(Self {
            owner,
            ir,
            facts,
            loops: &facts.loops,
            postdom: &facts.postdom,
            nodes: Vec::new(),
            quotes: Vec::new(),
            regions: Vec::new(),
            cache: BTreeMap::new(),
            cache_log: Vec::new(),
            arena_slots: 0,
            slot_limit,
            capacity_refused: false,
            join_cache: BTreeMap::new(),
            cycle_cache: BTreeMap::new(),
            normal_region: None,
            memo_limit: natural_limit.saturating_mul(2).min(MAX_PLAN_SLOTS / 8),
            compact_pressure: false,
            expanded_sources: BTreeSet::new(),
            natural_attempts: 0,
            natural_limit,
            metrics: Vec::new(),
            dispatch_reasons: Vec::new(),
            failed_operations: Vec::new(),
            preferred: Vec::new(),
            source,
            preferences,
        })
    }
    fn checkpoint(&self) -> Checkpoint {
        Checkpoint {
            nodes: self.nodes.len(),
            regions: self.regions.len(),
            cache: self.cache_log.len(),
            slots: self.arena_slots,
        }
    }
    fn rollback(&mut self, checkpoint: Checkpoint) {
        self.nodes.truncate(checkpoint.nodes);
        self.quotes.truncate(checkpoint.nodes);
        self.metrics.truncate(checkpoint.nodes);
        self.dispatch_reasons.truncate(checkpoint.nodes);
        self.failed_operations.truncate(checkpoint.nodes);
        self.preferred.truncate(checkpoint.nodes);
        self.regions.truncate(checkpoint.regions);
        while self.cache_log.len() > checkpoint.cache {
            self.cache.remove(&self.cache_log.pop().unwrap());
        }
        self.arena_slots = checkpoint.slots;
    }
    fn fits(&mut self, slots: usize) -> bool {
        if slots > self.slot_limit.saturating_sub(self.arena_slots) {
            self.capacity_refused = true;
            self.compact_pressure = true;
            false
        } else {
            true
        }
    }
    fn node(&mut self, kind: StructuredKind<PlanId>, mut sources: Vec<EntityId>) -> Option<PlanId> {
        if self.nodes.len() >= crate::MAX_STRUCTURED_WIRE_NODES {
            self.capacity_refused = true;
            self.compact_pressure = true;
            return None;
        }
        let child_slots = match &kind {
            StructuredKind::Sequence(v) => v.len(),
            StructuredKind::If { .. } | StructuredKind::While { .. } => 2,
            StructuredKind::Indirect { arms, .. } => arms.len().saturating_mul(2),
            StructuredKind::Dispatcher { cases, .. } => cases.iter().fold(cases.len(), |n, c| {
                n.saturating_add(c.transitions.len().saturating_mul(3))
            }),
            _ => 0,
        };
        let extra_sources = match &kind {
            StructuredKind::Sequence(v) => v.iter().fold(0_usize, |n, id| {
                n.saturating_add(self.nodes[id.0].sources.len())
            }),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => self.nodes[then_branch.0]
                .sources
                .len()
                .saturating_add(self.nodes[else_branch.0].sources.len()),
            StructuredKind::While { header, body, .. } => self.nodes[header.0]
                .sources
                .len()
                .saturating_add(self.nodes[body.0].sources.len()),
            StructuredKind::Indirect { arms, .. } => arms.iter().fold(0_usize, |size, arm| {
                size.saturating_add(self.nodes[arm.body.0].sources.len())
            }),
            _ => 0,
        };
        // Bound the temporary union before allocating it. A local refusal is
        // retried by an enclosing region, never by reviving a stopped owner.
        if !self.fits(
            sources
                .len()
                .saturating_add(extra_sources)
                .saturating_add(child_slots),
        ) {
            return None;
        }
        let failure_direction = self.failure_direction(&kind, &sources);
        let children: Vec<_> = match &kind {
            StructuredKind::Sequence(v) => v.clone(),
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => vec![**then_branch, **else_branch],
            StructuredKind::While { header, body, .. } => vec![**header, **body],
            StructuredKind::Indirect { arms, .. } => arms.iter().map(|arm| arm.body).collect(),
            _ => Vec::new(),
        };
        let size = children.iter().fold(sources.len(), |n, id| {
            n.saturating_add(self.nodes[id.0].sources.len())
        });
        if !self.owner.pay(
            size.saturating_mul(size.max(1).ilog2() as usize + 2)
                .saturating_add(8),
        ) {
            return None;
        }
        let preferred = children.iter().any(|id| self.preferred[id.0]);
        for id in children {
            sources.extend_from_slice(&self.nodes[id.0].sources);
        }
        sources.sort_unstable();
        sources.dedup();
        let sources = sources.into_boxed_slice().into_vec();
        self.arena_slots = self
            .arena_slots
            .saturating_add(sources.len())
            .saturating_add(child_slots);
        let node = PlanNode { kind, sources };
        let quote = quote_node(&node, &self.nodes, &self.quotes);
        let (metrics, failed_operations) = self.node_metrics(&node, quote, failure_direction);
        let id = PlanId(self.nodes.len());
        self.nodes.push(node);
        self.quotes.push(quote);
        self.metrics.push(metrics);
        self.dispatch_reasons.push(None);
        self.failed_operations.push(failed_operations);
        self.preferred.push(preferred);
        Some(id)
    }
    fn sequence(&mut self, nodes: Vec<PlanId>) -> Option<PlanId> {
        if nodes.len() == 1 {
            return Some(nodes[0]);
        }
        self.node(StructuredKind::Sequence(nodes), Vec::new())
    }
    fn block(&self, id: EntityId) -> Option<&crate::Block> {
        self.facts.blocks.get(&id).map(|n| &self.ir.blocks[*n])
    }
    fn edge(&self, id: EntityId) -> Option<&crate::Edge> {
        self.facts.edges.get(&id).map(|n| &self.ir.edges[*n])
    }
    fn reachable(&mut self, start: EntityId, stop: Option<EntityId>) -> Option<BTreeSet<EntityId>> {
        if !self.owner.pay(
            self.ir
                .blocks
                .len()
                .saturating_add(self.ir.edges.len())
                .saturating_mul(4),
        ) {
            return None;
        }
        let mut found = BTreeSet::new();
        let mut todo = vec![start];
        while let Some(id) = todo.pop() {
            if Some(id) == stop || !found.insert(id) {
                continue;
            }
            let index = *self.facts.blocks.get(&id)?;
            todo.extend(
                self.facts.successors[index]
                    .iter()
                    .map(|to| self.ir.blocks[*to].id),
            );
        }
        Some(found)
    }
    fn admit(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        body: PlanId,
        preference: StructurePreference,
    ) -> Option<Admission> {
        let mut natural = self.metrics[body.0];
        natural.entry_values = count(self.block(start)?.parameters.len());
        if matches!(self.nodes[body.0].kind, StructuredKind::Dispatcher { .. }) {
            return Some(Admission::Keep(
                body,
                Assessment {
                    natural: None,
                    dispatcher: Some(natural),
                    reason: self.dispatch_reasons[body.0]?,
                },
            ));
        }
        let quote = self.quotes[body.0];
        let excessive_depth = quote.depth > crate::MAX_STRUCTURED_WIRE_DEPTH;
        let hard = excessive_depth
            || quote.nodes > crate::MAX_STRUCTURED_WIRE_NODES
            || quote.references > MAX_SOURCE_REFERENCES;
        let mut assessment = Assessment {
            natural: Some(natural),
            dispatcher: None,
            reason: if preference == StructurePreference::Natural {
                ReadabilityReason::UserPreference
            } else {
                ReadabilityReason::NaturalControl
            },
        };
        if self.owner.artifact.config.strategy != Strategy::Dispatcher {
            if hard {
                self.owner.stop(Completion::Limited,DiagnosticCode::Limit,"structured occurrence/depth admission requires a dispatcher; selected strategy retains prior CFG");
                return None;
            }
            if preference == StructurePreference::Dispatcher {
                assessment.reason = ReadabilityReason::PreferenceFallback;
            }
            return Some(Admission::Keep(body, assessment));
        }
        let profile = self.owner.artifact.config.readability.profile;
        if !self
            .owner
            .pay(self.nodes[body.0].sources.len().saturating_mul(8))
        {
            return None;
        }
        let reading = self.comparison_metrics(body, natural);
        let compare = preference == StructurePreference::Dispatcher
            || (preference != StructurePreference::Natural
                && !self.preferred[body.0]
                && readability::compare(profile, reading));
        let check_expansion = quote.nodes >= NATURAL_OCCURRENCE_FLOOR
            && preference != StructurePreference::Natural
            && !self.preferred[body.0];
        if !hard && !compare && !check_expansion {
            return Some(Admission::Keep(body, assessment));
        }
        let mut region = self.reachable(start, stop)?;
        if let Some(lp) = lp {
            region.retain(|id| lp.nodes.contains(id) && *id != lp.header);
            region.insert(start);
        }
        // This is an exact quote of the concrete compact candidate's source
        // cases, not an estimate based on the plan arena's unique-node count.
        let compact = self.dispatch_metrics(start, &region)?;
        let compact_reading = self.dispatch_comparison_metrics(&region, compact);
        let excessive_expansion = check_expansion
            && count(quote.nodes)
                > compact
                    .dispatcher_blocks
                    .saturating_add(compact.dispatcher_transitions)
                    .saturating_add(1)
                    .saturating_mul(REGION_EXPANSION_RATIO as u64);
        assessment.dispatcher = Some(compact);
        if hard
            || excessive_expansion
            || preference == StructurePreference::Dispatcher
            || readability::prefers_compact(profile, reading, compact_reading)
        {
            assessment.reason = if excessive_depth {
                ReadabilityReason::DepthFallback
            } else if hard || excessive_expansion {
                ReadabilityReason::Expansion
            } else if preference == StructurePreference::Dispatcher {
                ReadabilityReason::UserPreference
            } else {
                ReadabilityReason::LowerReadingCost
            };
            // A local readability choice does not force unrelated regions into
            // dispatch. Only cumulative search/storage pressure is sticky.
            if excessive_expansion {
                if !self.owner.pay(region.len().saturating_mul(32)) {
                    return None;
                }
                self.expanded_sources.extend(region.iter().copied());
            }
            return Some(Admission::Compact(region, assessment));
        }
        Some(Admission::Keep(body, assessment))
    }
    fn build(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<PlanId> {
        if !self.owner.pay(8) {
            return None;
        }
        let key = RegionKey {
            entry: start,
            continuation: stop,
            loop_header: lp.map(|l| l.header),
        };
        // Ordinary plans are cached only for finite path-independent regions;
        // a complete source dispatcher is also path-independent for open/cyclic
        // regions. No cached PlanId survives the checkpoint that owns it.
        if let Some(id) = self.cache.get(&key) {
            return Some(*id);
        }
        let preference = self.preferences.get(&key).copied().unwrap_or_default();
        let checkpoint = self.checkpoint();
        let proposed = if preference == StructurePreference::Dispatcher
            && self.owner.artifact.config.strategy == Strategy::Dispatcher
            && Some(start) != stop
        {
            self.dispatch(start, stop, lp, ReadabilityReason::UserPreference)
                .map(|body| {
                    let mut metrics = self.metrics[body.0];
                    metrics.entry_values =
                        count(self.block(start).map_or(0, |block| block.parameters.len()));
                    Admission::Keep(
                        body,
                        Assessment {
                            natural: None,
                            dispatcher: Some(metrics),
                            reason: ReadabilityReason::UserPreference,
                        },
                    )
                })
        } else {
            self.build_uncached(start, stop, lp, depth, path)
                .and_then(|body| self.admit(start, stop, lp, body, preference))
        };
        let (body, assessment) = match proposed {
            Some(Admission::Keep(body, assessment)) => (body, assessment),
            Some(Admission::Compact(region, assessment)) => {
                self.rollback(checkpoint);
                self.capacity_refused = false;
                (
                    self.dispatch_region(start, region, assessment.reason)?,
                    assessment,
                )
            }
            None if self.capacity_refused && !self.owner.stopped => {
                self.rollback(checkpoint);
                self.capacity_refused = false;
                if self.owner.artifact.config.strategy != Strategy::Dispatcher {
                    self.owner.stop(Completion::Limited,DiagnosticCode::Limit,"temporary region plan admission requires a dispatcher; selected strategy retains prior CFG");
                    return None;
                }
                let body = self.dispatch(start, stop, lp, ReadabilityReason::PlanningFallback)?;
                let mut metrics = self.metrics[body.0];
                metrics.entry_values = count(self.block(start)?.parameters.len());
                (
                    body,
                    Assessment {
                        natural: None,
                        dispatcher: Some(metrics),
                        reason: ReadabilityReason::PlanningFallback,
                    },
                )
            }
            None => return None,
        };
        let selected = if matches!(self.nodes[body.0].kind, StructuredKind::Dispatcher { .. }) {
            StructureForm::Dispatcher
        } else {
            StructureForm::Natural
        };
        // Automatic enclosing compaction must not erase an admitted local user
        // choice. Explicit enclosing choices and hard resource fallback retain
        // their own precedence and report any superseded boundary afterwards.
        self.preferred[body.0] |= preference != StructurePreference::Auto;
        let interesting = self.ir.entry == Some(start) && stop.is_none() && lp.is_none()
            || preference != StructurePreference::Auto
            || selected == StructureForm::Dispatcher
            || assessment.dispatcher.is_some()
            || assessment
                .natural
                .is_some_and(|metrics| metrics.conditions > 0 && metrics.max_nesting > 1);
        let decision = if interesting {
            if !self.owner.pay(64) {
                return None;
            }
            Some(RegionDecision {
                region: RegionAnchor {
                    source: self.source.clone(),
                    entry: start,
                    continuation: stop,
                    loop_header: lp.map(|l| l.header),
                },
                preference,
                selected,
                reason: assessment.reason,
                natural: assessment.natural,
                dispatcher: assessment.dispatcher,
            })
        } else {
            None
        };
        self.regions.push(Region {
            entry: start,
            continuation: stop,
            loop_header: lp.map(|l| l.header),
            body,
            decision,
        });
        if self.facts.finite.contains(&start)
            || matches!(self.nodes[body.0].kind, StructuredKind::Dispatcher { .. })
        {
            self.cache.insert(key, body);
            self.cache_log.push(key);
        }
        Some(body)
    }
    fn build_uncached(
        &mut self,
        start: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<PlanId> {
        if Some(start) == stop {
            return self.sequence(Vec::new());
        }
        if self.expanded_sources.contains(&start)
            && self.owner.artifact.config.strategy == Strategy::Dispatcher
            && self.preferences.get(&RegionKey {
                entry: start,
                continuation: stop,
                loop_header: lp.map(|loop_| loop_.header),
            }) != Some(&StructurePreference::Natural)
        {
            return self.dispatch(start, stop, lp, ReadabilityReason::Expansion);
        }
        if self.compact_pressure || self.natural_attempts >= self.natural_limit {
            if self.owner.artifact.config.strategy == Strategy::Dispatcher {
                self.compact_pressure = true;
                return self.dispatch(start, stop, lp, ReadabilityReason::PlanningFallback);
            }
            self.owner.stop(Completion::Limited,DiagnosticCode::Limit,"cumulative natural region attempt admission requires a dispatcher; selected strategy retains prior CFG");
            return None;
        }
        self.natural_attempts += 1;
        if count(depth) > self.owner.limits().max_structuring_depth {
            if self.owner.artifact.config.strategy == Strategy::Dispatcher {
                return self.dispatch(start, stop, lp, ReadabilityReason::DepthFallback);
            }
            self.owner.stop(Completion::Limited,DiagnosticCode::Limit,"structuring depth admission requires a dispatcher; selected strategy retains prior CFG");
            return None;
        }
        if path.contains(&start) {
            return self.dispatch(start, stop, lp, ReadabilityReason::ControlReentry);
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
    ) -> Option<PlanId> {
        if !self.owner.pay(
            self.ir
                .blocks
                .len()
                .saturating_mul(self.ir.edges.len().saturating_add(1)),
        ) {
            return None;
        }
        let block = self.block(start)?.clone();
        let mut visited = path.clone();
        visited.insert(start);
        // A residual cycle must be entered as a state machine before emitting
        // its first body; otherwise the first case's effects would execute twice.
        if self.residual_cycle(start, lp).ok()? {
            // Keep a proved common continuation outside a residual cycle's
            // dispatcher. Without such a cut the original enclosing boundary
            // remains the safe fallback; this does not promise a minimum SCC.
            if lp.is_none()
                && stop.is_none()
                && let Some(follow) =
                    self.facts
                        .residual_continuation(self.ir, start, &mut |work| self.owner.pay(work))?
            {
                let compact = self.build(start, Some(follow), None, depth + 1, path)?;
                let suffix = self.build(follow, stop, None, depth + 1, path)?;
                return self.sequence(vec![compact, suffix]);
            }
            return self.dispatch(start, stop, lp, ReadabilityReason::ResidualCycle);
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
                    self.indirect(start, (&edges, unresolved), stop, lp, depth + 1, &visited)?
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
                    if let Some(other) = else_edge {
                        self.common_join(
                            self.edge(then_edges[0])?.to,
                            self.edge(other)?.to,
                            stop,
                            lp,
                        )
                        .ok()?
                    } else {
                        None
                    }
                } else {
                    None
                };
                // A terminal true arm lets the false continuation remain a
                // following sequence. This flattens compiler selector chains
                // without changing the order or nesting of semantic decisions.
                if join.is_none() && stop.is_none() && lp.is_none() {
                    let then_branch = if simple {
                        self.edge_path(then_edges[0], None, None, depth + 1, &visited)?
                    } else {
                        self.indirect(
                            start,
                            (&then_edges, unresolved),
                            None,
                            None,
                            depth + 1,
                            &visited,
                        )?
                    };
                    let else_branch = if let Some(edge) = else_edge {
                        self.node(StructuredKind::Transfer { edge }, vec![edge])?
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
                    let tail = if let Some(edge) = else_edge {
                        let next = self.edge(edge)?.to;
                        let follow = self.build(next, None, None, depth + 1, &visited)?;
                        self.sequence(vec![branch, follow])?
                    } else {
                        branch
                    };
                    return self.sequence(vec![body, tail]);
                }
                let arm_stop = join.or(stop);
                let then_branch = if simple {
                    self.edge_path(then_edges[0], arm_stop, lp, depth + 1, &visited)?
                } else {
                    self.indirect(
                        start,
                        (&then_edges, unresolved),
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
    ) -> Option<PlanId> {
        let edge = self.edge(id)?.clone();
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
        jump: (&[EntityId], bool),
        stop: Option<EntityId>,
        lp: Option<&Loop>,
        depth: usize,
        path: &BTreeSet<EntityId>,
    ) -> Option<PlanId> {
        let (edges, unresolved) = jump;
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
            let target = self.block(self.edge(*id)?.to)?.source?.pc;
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
    ) -> Option<PlanId> {
        let block = self.block(info.header)?.clone();
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
            && then_edges.len() == 1
            && invalid_targets.is_empty()
        {
            let first = self.edge(then_edges[0])?;
            let second = self.edge(*other)?;
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
        reason: ReadabilityReason,
    ) -> Option<PlanId> {
        let mut region = self.reachable(start, stop)?;
        if let Some(lp) = lp {
            region.retain(|id| lp.nodes.contains(id) && *id != lp.header);
            region.insert(start);
        }
        self.dispatch_region(start, region, reason)
    }
    fn dispatch_region(
        &mut self,
        start: EntityId,
        region: BTreeSet<EntityId>,
        reason: ReadabilityReason,
    ) -> Option<PlanId> {
        let transitions = region
            .iter()
            .filter_map(|id| self.block(*id))
            .map(|b| b.terminator.edges().len())
            .sum::<usize>();
        if !self.fits(
            region
                .len()
                .saturating_mul(2)
                .saturating_add(transitions.saturating_mul(3)),
        ) {
            return None;
        }
        let mut cases = Vec::new();
        for id in &region {
            let b = self.block(*id)?.clone();
            let mut transitions = Vec::new();
            for edge in b.terminator.edges() {
                if !self.owner.pay(1) {
                    return None;
                }
                let target = self.edge(edge)?.to;
                let id = EntityId(0);
                transitions.push(DispatchTransition { id, edge, target });
            }
            cases.push(DispatchCase {
                block: *id,
                transitions,
            });
        }
        let body = self.node(
            StructuredKind::Dispatcher {
                entry: start,
                cases,
            },
            region.into_iter().collect(),
        )?;
        self.dispatch_reasons[body.0] = Some(reason);
        Some(body)
    }
    fn cache_room(&mut self) -> bool {
        if self.join_cache.len().saturating_add(self.cycle_cache.len()) < self.memo_limit {
            true
        } else {
            self.compact_pressure = true;
            false
        }
    }
    fn residual_cycle(&mut self, start: EntityId, lp: Option<&Loop>) -> Result<bool, PlanError> {
        if !self.owner.pay(8) {
            return Err(PlanError::Limit);
        }
        let key = CycleKey {
            entry: start,
            loop_header: lp.map(|l| l.header),
        };
        if let Some(answer) = self.cycle_cache.get(&key) {
            return Ok(*answer);
        }
        let answer = self
            .facts
            .indexed_residual_cycle(self.ir, start, key.loop_header, &mut |n| self.owner.pay(n))
            .ok_or(PlanError::Limit)?;
        if self.cache_room() {
            self.cycle_cache.insert(key, answer);
        }
        Ok(answer)
    }
    fn common_join(
        &mut self,
        a: EntityId,
        b: EntityId,
        stop: Option<EntityId>,
        lp: Option<&Loop>,
    ) -> Result<Option<EntityId>, PlanError> {
        if !self.owner.pay(8) {
            return Err(PlanError::Limit);
        }
        let key = JoinKey {
            left: a,
            right: b,
            continuation: stop,
            loop_header: lp.map(|l| l.header),
        };
        if let Some(answer) = self.join_cache.get(&key) {
            return Ok(*answer);
        }
        let allowed = |id: &&EntityId| id.0 != 0 && lp.is_none_or(|l| l.nodes.contains(id));
        let strict: BTreeSet<_> = self.postdom[&a]
            .intersection(&self.postdom[&b])
            .filter(allowed)
            .copied()
            .collect();
        let mut normal: BTreeSet<_> = if self.facts.finite.contains(&a)
            && self.facts.finite.contains(&b)
            && !self.facts.failed.contains(&a)
            && !self.facts.failed.contains(&b)
        {
            self.facts.normal_postdom[&a]
                .intersection(&self.facts.normal_postdom[&b])
                .filter(allowed)
                .copied()
                .collect()
        } else {
            BTreeSet::new()
        };
        if strict.is_empty()
            && normal.is_empty()
            && (stop.is_some() || !self.facts.loops.is_empty())
        {
            let scope = RegionScope {
                continuation: stop,
                loop_header: lp.map(|info| info.header),
            };
            if self.normal_region.as_ref().map(|(key, _)| *key) != Some(scope) {
                // The facts are source-only. Dropping an old scope before
                // constructing its replacement bounds retained source storage
                // independently of recursive plan depth and arena rollback.
                self.normal_region = None;
                let facts = self
                    .facts
                    .normal_region(self.ir, scope, &mut |n| self.owner.pay(n))
                    .ok_or(PlanError::Limit)?;
                self.normal_region = Some((scope, facts));
            }
            let facts = &self.normal_region.as_ref().unwrap().1.postdom;
            if let (Some(left), Some(right)) = (facts.get(&a), facts.get(&b)) {
                if !self
                    .owner
                    .pay(left.len().saturating_add(right.len()).saturating_mul(8))
                {
                    return Err(PlanError::Limit);
                }
                normal = left.intersection(right).filter(allowed).copied().collect();
            }
        }
        // Most early-terminal pairs have no candidate at all. In that case no
        // distance arrays or graph walks are necessary; cache the negative fact.
        let answer = if strict.is_empty() && normal.is_empty() {
            None
        } else {
            let da = self
                .facts
                .indexed_distances(a, &mut |n| self.owner.pay(n))
                .ok_or(PlanError::Limit)?;
            let db = self
                .facts
                .indexed_distances(b, &mut |n| self.owner.pay(n))
                .ok_or(PlanError::Limit)?;
            let select = |set: BTreeSet<EntityId>| {
                set.into_iter()
                    .filter_map(|id| {
                        let n = self.facts.blocks[&id];
                        Some((da[n]?.max(db[n]?), id))
                    })
                    .min()
                    .map(|(_, id)| id)
            };
            select(strict).or_else(|| select(normal))
        };
        if self.cache_room() {
            self.join_cache.insert(key, answer);
        }
        Ok(answer)
    }
}

#[cfg(test)]
mod tests;

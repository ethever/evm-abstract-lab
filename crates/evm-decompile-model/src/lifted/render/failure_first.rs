//! Source-bound early failure projection. The plan moves whole owned source
//! occurrences; it grants no permission to erase primitives or payload builders.
use super::{LiftedRenderError as Error, LiftedRenderIndex};
use crate::{
    EntityId, ExitKind, Ir, LiftedControl, LiftedType, StructuredKind as K, StructuredNode,
    Terminator as T,
};
use std::collections::{BTreeMap, BTreeSet};

/// An admitted explicit failed arm followed by the original successful arm.
/// Identity and suffix ownership are private so consumers cannot forge cuts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureFirstIf {
    source: EntityId,
    failure_node: EntityId,
    success_node: EntityId,
    failure_nonzero: bool,
    pub(super) suffix: Option<(EntityId, usize, usize)>,
}
impl FailureFirstIf {
    /// Original source If occurrence.
    pub fn source(&self) -> EntityId {
        self.source
    }
    /// Original complete failed arm, including its edge assignments.
    pub fn failure_node(&self) -> EntityId {
        self.failure_node
    }
    /// Original successful arm emitted after the explicit failed If.
    pub fn success_node(&self) -> EntityId {
        self.success_node
    }
    /// Whether the original nonzero condition selects failure.
    pub fn failure_nonzero(&self) -> bool {
        self.failure_nonzero
    }
}

#[derive(Default)]
pub(super) struct FailureFirstIndex {
    plans: BTreeMap<EntityId, FailureFirstIf>,
    directions: BTreeMap<(EntityId, bool), FailureFirstIf>,
    ranges: BTreeMap<EntityId, (usize, usize)>,
    pub(super) cuts: BTreeMap<(EntityId, usize), EntityId>,
    calls: BTreeSet<EntityId>,
    pub(super) require_scopes: BTreeMap<EntityId, EntityId>,
}

const FALL: u8 = 1;
const FAILURE: u8 = 2;
const NORMAL: u8 = 4;
const ESCAPE: u8 = 8;
const UNKNOWN: u8 = 16;
// An unresolved successor may enter code outside the represented source route.
// A later lexical terminal does not close that successor.
const OPEN: u8 = 32;

#[derive(Clone, Copy)]
struct Summary {
    outcomes: u8,
    entry: Option<EntityId>,
    end: Option<EntityId>,
    first_edge: Option<EntityId>,
    empty: bool,
    body_at_end: bool,
    needs_body: bool,
}
impl Summary {
    const EMPTY: Self = Self {
        outcomes: FALL,
        entry: None,
        end: None,
        first_edge: None,
        empty: true,
        body_at_end: false,
        needs_body: false,
    };
    const UNKNOWN: Self = Self {
        outcomes: FALL | UNKNOWN | OPEN,
        empty: false,
        ..Self::EMPTY
    };
    const OPAQUE: Self = Self {
        outcomes: FALL | UNKNOWN,
        empty: false,
        ..Self::EMPTY
    };
    fn falls(self) -> bool {
        self.outcomes & FALL != 0
    }
    fn closed_failure(self) -> bool {
        self.outcomes == FAILURE && !self.needs_body
    }
    fn then(self, right: Self) -> Self {
        if self.empty {
            return right;
        }
        if right.empty || !self.falls() {
            return self;
        }
        let lexical_escape = right.entry.is_none() && right.outcomes == ESCAPE;
        let connected = lexical_escape || (self.end.is_some() && self.end == right.entry);
        let matched_body = connected && self.body_at_end;
        let uncertain = !connected || (right.needs_body && !matched_body);
        let open = (right.needs_body && !matched_body)
            || (!connected && (self.outcomes | right.outcomes) & UNKNOWN == 0);
        Self {
            // Uncertain earlier control may loop or escape instead of reaching
            // `right`. If it does return normally, `right` still executes, so a
            // later STOP/RETURN proves no normal fallthrough of the sequence.
            // UNKNOWN remains set and therefore never proves finite failure.
            outcomes: (self.outcomes & !FALL)
                | right.outcomes
                | if uncertain { UNKNOWN } else { 0 }
                | if open { OPEN } else { 0 },
            entry: self.entry,
            end: right.end,
            first_edge: self.first_edge,
            empty: false,
            body_at_end: right.body_at_end,
            needs_body: self.needs_body || (right.needs_body && !matched_body),
        }
    }
}

fn paid(pay: &mut impl FnMut(usize) -> bool, n: usize) -> Result<(), Error> {
    if pay(n) { Ok(()) } else { Err(Error::Limit) }
}

impl FailureFirstIndex {
    /// One bounded iterative tree walk and bottom-up pass. Local suffix facts
    /// are indexed once, never copied transitively into individual If plans.
    pub(super) fn new(
        index: &LiftedRenderIndex<'_>,
        ir: &Ir,
        pay: &mut impl FnMut(usize) -> bool,
    ) -> Result<Self, Error> {
        let program = index.program;
        let nodes = &index.nodes;
        let expressions = &index.expressions;
        let high_blocks = &index.blocks;
        let edges = &index.edges;
        let mut result = Self::default();
        let Some(root) = ir.structured.as_ref().map(|s| &s.root) else {
            return Ok(result);
        };
        paid(
            pay,
            ir.blocks
                .len()
                .saturating_add(program.controls.len())
                .saturating_mul(4),
        )?;
        let raw: BTreeMap<_, _> = ir.blocks.iter().map(|b| (b.id, b)).collect();
        let controls = &index.controls;
        if raw.len() != ir.blocks.len() || controls.len() != program.controls.len() {
            return Ok(result);
        }
        let mut order = Vec::new();
        let mut positions = BTreeMap::new();
        let mut follows = BTreeMap::new();
        enum Visit<'a> {
            Enter(&'a StructuredNode, usize, Option<(EntityId, usize)>),
            Leave(EntityId, usize),
        }
        let mut pending = vec![Visit::Enter(root, 1, None)];
        while let Some(visit) = pending.pop() {
            paid(pay, 4)?;
            match visit {
                Visit::Leave(id, start) => {
                    result.ranges.insert(id, (start, order.len()));
                }
                Visit::Enter(node, depth, follow) => {
                    if depth > crate::MAX_STRUCTURED_WIRE_DEPTH
                        || order.len() >= crate::MAX_STRUCTURED_WIRE_NODES
                        || !nodes.get(&node.id).is_some_and(|n| std::ptr::eq(*n, node))
                    {
                        return Ok(Self::default());
                    }
                    let start = order.len();
                    if let Some(follow) = follow {
                        follows.insert(node.id, follow);
                    }
                    order.push(node);
                    pending.push(Visit::Leave(node.id, start));
                    match &node.kind {
                        K::Sequence(v) => {
                            paid(pay, v.len().saturating_mul(4))?;
                            for (position, child) in v.iter().enumerate().rev() {
                                positions.insert(child.id, (node.id, position));
                                let next = if position + 1 < v.len() {
                                    Some((node.id, position + 1))
                                } else {
                                    follow
                                };
                                pending.push(Visit::Enter(child, depth + 1, next));
                            }
                        }
                        K::If {
                            then_branch,
                            else_branch,
                            ..
                        } => {
                            pending.push(Visit::Enter(else_branch, depth + 1, follow));
                            pending.push(Visit::Enter(then_branch, depth + 1, follow));
                        }
                        K::While { header, body, .. } => {
                            pending.push(Visit::Enter(body, depth + 1, None));
                            pending.push(Visit::Enter(header, depth + 1, None));
                        }
                        K::Indirect { arms, .. } => {
                            paid(pay, arms.len().saturating_mul(4))?;
                            pending.extend(
                                arms.iter()
                                    .rev()
                                    .map(|a| Visit::Enter(&a.body, depth + 1, follow)),
                            );
                        }
                        _ => {}
                    }
                }
            }
        }
        let branch = |id, condition, left: Summary, right: Summary| -> Option<EntityId> {
            let control: &&LiftedControl = controls.get(&id)?;
            let expression = expressions.get(&control.condition)?;
            let source = raw.get(&expression.block)?;
            let T::Branch {
                condition: raw_condition,
                then_edges,
                else_edge: Some(other),
                unresolved: false,
                invalid_targets,
                ..
            } = &source.terminator
            else {
                return None;
            };
            if expression.ty != LiftedType::Predicate
                || *raw_condition != condition
                || then_edges.len() != 1
                || !invalid_targets.is_empty()
                || high_blocks.get(&source.id)?.condition != Some(control.condition)
            {
                return None;
            }
            for (summary, expected, kind) in [
                (left, then_edges[0], crate::EdgeKind::True),
                (right, *other, crate::EdgeKind::False),
            ] {
                let edge = edges.get(&expected)?;
                if edge.from != source.id
                    || edge.kind != kind
                    || !raw.contains_key(&edge.to)
                    || (!summary.empty && summary.first_edge != Some(expected))
                    || (summary.empty && !edge.arguments.is_empty())
                {
                    return None;
                }
            }
            Some(source.id)
        };
        let mut summaries = BTreeMap::<EntityId, Summary>::new();
        let mut suffixes = BTreeMap::new();
        for node in order.iter().rev() {
            paid(pay, 8)?;
            let summary = match &node.kind {
                K::Sequence(v) => {
                    paid(pay, v.len().saturating_mul(4))?;
                    let mut suffix = Summary::EMPTY;
                    suffixes.insert((node.id, v.len()), suffix);
                    for (position, child) in v.iter().enumerate().rev() {
                        suffix = summaries[&child.id].then(suffix);
                        suffixes.insert((node.id, position), suffix);
                    }
                    suffix
                }
                K::Block { block } if raw.contains_key(block) => Summary {
                    outcomes: if raw[block].terminator.is_partial() {
                        FALL | UNKNOWN | OPEN
                    } else {
                        FALL
                    },
                    entry: Some(*block),
                    end: Some(*block),
                    empty: false,
                    body_at_end: true,
                    ..Summary::EMPTY
                },
                K::Transfer { edge } => edges
                    .get(edge)
                    .filter(|e| raw.contains_key(&e.from) && raw.contains_key(&e.to))
                    .map_or(Summary::UNKNOWN, |e| Summary {
                        outcomes: FALL,
                        entry: Some(e.from),
                        end: Some(e.to),
                        first_edge: Some(*edge),
                        empty: false,
                        body_at_end: false,
                        needs_body: false,
                    }),
                K::Exit { block } => match raw.get(block).map(|b| &b.terminator) {
                    Some(T::Exit { kind, .. }) => Summary {
                        outcomes: if *kind == ExitKind::Revert {
                            FAILURE
                        } else {
                            NORMAL
                        },
                        entry: Some(*block),
                        empty: false,
                        needs_body: true,
                        ..Summary::EMPTY
                    },
                    Some(T::Trap { .. }) => Summary {
                        outcomes: FAILURE,
                        entry: Some(*block),
                        empty: false,
                        needs_body: true,
                        ..Summary::EMPTY
                    },
                    _ => Summary::UNKNOWN,
                },
                K::If {
                    condition,
                    then_branch,
                    else_branch,
                } => {
                    let left = summaries[&then_branch.id];
                    let right = summaries[&else_branch.id];
                    branch(node.id, *condition, left, right)
                        .filter(|_| !left.needs_body && !right.needs_body)
                        .map_or(Summary::UNKNOWN, |entry| Summary {
                            outcomes: left.outcomes | right.outcomes,
                            entry: Some(entry),
                            end: match (left.falls(), right.falls()) {
                                (true, true) if left.end == right.end => left.end,
                                (true, false) => left.end,
                                (false, true) => right.end,
                                _ => None,
                            },
                            empty: false,
                            body_at_end: match (left.falls(), right.falls()) {
                                (true, true) => left.body_at_end && right.body_at_end,
                                (true, false) => left.body_at_end,
                                (false, true) => right.body_at_end,
                                _ => false,
                            },
                            ..Summary::EMPTY
                        })
                }
                K::Break | K::Continue => Summary {
                    outcomes: ESCAPE,
                    empty: false,
                    ..Summary::EMPTY
                },
                K::While { header, body, .. } => Summary {
                    outcomes: Summary::OPAQUE.outcomes
                        | ((summaries[&header.id].outcomes | summaries[&body.id].outcomes) & OPEN),
                    ..Summary::OPAQUE
                },
                K::Dispatcher { cases, .. } => {
                    paid(pay, cases.len().saturating_mul(4))?;
                    if cases.iter().any(|case| {
                        raw.get(&case.block)
                            .is_none_or(|b| b.terminator.is_partial())
                    }) {
                        Summary::UNKNOWN
                    } else {
                        Summary::OPAQUE
                    }
                }
                K::Indirect {
                    block,
                    arms,
                    unresolved,
                } => {
                    paid(pay, arms.len().saturating_mul(2))?;
                    if *unresolved
                        || arms.is_empty()
                        || raw.get(block).is_none_or(|b| b.terminator.is_partial())
                        || arms
                            .iter()
                            .any(|a| summaries[&a.body.id].outcomes & OPEN != 0)
                    {
                        Summary::UNKNOWN
                    } else {
                        Summary::OPAQUE
                    }
                }
                // Missing or partial source witnesses never close an unknown
                // successor, even if a later lexical node is a terminal.
                _ => Summary::UNKNOWN,
            };
            summaries.insert(node.id, summary);
        }
        // A full index may replace an If with an invocation. Base indexes also
        // exclude these roots conservatively without trusting their certificates.
        paid(pay, program.invocations.len().saturating_mul(2))?;
        result.calls = program.invocations.iter().map(|i| i.source).collect();
        // A compact guard must own the entire failed source route in the
        // rendered scope, including a suffix inherited through enclosing Ifs.
        // The ranked payload/effect certificate is admitted independently by
        // the selected control plan; these are only source closure facts.
        for control in &program.controls {
            paid(pay, 4)?;
            let Some(guard) = &control.guard else {
                continue;
            };
            let Some(&failed) = summaries.get(&guard.failure_node) else {
                continue;
            };
            let scope = if failed.closed_failure() {
                Some(control.node)
            } else {
                follows.get(&control.node).and_then(|&(sequence, start)| {
                    let tail = *suffixes.get(&(sequence, start))?;
                    failed.then(tail).closed_failure().then_some(sequence)
                })
            };
            if let Some(scope) = scope {
                result.require_scopes.insert(control.node, scope);
            }
        }
        for node in order {
            paid(pay, 8)?;
            let K::If {
                condition,
                then_branch,
                else_branch,
            } = &node.kind
            else {
                continue;
            };
            let left = summaries[&then_branch.id];
            let right = summaries[&else_branch.id];
            let Some(source_block) = branch(node.id, *condition, left, right) else {
                continue;
            };
            let mut chosen = None;
            for (failure_nonzero, failure, success, failed, passed) in [
                (true, then_branch, else_branch, left, right),
                (false, else_branch, then_branch, right, left),
            ] {
                let suffix = if failed.closed_failure() {
                    None
                } else {
                    if passed.falls() || passed.needs_body || passed.outcomes & OPEN != 0 {
                        continue;
                    }
                    let Some(&(sequence, position)) = positions.get(&node.id) else {
                        continue;
                    };
                    let K::Sequence(v) = &nodes[&sequence].kind else {
                        continue;
                    };
                    let start = position + 1;
                    let tail = suffixes[&(sequence, start)];
                    if start == v.len() || !failed.then(tail).closed_failure() {
                        continue;
                    }
                    if failed.empty {
                        let T::Branch {
                            then_edges,
                            else_edge,
                            ..
                        } = &raw[&source_block].terminator
                        else {
                            continue;
                        };
                        let edge = if failure_nonzero {
                            then_edges[0]
                        } else {
                            else_edge.unwrap()
                        };
                        if tail.entry != Some(edges[&edge].to) {
                            continue;
                        }
                    }
                    Some((sequence, start, v.len()))
                };
                let plan = FailureFirstIf {
                    source: node.id,
                    failure_node: failure.id,
                    success_node: success.id,
                    failure_nonzero,
                    suffix,
                };
                // Both arms can be finite failures while only one has the
                // compact guard's exact payload/effect certificate. Keep each
                // structural direction independent of the display preference.
                result
                    .directions
                    .insert((node.id, failure_nonzero), plan.clone());
                if chosen.is_none() {
                    chosen = Some(plan);
                }
            }
            if let Some(plan) = chosen {
                if let Some((sequence, start, _)) = plan.suffix {
                    result.cuts.insert((sequence, start), node.id);
                }
                result.plans.insert(node.id, plan);
            }
        }
        Ok(result)
    }
    pub(super) fn contains(&self, boundary: EntityId, source: EntityId) -> bool {
        self.ranges
            .get(&boundary)
            .zip(self.ranges.get(&source))
            .is_some_and(|((a, b), (c, d))| a <= c && d <= b)
    }
}

impl<'a> LiftedRenderIndex<'a> {
    pub(super) fn failure_first_direction(
        &self,
        source: EntityId,
        boundary: EntityId,
        failure_nonzero: bool,
    ) -> Option<&FailureFirstIf> {
        let index = self.failure_first.as_ref().ok()?;
        let plan = index.directions.get(&(source, failure_nonzero))?;
        (index.contains(boundary, source)
            && plan
                .suffix
                .is_none_or(|(sequence, _, _)| index.contains(boundary, sequence)))
        .then_some(plan)
    }
    /// Explicit failure-first decision within the exact rendered source root.
    /// A procedure cannot borrow a following suffix outside its canonical body.
    pub fn failure_first(&self, source: EntityId, boundary: EntityId) -> Option<&FailureFirstIf> {
        if self.failure_first.as_ref().ok()?.calls.contains(&source) {
            return None;
        }
        self.failure_first_expanded(source, boundary)
    }
    /// Canonical definitions expand source bodies instead of substituting
    /// invocation rows. They use the identical plan with that substitution off.
    pub(crate) fn failure_first_expanded(
        &self,
        source: EntityId,
        boundary: EntityId,
    ) -> Option<&FailureFirstIf> {
        let index = self.failure_first.as_ref().ok()?;
        let plan = index.plans.get(&source)?;
        (index.contains(boundary, source)
            && plan
                .suffix
                .is_none_or(|(sequence, _, _)| index.contains(boundary, sequence)))
        .then_some(plan)
    }
    /// Original contiguous source suffix; neither copied nor reconstructed.
    pub fn failure_first_suffix(&self, plan: &FailureFirstIf) -> Option<&'a [StructuredNode]> {
        if self.failure_first.as_ref().ok()?.plans.get(&plan.source) != Some(plan) {
            return None;
        }
        let (sequence, start, end) = plan.suffix?;
        let K::Sequence(v) = &self.nodes.get(&sequence)?.kind else {
            return None;
        };
        v.get(start..end)
    }
    /// Source Sequence and positions for traversing a relocated suffix with
    /// its own nested cuts, using the same boundary as the enclosing plan.
    pub fn failure_first_suffix_range(
        &self,
        plan: &FailureFirstIf,
    ) -> Option<(EntityId, std::ops::Range<usize>)> {
        if self.failure_first.as_ref().ok()?.plans.get(&plan.source) != Some(plan) {
            return None;
        }
        plan.suffix
            .map(|(sequence, start, end)| (sequence, start..end))
    }
    /// Skip only the owned suffix moved by the immediately preceding If plan.
    pub fn failure_first_sequence_end(
        &self,
        sequence: EntityId,
        position: usize,
        boundary: EntityId,
    ) -> Option<usize> {
        let source = *self
            .failure_first
            .as_ref()
            .ok()?
            .cuts
            .get(&(sequence, position))?;
        self.failure_first(source, boundary)?
            .suffix
            .map(|(_, _, end)| end)
    }
    /// Render the same typed predicate once at its original decision site.
    pub fn render_failure_first_condition(
        &self,
        plan: &FailureFirstIf,
        mut pay: impl FnMut(usize) -> bool,
    ) -> Result<String, Error> {
        if self
            .failure_first
            .as_ref()
            .ok()
            .and_then(|i| i.plans.get(&plan.source))
            != Some(plan)
        {
            return Err(Error::MissingWitness(plan.source));
        }
        let control = self
            .control(plan.source)
            .ok_or(Error::MissingWitness(plan.source))?;
        self.render_condition(control.condition, !plan.failure_nonzero, &mut pay)
    }
    /// Failed admission publishes no partial omission plan.
    pub fn validate_failure_first_sources(&self) -> Result<(), Error> {
        self.failure_first.as_ref().map(|_| ()).map_err(|e| *e)
    }
}

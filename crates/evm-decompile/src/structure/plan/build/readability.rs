//! Source-operation and control proxies for concrete region representations.
//! Correctness and retained-size admission precede preference. These weights
//! express reading priorities; they are not a cognitive or optimality claim.
use super::{PlanId, PlanNode, Planner, Quote};
use crate::{
    EntityId, Progress, ReadabilityMetrics, ReadabilityProfile, StructuredKind, Terminator,
    budget::count,
};
use std::collections::BTreeSet;

pub(super) fn compare(profile: ReadabilityProfile, metrics: ReadabilityMetrics) -> bool {
    // Natural-reading profiles cannot choose additional dispatched source blocks
    // on proxy cost alone, so do not pay to build that dominated alternative.
    if profile != ReadabilityProfile::Compact && metrics.dispatcher_blocks == 0 {
        return false;
    }
    let (floor, copies, depth) = match profile {
        ReadabilityProfile::Balanced => (256, 8, 10),
        ReadabilityProfile::Linear => (128, 16, 6),
        ReadabilityProfile::Compact => (128, 2, 8),
    };
    // Small source trees often contain raw payload builders that a later,
    // independently certified projection displays as one guard. Their raw
    // instruction repetition is not evidence for replacing natural control.
    // Expression outlining belongs to Lifted; explicit region choices still
    // bypass this conservative automatic-comparison floor.
    metrics.syntax_nodes >= floor
        && (metrics.duplicated_operations > copies || metrics.max_nesting >= depth)
}

pub(super) fn score(profile: ReadabilityProfile, metrics: ReadabilityMetrics) -> u64 {
    let (operations, repeats, nesting, cases, transitions) = match profile {
        ReadabilityProfile::Balanced => (4, 8, 4, 18, 6),
        ReadabilityProfile::Linear => (4, 10, 12, 28, 8),
        ReadabilityProfile::Compact => (2, 16, 3, 8, 3),
    };
    metrics
        .operations
        .saturating_mul(operations)
        .saturating_add(metrics.duplicated_operations.saturating_mul(repeats))
        .saturating_add(metrics.conditions.saturating_mul(3))
        .saturating_add(
            metrics
                .max_nesting
                .saturating_mul(metrics.max_nesting)
                .saturating_mul(metrics.conditions.clamp(1, 16))
                .saturating_mul(nesting),
        )
        .saturating_add(metrics.dispatcher_blocks.saturating_mul(cases))
        .saturating_add(metrics.dispatcher_transitions.saturating_mul(transitions))
        .saturating_add(metrics.entry_values.saturating_mul(2))
        .saturating_add(metrics.syntax_nodes)
}

/// Later semantic projection can turn source repetition into short guards or
/// shared computations. Natural-reading profiles therefore do not trade more
/// dispatched source blocks for a lower raw-size score. Compact explicitly
/// allows that choice; representation safety is checked independently.
pub(super) fn prefers_compact(
    profile: ReadabilityProfile,
    natural: ReadabilityMetrics,
    compact: ReadabilityMetrics,
) -> bool {
    if profile != ReadabilityProfile::Compact
        && compact.dispatcher_blocks > natural.dispatcher_blocks
    {
        return false;
    }
    score(profile, compact) < score(profile, natural)
}

impl<P: FnMut(Progress)> Planner<'_, P> {
    /// Discount a success continuation's visual nesting only when source facts
    /// already prove the entire other arm finitely fails. Unknown/cyclic arms
    /// do not get the early-failure projection's readability advantage.
    pub(super) fn failure_direction(
        &self,
        kind: &StructuredKind<PlanId>,
        sources: &[EntityId],
    ) -> Option<bool> {
        let StructuredKind::If { condition, .. } = kind else {
            return None;
        };
        for source in sources {
            let Some(block) = self.block(*source) else {
                continue;
            };
            if let Terminator::Branch {
                condition: original,
                then_edges,
                else_edge: Some(other),
                unresolved: false,
                invalid_targets,
                ..
            } = &block.terminator
                && original == condition
                && invalid_targets.is_empty()
            {
                if !then_edges.is_empty()
                    && then_edges.iter().all(|id| {
                        self.edge(*id)
                            .is_some_and(|edge| self.facts.failed.contains(&edge.to))
                    })
                {
                    return Some(true);
                }
                if self
                    .edge(*other)
                    .is_some_and(|edge| self.facts.failed.contains(&edge.to))
                {
                    return Some(false);
                }
            }
        }
        None
    }

    pub(super) fn node_metrics(
        &self,
        node: &PlanNode,
        quote: Quote,
        failure_direction: Option<bool>,
    ) -> (ReadabilityMetrics, u64) {
        let mut result = ReadabilityMetrics {
            syntax_nodes: count(quote.nodes),
            ..Default::default()
        };
        let mut failed_operations = 0_u64;
        let mut child = |id: PlanId| {
            let next = self.metrics[id.0];
            result.operations = result.operations.saturating_add(next.operations);
            result.conditions = result.conditions.saturating_add(next.conditions);
            result.max_nesting = result.max_nesting.max(next.max_nesting);
            result.dispatcher_blocks = result
                .dispatcher_blocks
                .saturating_add(next.dispatcher_blocks);
            result.dispatcher_transitions = result
                .dispatcher_transitions
                .saturating_add(next.dispatcher_transitions);
            failed_operations = failed_operations.saturating_add(self.failed_operations[id.0]);
        };
        match &node.kind {
            StructuredKind::Sequence(children) => {
                for id in children {
                    child(*id);
                }
            }
            StructuredKind::If {
                then_branch,
                else_branch,
                ..
            } => {
                child(**then_branch);
                child(**else_branch);
                result.conditions = result.conditions.saturating_add(1);
                result.max_nesting = match failure_direction {
                    Some(true) => self.metrics[then_branch.0]
                        .max_nesting
                        .saturating_add(1)
                        .max(self.metrics[else_branch.0].max_nesting),
                    Some(false) => self.metrics[else_branch.0]
                        .max_nesting
                        .saturating_add(1)
                        .max(self.metrics[then_branch.0].max_nesting),
                    None => result.max_nesting.saturating_add(1),
                };
            }
            StructuredKind::While {
                header,
                body,
                condition,
                ..
            } => {
                child(**header);
                child(**body);
                result.conditions = result
                    .conditions
                    .saturating_add(u64::from(condition.is_some()));
                result.max_nesting = result.max_nesting.saturating_add(1);
            }
            StructuredKind::Indirect { arms, .. } => {
                for arm in arms {
                    child(arm.body);
                }
                result.conditions = result.conditions.saturating_add(1);
                result.max_nesting = result.max_nesting.saturating_add(1);
            }
            StructuredKind::Block { block } => {
                result.operations =
                    count(self.block(*block).map_or(0, |block| block.operations.len()));
                if self.facts.failed.contains(block) {
                    failed_operations = result.operations;
                }
            }
            StructuredKind::Dispatcher { cases, .. } => {
                result.max_nesting = 2;
                result.dispatcher_blocks = count(cases.len());
                result.conditions = 1;
                for case in cases {
                    if let Some(block) = self.block(case.block) {
                        result.operations = result
                            .operations
                            .saturating_add(count(block.operations.len()));
                        if self.facts.failed.contains(&block.id) {
                            failed_operations =
                                failed_operations.saturating_add(count(block.operations.len()));
                        }
                        result.conditions = result.conditions.saturating_add(u64::from(matches!(
                            block.terminator,
                            Terminator::Branch { .. }
                        )));
                    }
                    result.dispatcher_transitions = result
                        .dispatcher_transitions
                        .saturating_add(count(case.transitions.len()));
                }
            }
            _ => {}
        }
        let unique_operations = node
            .sources
            .iter()
            .filter_map(|id| self.block(*id))
            .fold(0_u64, |sum, block| {
                sum.saturating_add(count(block.operations.len()))
            });
        result.duplicated_operations = result.operations.saturating_sub(unique_operations);
        (result, failed_operations)
    }

    /// Finite-failure payload builders are not evidence of profitable control
    /// compaction: a later certified view may display them as guards. Receipts
    /// still expose all source operations; this cost adjustment never erases any.
    pub(super) fn comparison_metrics(
        &self,
        body: PlanId,
        mut metrics: ReadabilityMetrics,
    ) -> ReadabilityMetrics {
        let failed = self.failed_operations[body.0];
        let unique = self.nodes[body.0]
            .sources
            .iter()
            .filter(|id| self.facts.failed.contains(id))
            .filter_map(|id| self.block(*id))
            .fold(0_u64, |sum, block| {
                sum.saturating_add(count(block.operations.len()))
            });
        metrics.operations = metrics.operations.saturating_sub(failed);
        metrics.duplicated_operations = metrics
            .duplicated_operations
            .saturating_sub(failed.saturating_sub(unique));
        metrics
    }

    pub(super) fn dispatch_comparison_metrics(
        &self,
        region: &BTreeSet<EntityId>,
        mut metrics: ReadabilityMetrics,
    ) -> ReadabilityMetrics {
        let failed = region
            .iter()
            .filter(|id| self.facts.failed.contains(id))
            .filter_map(|id| self.block(*id))
            .fold(0_u64, |sum, block| {
                sum.saturating_add(count(block.operations.len()))
            });
        metrics.operations = metrics.operations.saturating_sub(failed);
        metrics
    }

    pub(super) fn dispatch_metrics(
        &mut self,
        start: EntityId,
        region: &BTreeSet<EntityId>,
    ) -> Option<ReadabilityMetrics> {
        if !self.owner.pay(region.len().saturating_mul(8)) {
            return None;
        }
        let mut result = ReadabilityMetrics {
            max_nesting: 2,
            conditions: 1,
            syntax_nodes: 1,
            dispatcher_blocks: count(region.len()),
            entry_values: count(self.block(start)?.parameters.len()),
            ..Default::default()
        };
        for id in region {
            let block = self.block(*id)?;
            result.operations = result
                .operations
                .saturating_add(count(block.operations.len()));
            result.conditions = result.conditions.saturating_add(u64::from(matches!(
                block.terminator,
                Terminator::Branch { .. }
            )));
            result.dispatcher_transitions = result
                .dispatcher_transitions
                .saturating_add(count(block.terminator.edges().len()));
        }
        Some(result)
    }
}

mod context;
mod portals;
mod sources;

use std::collections::BTreeMap;

use super::{
    super::scope::{
        CardKind, ControlOrigin, Destination, FlowOrigin, GraphProjection, GraphScope,
        OriginEvidence, ProjectionCard, ProjectionError, ProjectionTarget, ScopeCounts,
        TargetEntry, TargetSet, TargetSetKind,
    },
    edge,
};
use evm_abstract_protocol::{
    AnalysisReport, MinimumSummary, ProcessControlRelation, ProcessLocation, ProgramInfo,
    RegionalNode, SummarySourceStatus,
};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PointKey {
    Source(ProcessLocation),
    Receipt(usize),
}

struct Builder<'a> {
    report: &'a AnalysisReport,
    summary: usize,
    relation: &'a MinimumSummary,
    programs: BTreeMap<usize, &'a ProgramInfo>,
    directories: BTreeMap<usize, &'a ProcessControlRelation>,
    members: Vec<&'a RegionalNode>,
    points: BTreeMap<ControlOrigin, usize>,
    groups: BTreeMap<PointKey, usize>,
    projection: GraphProjection,
    overview: bool,
}

pub(super) fn build(
    report: &AnalysisReport,
    scope: GraphScope,
) -> Result<GraphProjection, ProjectionError> {
    let model = report
        .minimum
        .as_ref()
        .ok_or(ProjectionError::MissingModel)?;
    let summary = scope.summary();
    let relation = model
        .summaries
        .get(summary)
        .ok_or(ProjectionError::MissingSummary)?;
    let status = relation.relation.source_status(
        model.summaries[summary + 1..]
            .iter()
            .any(|later| later.relation.source_state == relation.relation.source_state),
    );
    let overview = matches!(scope, GraphScope::Overview { .. });
    let (members, anchor) = context::resolve(relation, scope)?;
    let mut builder = Builder {
        report,
        summary,
        relation,
        programs: report
            .programs
            .iter()
            .map(|row| (row.id, row.as_ref()))
            .collect(),
        directories: report
            .process_controls
            .iter()
            .map(|row| (row.id, row.as_ref()))
            .collect(),
        members,
        points: BTreeMap::new(),
        groups: BTreeMap::new(),
        projection: GraphProjection {
            scope,
            source_status: status,
            cards: Vec::new(),
            edges: Vec::new(),
            target_sets: Vec::new(),
            counts: ScopeCounts::default(),
            anchor: None,
            hidden_set: None,
        },
        overview,
    };
    builder.sources()?;
    builder.source_edges()?;
    builder.unexpanded_jumps();
    builder.regional_edges();
    builder.calls();
    builder.projection.anchor = anchor.and_then(|origin| builder.points.get(&origin).copied());
    let cfg = &relation.relation.regional_cfg;
    let counts = &mut builder.projection.counts;
    counts.total_receipts = cfg.nodes.len();
    counts.context_receipts = builder.members.len();
    counts.displayed_receipts = builder.members.len();
    counts.other_context_receipts = cfg.nodes.len().saturating_sub(builder.members.len());
    counts.total_points = builder.groups.len();
    counts.displayed_points = counts.total_points;
    counts.unexpanded_points = builder
        .projection
        .cards
        .iter()
        .filter(|card| {
            matches!(card.kind, CardKind::Source { .. } | CardKind::Receipt)
                && !card
                    .origins
                    .iter()
                    .any(|origin| origin.through || origin.transferred)
        })
        .count();
    counts.displayed_unexpanded_points = counts.unexpanded_points;
    counts.displayed_calls = counts.calls;
    Ok(builder.projection)
}

impl Builder<'_> {
    fn target(&self, node: &RegionalNode, location: Option<ProcessLocation>) -> ProjectionTarget {
        ProjectionTarget {
            summary: self.summary,
            procedure: node.procedure,
            origin: ControlOrigin {
                node: node.id,
                location,
            },
        }
    }

    fn evidence(
        &self,
        node: &RegionalNode,
        location: Option<ProcessLocation>,
        through: bool,
        boundary: bool,
    ) -> OriginEvidence {
        OriginEvidence {
            target: self.target(node, location),
            input_revision: node.input_revision,
            current: self.projection.source_status == SummarySourceStatus::Current
                && !node.excluded
                && node.transfer.as_ref().is_some_and(|transfer| {
                    transfer.current && transfer.input_revision == node.input_revision
                }),
            complete: node
                .transfer
                .as_ref()
                .is_some_and(|transfer| transfer.complete),
            excluded: node.excluded,
            boundary,
            through,
            transferred: location.is_none() && node.transfer.is_some(),
        }
    }

    fn point(&self, node: &RegionalNode, location: ProcessLocation) -> Option<usize> {
        self.points
            .get(&ControlOrigin {
                node: node.id,
                location: Some(location),
            })
            .copied()
    }

    fn body(&self, node: usize) -> Option<usize> {
        self.points
            .get(&ControlOrigin {
                node,
                location: None,
            })
            .copied()
    }

    fn portal(
        &mut self,
        kind: CardKind,
        origins: Vec<OriginEvidence>,
        targets: TargetSet,
    ) -> usize {
        let target_set = self.projection.target_sets.len();
        self.projection.target_sets.push(targets);
        let id = self.projection.cards.len();
        self.projection.cards.push(ProjectionCard {
            kind,
            origins,
            target_set: Some(target_set),
        });
        id
    }
}

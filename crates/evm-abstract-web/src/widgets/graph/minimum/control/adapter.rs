//! Text/color adapter for the shared compact renderer. Evidence ownership and
//! window construction stay in the pure projection modules.

use std::collections::BTreeMap;

use super::{
    CardKind, Control, Destination, Edge, FlowOrigin, GraphProjection, GraphScope, GraphWindow,
    ProjectionError, TargetSet, TargetSetKind, card, compact_owner,
};
use crate::{app::minimum::location_label, palette};
use evm_abstract_protocol::{AnalysisReport, CodeKind, EdgeKind, ProcessLocation};

impl Control {
    pub(crate) fn scoped(
        report: &AnalysisReport,
        scope: GraphScope,
    ) -> Result<Self, ProjectionError> {
        let projection = GraphProjection::build(report, scope)?;
        Ok(Self::from_projection(report, projection))
    }

    pub(crate) fn scoped_with_limit(
        report: &AnalysisReport,
        scope: GraphScope,
        point_limit: usize,
    ) -> Result<Self, ProjectionError> {
        let projection = GraphProjection::with_limit(report, scope, point_limit)?;
        Ok(Self::from_projection(report, projection))
    }

    pub(crate) fn scoped_window(
        report: &AnalysisReport,
        scope: GraphScope,
        window: GraphWindow,
    ) -> Result<Self, ProjectionError> {
        match window {
            GraphWindow::Entry => Self::scoped(report, scope),
            GraphWindow::Nearby { point_limit } => {
                Self::scoped_with_limit(report, scope, point_limit)
            }
            GraphWindow::FullSource => {
                let projection = GraphProjection::with_window(report, scope, window)?;
                Ok(Self::from_projection(report, projection))
            }
        }
    }

    fn from_projection(report: &AnalysisReport, projection: GraphProjection) -> Self {
        let scope = projection.scope;
        let cfg = &report.minimum.as_ref().unwrap().summaries[scope.summary()]
            .relation
            .regional_cfg;
        let nodes: BTreeMap<_, _> = cfg
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.id, (index, node)))
            .collect();
        let programs: BTreeMap<_, _> = report
            .programs
            .iter()
            .map(|program| (program.id, program.as_ref()))
            .collect();
        let cards = projection.cards.iter().map(|projected| {
            let representative = projected.origins.first().map(|origin| origin.target)
                .or_else(|| projected.target_set.and_then(|index| projection.target_sets[index].entries.iter().find_map(|entry| {
                    match entry.destination { Destination::Control(target) => Some(target), Destination::Source { .. } => None }
                })));
            let (index, node) = representative.and_then(|target| nodes.get(&target.origin.node).copied())
                .or_else(|| cfg.nodes.first().map(|node| (0, node)))
                .expect("every visible point or portal has a retained source receipt");
            let location = representative.and_then(|target| target.origin.location);
            let program = node.frame.process.as_ref().and_then(|frame| report.process_program(&frame.source));
            let mut card = card(node, index, location, false, program);
            card.kind = projected.kind;
            card.origins = projected.origins.clone();
            card.target_set = projected.target_set;
            card.boundary = projected.target_set.is_some() || projected.origins.iter().any(|origin| origin.boundary);
            let set = projected.target_set.map(|index| &projection.target_sets[index]);
            let occurrences = projected.origins.len();
            let suffix = if occurrences == 1 { "1 receipt".into() } else { format!("{occurrences} separate receipts") };
            match projected.kind {
                CardKind::Source { program, location } => {
                    card.title = match location {
                        ProcessLocation::Block(index) => program.and_then(|id| programs.get(&id)).and_then(|program| program.blocks.get(index))
                            .map_or_else(|| "Uncaptured source block".into(), |block| format!("0x{:04x} · {}", block.start_pc,
                                block.instructions.last().map_or("end", |instruction| instruction.name.as_str()))),
                        ProcessLocation::JumpTargets => "Unknown jump".into(),
                        ProcessLocation::Any => match node.frame.kind {
                            CodeKind::Precompile => "Precompile",
                            CodeKind::Empty => "Empty code",
                            CodeKind::InvalidDelegation => "Invalid delegation",
                            CodeKind::Initcode => "Unknown constructor",
                            _ => "Unknown code",
                        }.into(),
                        ProcessLocation::End => "End of code".into(),
                    };
                    card.caption = if let Some(TargetSet { kind: TargetSetKind::JumpTargets { relation }, .. }) = set {
                        let targets = report.process_controls.iter().find(|row| row.id == *relation).map_or(0, |row| row.jump_targets.len());
                        format!("MAY {targets} legal JUMPDEST targets · inspect set")
                    } else {
                        format!("MAY source · {suffix}{}", if projected.origins.iter().any(|origin| origin.boundary) { " · boundary" } else { "" })
                    };
                    if !projected.origins.iter().any(|origin| origin.through || origin.transferred) {
                        card.caption.push_str(" · unexpanded");
                    }
                }
                CardKind::Receipt => {
                    card.title = node.frame.process.as_ref().map_or_else(|| format!("{:?} control", node.frame.kind), |frame| location_label(frame.location));
                    card.caption = format!("Regional receipt · {suffix}");
                }
                CardKind::Calls { action } => {
                    card.title = action.map_or_else(|| "Call boundary".into(), |action| format!("{action:?}").to_uppercase());
                    let calls = set.and_then(|set| match &set.kind { TargetSetKind::Calls { calls } => Some(calls.len()), _ => None }).unwrap_or(0);
                    let entries = set.map_or(0, |set| set.entries.iter().filter(|entry| matches!(entry.via, FlowOrigin::Call { .. })).count());
                    card.caption = format!("{calls} calls · {entries} callee entries · inspect targets");
                    if let Some(TargetSet { kind: TargetSetKind::Calls { calls }, .. }) = set {
                        let mut targets = Vec::new();
                        for id in calls {
                            if let Some(target) = cfg.calls.iter().find(|call| call.id == *id).and_then(|call| call.target.as_ref())
                                && !targets.contains(&target) {
                                targets.push(target);
                            }
                        }
                        if let [target] = targets.as_slice() {
                            card.title.push_str(&format!(" · {}", compact_owner(target)));
                            card.hover.push_str(&format!("\nOriginal call target: {target:?}"));
                        }
                    }
                }
                CardKind::Folded => {
                    card.title = if set.is_some_and(|set| set.kind == TargetSetKind::ContextBoundary) { "Other context" } else { "More control" }.into();
                    let mut destinations = Vec::new();
                    if let Some(set) = set {
                        for entry in &set.entries { if !destinations.contains(&entry.destination) { destinations.push(entry.destination); } }
                    }
                    card.caption = format!("{} destinations · navigate to expand", destinations.len());
                }
            }
            card.hover.push_str("\nSource grouping keeps each receipt separate; no instruction-level execution or joint state is inferred.");
            card
        }).collect();
        let edges = projection
            .edges
            .iter()
            .map(|edge| {
                let delivery = edge
                    .origins
                    .iter()
                    .any(|origin| matches!(origin, FlowOrigin::Delivery { .. }));
                let current = edge
                    .origins
                    .iter()
                    .all(|origin| origin.qualified_current(report, scope.summary()));
                Edge {
                    from: edge.from,
                    to: edge.to,
                    kind: edge.kind,
                    level: edge.level,
                    label: match edge.kind {
                        EdgeKind::Fallthrough | EdgeKind::ProcessControl => String::new(),
                        EdgeKind::BranchTrue => "true".into(),
                        EdgeKind::BranchFalse => "false".into(),
                        EdgeKind::Jump => "MAY jump".into(),
                        EdgeKind::Call => "call".into(),
                        _ => format!("{:?}", edge.kind),
                    },
                    color: if edge.kind == EdgeKind::Call {
                        palette::PURPLE
                    } else if delivery && current {
                        palette::BLUE
                    } else {
                        palette::MUTED
                    },
                    origins: edge.origins.clone(),
                }
            })
            .collect();
        Self {
            cards,
            edges,
            source_edges: projection
                .edges
                .iter()
                .flat_map(|edge| &edge.origins)
                .filter(|origin| matches!(origin, FlowOrigin::Source { .. }))
                .count(),
            hubs: projection
                .cards
                .iter()
                .filter(|card| {
                    matches!(
                        card.kind,
                        CardKind::Source {
                            location: ProcessLocation::JumpTargets,
                            ..
                        }
                    )
                })
                .count(),
            scope: Some(scope),
            window: projection.window,
            counts: projection.counts,
            target_sets: projection.target_sets,
            anchor: projection.anchor,
            core: projection.core,
            frontiers: projection.frontiers,
            hidden_set: projection.hidden_set,
            level: projection.level,
            model_relations: projection.model_relations,
            model_set: projection.model_set,
        }
    }
}

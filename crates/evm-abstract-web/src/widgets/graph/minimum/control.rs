//! Expand actual pure-region receipts using shared source geometry. Display IDs
//! are private; a source point always retains its regional invocation identity.

#[cfg(test)]
use std::collections::BTreeMap;

use crate::app::minimum::location_label;
#[cfg(test)]
use crate::palette;
use egui::Color32;
#[cfg(test)]
use evm_abstract_protocol::AnalysisReport;
use evm_abstract_protocol::{EdgeKind, ProcessLocation, RegionalNode};

mod adapter;
mod evidence;
mod projection;
mod scope;
pub(crate) use scope::*;

pub(super) struct Card {
    #[cfg(test)]
    pub node: usize,
    #[cfg(test)]
    pub location: Option<ProcessLocation>,
    pub boundary: bool,
    pub title: String,
    pub caption: String,
    pub qualifier: String,
    pub hover: String,
    pub kind: CardKind,
    pub origins: Vec<OriginEvidence>,
    pub target_set: Option<usize>,
}

pub(super) struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: EdgeKind,
    pub label: String,
    pub color: Color32,
    pub origins: Vec<FlowOrigin>,
}

#[derive(Default)]
pub(super) struct Control {
    pub cards: Vec<Card>,
    pub edges: Vec<Edge>,
    pub source_edges: usize,
    pub hubs: usize,
    pub scope: Option<GraphScope>,
    pub counts: ScopeCounts,
    pub target_sets: Vec<TargetSet>,
    pub anchor: Option<usize>,
    pub hidden_set: Option<usize>,
}

impl Control {
    /// Exhaustive receipt oracle retained for the original semantic regressions.
    #[cfg(test)]
    pub fn new(report: &AnalysisReport, summary: usize) -> Self {
        let model = report.minimum.as_ref().unwrap();
        let cfg = &model.summaries[summary].relation.regional_cfg;
        let programs: BTreeMap<_, _> = report.programs.iter().map(|p| (p.id, p.as_ref())).collect();
        let relations: BTreeMap<_, _> = report
            .process_controls
            .iter()
            .map(|r| (r.id, r.as_ref()))
            .collect();
        let node_programs: Vec<_> = cfg
            .nodes
            .iter()
            .map(|node| {
                if let Some(region) = node
                    .transfer
                    .as_ref()
                    .and_then(|transfer| transfer.process.as_ref())
                    .and_then(|process| process.control.as_ref())
                {
                    return relations
                        .get(&region.relation)
                        .and_then(|relation| relation.program)
                        .and_then(|id| programs.get(&id).copied());
                }
                node.frame.process.as_ref().and_then(|frame| {
                    programs
                        .values()
                        .copied()
                        .find(|program| frame.source.matches_program(program))
                })
            })
            .collect();
        let mut result = Self::default();
        let mut entries = BTreeMap::new();
        let mut points = BTreeMap::new();
        // Allocate regional anchors first, including sparse table IDs. Different
        // owners/static modes/call contexts never share a display point.
        for (index, node) in cfg.nodes.iter().enumerate() {
            let id = result.cards.len();
            entries.insert(node.id, id);
            if let Some(frame) = &node.frame.process {
                points.insert((node.id, frame.location), id);
            }
            result
                .cards
                .push(card(node, index, None, false, node_programs[index]));
        }
        for (index, node) in cfg.nodes.iter().enumerate() {
            let Some(region) = node
                .transfer
                .as_ref()
                .and_then(|t| t.process.as_ref())
                .and_then(|p| p.control.as_ref())
            else {
                continue;
            };
            let Some(relation) = relations.get(&region.relation) else {
                continue;
            };
            for &location in region.through.iter().chain(&region.boundaries) {
                points.entry((node.id, location)).or_insert_with(|| {
                    let id = result.cards.len();
                    result.cards.push(card(
                        node,
                        index,
                        Some(location),
                        region.boundaries.contains(&location),
                        node_programs[index],
                    ));
                    id
                });
            }
            // Only actual through points authorize showing source edges. A
            // boundary obligation alone never expands the directory's code.
            for &location in &region.through {
                match location {
                    ProcessLocation::Block(block) => {
                        if let Some(block) = relation.blocks.get(block) {
                            for edge in block.successors.iter().flatten() {
                                if let (Some(&from), Some(&to)) = (
                                    points.get(&(node.id, location)),
                                    points.get(&(node.id, edge.to)),
                                ) {
                                    result.edges.push(Edge {
                                        from,
                                        to,
                                        kind: edge.kind,
                                        label: format!("MAY {:?}", edge.kind),
                                        color: palette::MUTED,
                                        origins: Vec::new(),
                                    });
                                    result.source_edges += 1;
                                }
                            }
                            // continuation is deliberately not a local edge:
                            // CALL/CREATE return control needs a real delivery.
                        }
                    }
                    ProcessLocation::JumpTargets => {
                        result.hubs += 1;
                        for &target in &relation.jump_targets {
                            let to = ProcessLocation::Block(target);
                            if let (Some(&from), Some(&to)) =
                                (points.get(&(node.id, location)), points.get(&(node.id, to)))
                            {
                                result.edges.push(Edge {
                                    from,
                                    to,
                                    kind: EdgeKind::Jump,
                                    label: "MAY legal JUMPDEST".into(),
                                    color: palette::WARNING,
                                    origins: Vec::new(),
                                });
                                result.source_edges += 1;
                            }
                        }
                    }
                    ProcessLocation::Any | ProcessLocation::End => {}
                }
            }
        }
        for node in &cfg.nodes {
            for &target in &node.successors {
                let kind = node
                    .transfer
                    .as_ref()
                    .and_then(|t| t.successors.iter().find(|e| e.to == target))
                    .map_or(EdgeKind::Fallthrough, |e| e.kind);
                if let (Some(&from), Some(&to)) = (entries.get(&node.id), entries.get(&target)) {
                    result.edges.push(Edge {
                        from,
                        to,
                        kind,
                        label: format!("r{} → r{target} · {kind:?}", node.id),
                        color: palette::BLUE,
                        origins: Vec::new(),
                    });
                }
            }
        }
        for call in &cfg.calls {
            for target in &call.entries {
                if let (Some(&from), Some(&to)) = (entries.get(&call.from), entries.get(target)) {
                    result.edges.push(Edge {
                        from,
                        to,
                        kind: EdgeKind::Call,
                        label: format!("call c{} · r{} → r{target}", call.id, call.from),
                        color: palette::PURPLE,
                        origins: Vec::new(),
                    });
                }
            }
            for delivery in &call.deliveries {
                for target in &delivery.successors {
                    if let (Some(&from), Some(&to)) =
                        (entries.get(&call.from), entries.get(&target.to))
                    {
                        result.edges.push(Edge {
                            from,
                            to,
                            kind: target.kind,
                            label: format!(
                                "c{} {:?} · input {} · generation {}",
                                call.id,
                                delivery.kind,
                                delivery.input_revision,
                                delivery.output_generation
                            ),
                            color: if delivery.current
                                && delivery.input_revision == call.input_revision
                            {
                                palette::BLUE
                            } else {
                                palette::MUTED
                            },
                            origins: Vec::new(),
                        });
                    }
                }
            }
        }
        result
    }
}

fn card(
    node: &RegionalNode,
    _index: usize,
    location: Option<ProcessLocation>,
    boundary: bool,
    program: Option<&evm_abstract_protocol::ProgramInfo>,
) -> Card {
    let frame = node.frame.process.as_ref();
    let point = location.or_else(|| frame.map(|frame| frame.location));
    // Program IDs and dense block indices are different domains.
    let caption = match point {
        Some(ProcessLocation::Block(index)) => program
            .and_then(|program| {
                program.blocks.get(index).map(|block| {
                    format!(
                        "MAY B{} · program {} · source 0x{:04x}",
                        block.id, program.id, block.start_pc
                    )
                })
            })
            .unwrap_or_else(|| {
                format!(
                    "MAY {} · source bytes unavailable",
                    location_label(ProcessLocation::Block(index))
                )
            }),
        Some(location) => format!("MAY {}", location_label(location)),
        None => "Regional control receipt".into(),
    };
    let qualifier = frame.map_or_else(
        || format!("input {} · {:?}", node.input_revision, node.frame.kind),
        |frame| {
            format!(
                "owner {} · static {}",
                compact_owner(&frame.owner),
                frame.is_static
            )
        },
    );
    let hover = frame.map_or_else(String::new, |frame| {
        format!(
            "Source: {:?}\nEpoch: {}\nOwner: {:?} · static {}\nInput revision: {}\n{}",
            frame.source,
            frame.epoch,
            frame.owner,
            frame.is_static,
            node.input_revision,
            if boundary {
                "Boundary obligation; effects require its own Body receipt"
            } else {
                "MAY source geometry for this regional receipt"
            }
        )
    });
    Card {
        #[cfg(test)]
        node: _index,
        #[cfg(test)]
        location,
        boundary,
        title: format!(
            "r{} · procedure {}{}",
            node.id,
            node.procedure,
            if location.is_some() {
                " · MAY point"
            } else {
                ""
            }
        ),
        caption,
        qualifier,
        hover,
        kind: point.map_or(CardKind::Receipt, |location| CardKind::Source {
            program: program.map(|program| program.id),
            location,
        }),
        origins: Vec::new(),
        target_set: None,
    }
}

fn compact_owner(owner: &evm_abstract_protocol::ProcessOwner) -> String {
    match owner {
        evm_abstract_protocol::ProcessOwner::RootNamespace => "root namespace".into(),
        evm_abstract_protocol::ProcessOwner::Residual => "residual class".into(),
        evm_abstract_protocol::ProcessOwner::Named(address) if address.len() > 18 => {
            format!("{}…{}", &address[..8], &address[address.len() - 6..])
        }
        evm_abstract_protocol::ProcessOwner::Named(address) => address.clone(),
    }
}

#[cfg(test)]
mod tests;

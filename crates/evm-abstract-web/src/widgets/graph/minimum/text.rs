//! Cached code previews are renderer data. Regional origin/currentness remains
//! in Control; no preview row is promoted to an executed instruction.
use super::control::{Card, CardKind};
use crate::{palette, widgets::graph::source};
use egui::{Color32, Vec2};
use evm_abstract_protocol::{AnalysisReport, ProcessLocation};

const PREVIEW_ROWS: usize = 4;
pub(super) const ACTION_HEIGHT: f32 = 24.0;

pub(super) struct CardText {
    pub title: String,
    pub compact_title: String,
    pub detail: String,
    pub rows: Vec<(String, Color32)>,
    pub size: Vec2,
}

impl CardText {
    pub(super) fn new(report: &AnalysisReport, card: &Card) -> Self {
        let mut title = card.title.clone();
        let mut compact_title = title.clone();
        let mut detail = card.caption.clone();
        let mut rows = Vec::new();
        if let CardKind::Source {
            program: Some(program),
            location: ProcessLocation::Block(index),
        } = card.kind
            && let Some(block) = report
                .programs
                .iter()
                .find(|row| row.id == program)
                .and_then(|program| program.blocks.get(index))
        {
            title = source::title(block.start_pc, &block.instructions);
            compact_title = format!("Block 0x{:04x}", block.start_pc);
            detail = source::ending(&block.instructions);
            let prefix = &block.instructions[..block.instructions.len().saturating_sub(1)];
            rows.extend(prefix.iter().take(PREVIEW_ROWS).map(|instruction| {
                (
                    source::instruction(instruction),
                    if instruction.valid {
                        palette::TEXT
                    } else {
                        palette::ERROR
                    },
                )
            }));
            if prefix.len() > PREVIEW_ROWS {
                rows.push((
                    format!(
                        "… {} more instructions · open Code",
                        prefix.len() - PREVIEW_ROWS
                    ),
                    palette::MUTED,
                ));
            }
        }
        if !card.origins.is_empty()
            && card
                .origins
                .iter()
                .all(|origin| !origin.through && !origin.transferred)
        {
            rows.push(("Unexpanded control boundary".into(), palette::WARNING));
        }
        if matches!(card.kind, CardKind::Receipt) {
            rows.push((card.qualifier.clone(), palette::MUTED));
        }
        let actions = if !card.origins.is_empty() || card.target_set.is_some() {
            ACTION_HEIGHT
        } else {
            0.0
        };
        let height = 42.0 + rows.len() as f32 * 14.0 + actions + 6.0;
        Self {
            title,
            compact_title,
            detail,
            rows,
            size: Vec2::new(300.0, height),
        }
    }
}

pub(super) fn context_caption(
    report: &AnalysisReport,
    scope: super::GraphScope,
    control: &super::control::Control,
) -> String {
    use super::control::GraphLevel;
    if control.level == GraphLevel::Model {
        return "Regional relations · independent contexts".into();
    }
    let node = control
        .anchor
        .and_then(|id| control.cards.get(id))
        .and_then(|card| card.origins.first())
        .and_then(|origin| {
            report
                .minimum
                .as_ref()?
                .summaries
                .get(scope.summary())?
                .relation
                .regional_cfg
                .nodes
                .iter()
                .find(|node| node.id == origin.target.origin.node)
        });
    let qualifier = node
        .and_then(|node| node.frame.process.as_ref())
        .map_or_else(
            || "Source context".into(),
            |frame| {
                let owner = match &frame.owner {
                    evm_abstract_protocol::ProcessOwner::RootNamespace => "Root input owner".into(),
                    evm_abstract_protocol::ProcessOwner::Residual => "Residual owner".into(),
                    evm_abstract_protocol::ProcessOwner::Named(address)
                        if address.is_ascii() && address.len() > 18 =>
                    {
                        format!("owner {}…{}", &address[..8], &address[address.len() - 6..])
                    }
                    evm_abstract_protocol::ProcessOwner::Named(address) => {
                        format!("owner {address}")
                    }
                };
                format!("static {} · {owner}", frame.is_static)
            },
        );
    format!(
        "{} · {qualifier}",
        if control.level == GraphLevel::Source {
            "MAY"
        } else {
            "Abstract boundary"
        }
    )
}

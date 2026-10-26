//! Entry geometry is chosen from the actual canvas at first use, at readable
//! type scale. The same typed ELK routes serve Entry, Nearby and Full source.
use super::{control::Control, text::CardText};
use crate::{
    palette,
    widgets::graph::{Placement, layout},
};
use egui::{FontId, Painter, Rect, Vec2};
use evm_abstract_layout as geometry;

#[derive(Clone, Copy)]
pub(super) struct Density {
    anchor_width: f32,
    other_width: f32,
    anchor_rows: usize,
    other_rows: usize,
    compact: bool,
}

pub(super) fn candidates(canvas: Vec2, alternatives: usize, entry: bool) -> Vec<Density> {
    let available = (canvas.x - 16.0).max(100.0);
    let width = available.min(300.0);
    let other = if entry && alternatives > 1 {
        ((available - 16.0 * (alternatives - 1) as f32) / alternatives as f32).clamp(88.0, 300.0)
    } else {
        width
    };
    [(4, 4, false), (2, 1, false), (1, 0, true), (0, 0, true)]
        .into_iter()
        .map(|(anchor_rows, other_rows, compact)| Density {
            anchor_width: width,
            other_width: other,
            anchor_rows,
            other_rows,
            compact,
        })
        .collect()
}

impl Density {
    /// ELK ports/labels can widen a core beyond the sum of its card widths.
    /// Refit dimensions from measured geometry; type size is never reduced.
    pub(super) fn fit_width(self, measured: Rect, canvas: Vec2) -> Self {
        let ratio = ((canvas.x - 16.0) / measured.width()).clamp(0.5, 0.97);
        Self {
            anchor_width: (self.anchor_width * ratio).max(120.0),
            other_width: (self.other_width * ratio).max(88.0),
            ..self
        }
    }
}

pub(super) fn text(base: &[CardText], anchor: Option<usize>, density: Density) -> Vec<CardText> {
    base.iter()
        .enumerate()
        .map(|(id, text)| {
            let mut text = text.clone();
            let root = Some(id) == anchor;
            text.resize(
                if root {
                    density.anchor_width
                } else {
                    density.other_width
                },
                if root {
                    density.anchor_rows
                } else {
                    density.other_rows
                },
                density.compact,
            );
            text
        })
        .collect()
}

pub(super) fn place(
    control: &Control,
    text: &[CardText],
    painter: &Painter,
) -> Result<Placement, geometry::LayoutError> {
    let input = geometry::Input {
        nodes: control
            .cards
            .iter()
            .enumerate()
            .map(|(id, card)| geometry::Node {
                id,
                size: geometry::Size::new(f64::from(text[id].size.x), f64::from(text[id].size.y)),
                program: None,
                block: id,
                frame_depth: None,
                boundary: card.boundary,
            })
            .collect(),
        edges: control
            .edges
            .iter()
            .enumerate()
            .map(|(id, edge)| {
                let measured = painter
                    .layout_no_wrap(edge.label.clone(), FontId::monospace(10.0), palette::MUTED)
                    .size();
                geometry::Edge {
                    id,
                    from: edge.from,
                    to: edge.to,
                    kind: layout::kind(edge.kind),
                    label: geometry::Label {
                        text: edge.label.clone(),
                        size: if edge.label.is_empty() {
                            geometry::Size::new(1.0, 1.0)
                        } else {
                            geometry::Size::new(
                                f64::from(measured.x + 8.0),
                                f64::from(measured.y + 4.0),
                            )
                        },
                    },
                }
            })
            .collect(),
    };
    layout::scene(&input)
}

pub(super) fn core_bounds(control: &Control, placement: &Placement) -> Rect {
    control
        .core
        .iter()
        .filter_map(|id| placement.nodes.get(id))
        .fold(Rect::NOTHING, |bounds, rect| bounds.union(*rect))
}

pub(super) fn fits(bounds: Rect, canvas: Vec2) -> bool {
    bounds.is_finite() && bounds.width() <= canvas.x - 16.0 && bounds.height() <= canvas.y - 16.0
}

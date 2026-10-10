//! Screen-space CFG edges, arrows and labels.

use evm_abstract_notation::Symbol;
mod geometry;
#[cfg(test)]
mod tests;

use super::layout::EdgeRoute;
use crate::palette;
use egui::{FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use evm_abstract_protocol::EdgeKind;

pub(super) fn edge_label(id: usize, kind: EdgeKind) -> String {
    format!(
        "{notation_0} {}",
        match kind {
            EdgeKind::BranchTrue => "true",
            EdgeKind::BranchFalse => "false",
            EdgeKind::Jump => "jump",
            EdgeKind::Fallthrough => "next",
            EdgeKind::Call => "call",
            EdgeKind::Return => "return",
            EdgeKind::Failure => "failure",
            EdgeKind::Revert => "revert",
        },
        notation_0 = Symbol::Edge(id)
    )
}

#[cfg(test)]
pub(super) fn paint_edge(
    painter: &Painter,
    route: &EdgeRoute,
    origin: Pos2,
    kind: EdgeKind,
    id: usize,
    zoom: f32,
    selected: bool,
) {
    paint_edge_text(
        painter,
        route,
        origin,
        kind,
        &edge_label(id, kind),
        zoom,
        selected,
    );
}

pub(super) fn paint_edge_text(
    painter: &Painter,
    route: &EdgeRoute,
    origin: Pos2,
    kind: EdgeKind,
    label_text: &str,
    zoom: f32,
    selected: bool,
) {
    let color = match kind {
        EdgeKind::BranchTrue | EdgeKind::Jump => palette::ACCENT,
        EdgeKind::BranchFalse => palette::WARNING,
        EdgeKind::Call | EdgeKind::Return => palette::PURPLE,
        EdgeKind::Failure | EdgeKind::Revert => palette::ERROR,
        EdgeKind::Fallthrough => palette::BLUE,
    };
    let stroke = Stroke::new(
        if selected { 1.6 } else { 1.0 },
        if selected {
            color
        } else {
            color.gamma_multiply(0.65)
        },
    );
    let pixels_per_point = painter.pixels_per_point();
    // Unlike individual LineSegments, epaint does not round PathShape centers
    // to the physical pixel grid. Apply the same stroke-aware rounding here so
    // orthogonal segments retain equal coverage as the graph moves or scales.
    let points = geometry::screen_points(&route.path, origin, zoom, stroke, pixels_per_point);
    if points.len() < 2 {
        return;
    }
    painter.add(Shape::line(points.clone(), stroke));
    let end = points[points.len() - 1];
    let direction = (end - points[points.len() - 2]).normalized();
    let perpendicular = Vec2::new(-direction.y, direction.x);
    let size = (6.0 * zoom).max(2.0);
    painter.add(Shape::convex_polygon(
        vec![
            end,
            end - direction * size + perpendicular * size * 0.5,
            end - direction * size - perpendicular * size * 0.5,
        ],
        stroke.color,
        Stroke::NONE,
    ));
    // ELK jointly places the route and its annotation. The label may sit beside
    // a segment; drawing it must never add bends or break the original path.
    if zoom > 0.35 && !label_text.is_empty() {
        let label = Rect::from_min_max(
            origin + route.label.min.to_vec2() * zoom,
            origin + route.label.max.to_vec2() * zoom,
        );
        painter.rect_filled(label, 2.0, palette::BACKGROUND);
        let text_color = palette::MUTED.gamma_multiply(0.8);
        let galley = painter.layout_job(crate::notation::job(
            label_text,
            FontId::monospace(9.0 * zoom),
            text_color,
        ));
        let position = label.center() - galley.size() * 0.5;
        painter
            .with_clip_rect(label.intersect(painter.clip_rect()))
            .galley(position, galley, text_color);
    }
}

//! Screen-space CFG edges, arrows and labels.

mod geometry;
#[cfg(test)]
mod tests;

use super::layout::EdgeRoute;
use crate::palette;
use egui::{FontId, Painter, Pos2, Rect, Shape, Stroke, Vec2};
use evm_abstract_protocol::EdgeKind;

pub(super) fn edge_label(id: usize, kind: EdgeKind) -> String {
    format!(
        "e{id} {}",
        match kind {
            EdgeKind::BranchTrue => "true",
            EdgeKind::BranchFalse => "false",
            EdgeKind::Jump => "jump",
            EdgeKind::Fallthrough => "next",
            EdgeKind::Call => "call",
            EdgeKind::Return => "return",
            EdgeKind::Failure => "failure",
            EdgeKind::Revert => "revert",
        }
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
    let incoming = geometry::screen_points(&route.to_label, origin, zoom, stroke, pixels_per_point);
    let label_entry = incoming.last().copied();
    if incoming.len() >= 2 {
        painter.add(Shape::line(incoming, stroke));
    }
    let points = geometry::screen_points(&route.to_target, origin, zoom, stroke, pixels_per_point);
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
    // The label is a layout vertex, not a floating mask over an edge. The two
    // routes end/start at its boundary and only the target gets an arrowhead.
    let label = Rect::from_min_max(
        origin + route.label.min.to_vec2() * zoom,
        origin + route.label.max.to_vec2() * zoom,
    );
    // Preserve the routing gap without making each annotation look like a
    // selected state or button. Edge kind remains visible in the colored lines.
    painter.rect_filled(label, 2.0, palette::BACKGROUND);
    if zoom > 0.35 {
        let text_color = palette::MUTED.gamma_multiply(0.8);
        let galley =
            painter.layout_no_wrap(label_text.into(), FontId::monospace(9.0 * zoom), text_color);
        let position = label.center() - galley.size() * 0.5;
        painter
            .with_clip_rect(label.intersect(painter.clip_rect()))
            .galley(position, galley, text_color);
    } else if let Some(entry) = label_entry {
        // With neither text nor an outline, retain a continuous edge through
        // the reserved slot instead of leaving an unexplained gap at low zoom.
        painter.line_segment([entry, points[0]], stroke);
    }
}

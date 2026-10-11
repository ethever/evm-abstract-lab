//! Moving arrowheads follow the screen-space route's source-to-target order.
//! The layout, original stroke, endpoint arrow and annotation remain immutable.

#[cfg(test)]
mod tests;

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, Vec2};

const SPEED: f64 = 60.0;
const SPACING: f64 = 56.0;

/// Paint only the visible part of a highlighted route and report whether its
/// actual segments intersect the clip. Bounding boxes and labels alone must
/// not keep an offscreen edge's animation repainting.
pub(super) fn paint(painter: &Painter, points: &[Pos2], color: Color32, zoom: f32) -> bool {
    let total: f64 = points
        .windows(2)
        .map(|pair| segment_delta(pair[0], pair[1]))
        .map(|delta| delta[0].hypot(delta[1]))
        .sum();
    if !total.is_finite() || total <= 0.0 {
        return false;
    }
    let size = (7.0 * zoom).clamp(4.5, 10.0);
    let clip = painter.clip_rect();
    let spacing = SPACING.min(total);
    // At 30 fps, very short routes need a slower cycle to retain a visibly
    // forward motion instead of sampling almost a complete period per frame.
    let speed = SPEED.min(spacing * 10.0);
    // Reduce the f64 clock before converting to screen coordinates so long
    // sessions retain the same smooth phase as a freshly opened graph.
    let phase = painter
        .ctx()
        .input(|input| input.time.rem_euclid(spacing / speed) * speed);
    let color = color.lerp_to_gamma(Color32::WHITE, 0.7);
    let mut offset = 0.0;
    let mut visible = false;
    for pair in points.windows(2) {
        let delta = segment_delta(pair[0], pair[1]);
        let length = delta[0].hypot(delta[1]);
        if length <= 0.0 {
            continue;
        }
        visible |= clip_segment(pair[0], pair[1], clip).is_some();
        if let Some((from, to)) = clip_segment(pair[0], pair[1], clip.expand(size)) {
            let first = offset + from * length;
            let visible_length = (to - from) * length;
            let remaining = (1.0 - from) * length;
            let clipped_start = [
                f64::from(pair[0].x) + delta[0] * from,
                f64::from(pair[0].y) + delta[1] * from,
            ];
            // Keep phase and marker steps local to the visible interval. An
            // absolute f32 arc at a billion points would swallow every tick;
            // even the final tip interpolation must happen before narrowing.
            let mut distance = (phase - first.rem_euclid(spacing)).rem_euclid(spacing);
            let tangent = [delta[0] / length, delta[1] / length];
            let direction = Vec2::new(tangent[0] as f32, tangent[1] as f32);
            let perpendicular = Vec2::new(-direction.y, direction.x);
            // The next segment owns a bend exactly on its start, which gives
            // that marker the outgoing direction without drawing it twice.
            while distance <= visible_length && distance < remaining {
                let tip = Pos2::new(
                    (clipped_start[0] + tangent[0] * distance) as f32,
                    (clipped_start[1] + tangent[1] * distance) as f32,
                );
                painter.add(Shape::convex_polygon(
                    vec![
                        tip,
                        tip - direction * size + perpendicular * size * 0.45,
                        tip - direction * size - perpendicular * size * 0.45,
                    ],
                    color,
                    Stroke::NONE,
                ));
                let next = distance + spacing;
                if next <= distance {
                    break;
                }
                distance = next;
            }
        }
        offset += length;
    }
    visible
}

/// Parametric interval of a segment inside a rectangle, retaining route order
/// for leftward/upward segments and handling horizontal/vertical lines exactly.
fn clip_segment(start: Pos2, end: Pos2, clip: Rect) -> Option<(f64, f64)> {
    let delta = segment_delta(start, end);
    let mut from = 0.0_f64;
    let mut to = 1.0_f64;
    for axis in 0..2 {
        if delta[axis] == 0.0 {
            if start[axis] < clip.min[axis] || start[axis] > clip.max[axis] {
                return None;
            }
        } else {
            let a = (f64::from(clip.min[axis]) - f64::from(start[axis])) / delta[axis];
            let b = (f64::from(clip.max[axis]) - f64::from(start[axis])) / delta[axis];
            from = from.max(a.min(b));
            to = to.min(a.max(b));
            if from > to {
                return None;
            }
        }
    }
    Some((from, to))
}

fn segment_delta(start: Pos2, end: Pos2) -> [f64; 2] {
    [
        f64::from(end.x) - f64::from(start.x),
        f64::from(end.y) - f64::from(start.y),
    ]
}

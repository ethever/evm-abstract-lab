//! Apply the physical-pixel rules that epaint applies to individual orthogonal
//! line segments, while retaining a single joined path for each CFG edge.

use egui::{Pos2, Rect, Stroke, Vec2};

pub(super) fn screen_points(
    source: &[Pos2],
    origin: Pos2,
    zoom: f32,
    stroke: Stroke,
    pixels_per_point: f32,
) -> Vec<Pos2> {
    let mut points: Vec<_> = source
        .iter()
        .map(|point| {
            let mut screen = origin + point.to_vec2() * zoom;
            stroke.round_center_to_pixel(pixels_per_point, &mut screen.x);
            stroke.round_center_to_pixel(pixels_per_point, &mut screen.y);
            screen
        })
        .collect();
    // A short elbow can collapse to one physical pixel after zooming out.
    // Keeping duplicate vertices would make its normal or arrow direction zero.
    points.dedup();
    points
}

pub(super) fn label_rect(
    desired: Rect,
    points: &[Pos2],
    stroke: Stroke,
    pixels_per_point: f32,
) -> Rect {
    // Include the stroke's antialias fringe, half-pixel rectangle rounding,
    // and a half-pixel gap. This is a screen-space clearance, not a zoom-scaled
    // font-size estimate: the real galley's height determines the mask's size.
    let clearance = stroke.width * 0.5 + 1.5 / pixels_per_point;
    let obstacles: Vec<_> = points
        .windows(2)
        .map(|segment| Rect::from_two_pos(segment[0], segment[1]).expand(clearance))
        .collect();
    let mut ys = vec![desired.top()];
    for blocked in &obstacles {
        ys.extend([blocked.top() - desired.height(), blocked.bottom()]);
    }
    let mut best = desired;
    let mut distance = f32::INFINITY;
    // Retain the layout's horizontal label lane. In left-to-right layouts it
    // is the reserved space between node columns; moving sideways to avoid a
    // vertical elbow could put the label over a node. Obstacle boundaries give
    // the nearest clear vertical position without changing that lane.
    for y in ys {
        let candidate = Rect::from_min_size(Pos2::new(desired.left(), y), desired.size());
        let delta: Vec2 = candidate.min - desired.min;
        if delta.length_sq() < distance
            && obstacles
                .iter()
                .all(|blocked| !overlaps(candidate, *blocked))
        {
            best = candidate;
            distance = delta.length_sq();
        }
    }
    best
}

fn overlaps(first: Rect, second: Rect) -> bool {
    let overlap = first.intersect(second);
    overlap.width() > 0.0 && overlap.height() > 0.0
}

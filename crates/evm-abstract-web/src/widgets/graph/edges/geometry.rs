//! Apply the physical-pixel rules that epaint applies to individual orthogonal
//! line segments, while retaining a single joined path for each CFG edge.

use egui::{Pos2, Stroke};

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

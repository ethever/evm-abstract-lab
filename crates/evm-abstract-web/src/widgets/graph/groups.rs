//! Spatial annotations never become selectable analysis nodes or edge targets.

use egui::{Align2, FontId, Painter, Rect, Sense, Shape, Stroke, Ui, Vec2};
use evm_abstract_layout::GroupKind;

use super::layout::Group;
use crate::palette;

#[cfg(test)]
mod tests;

const DASH: f64 = 4.0;
const PERIOD: f64 = 8.0;

pub(super) fn paint(
    ui: &mut Ui,
    painter: &Painter,
    groups: &[Group],
    canvas: Rect,
    pan: Vec2,
    zoom: f32,
    visible: Rect,
) {
    let transform = |rect: Rect| {
        Rect::from_min_max(
            canvas.min + pan + rect.min.to_vec2() * zoom,
            canvas.min + pan + rect.max.to_vec2() * zoom,
        )
    };
    // Parents are emitted before children, behind the unchanged real edges and
    // cards. No fill, so the grouping reads as an annotation rather than a state.
    for group in groups {
        if !group.bounds.intersects(visible) && !group.caption.intersects(visible) {
            continue;
        }
        frame(painter, transform(group.bounds), canvas);
        let caption = transform(group.caption);
        if caption.intersects(canvas) {
            crate::notation::paint(
                &painter.with_clip_rect(caption.intersect(canvas)),
                caption.min,
                Align2::LEFT_TOP,
                &group.title,
                FontId::monospace(10.0 * zoom),
                palette::MUTED,
            );
            let kind = match group.kind {
                GroupKind::Program(_) => "Program",
                GroupKind::Chain => "Sequence",
                GroupKind::Cycle => "Cycle",
            };
            ui.interact(
                caption.intersect(canvas),
                ui.id().with(("cfg_spatial_group", group.id.0)),
                Sense::hover(),
            )
            .on_hover_text(format!(
                "{kind} spatial group: {} displayed cards. Each card retains its own native states and SSA; this frame does not prove a composable execution path.",
                group.members.len()
            ));
        }
    }
}

/// Clip the four real sides before expanding dashes. A long chain can surround
/// the viewport while every side lies offscreen; its full perimeter must never
/// turn into tens of thousands of invisible shapes on each painted frame.
fn frame(painter: &Painter, rect: Rect, canvas: Rect) {
    let stroke = Stroke::new(1.0, palette::BORDER.gamma_multiply(0.65));
    let clip = canvas.intersect(painter.clip_rect());
    if !clip.is_positive() || !rect.is_finite() {
        return;
    }
    // Retain the stroke and its antialiasing fringe at the canvas boundary.
    let clip = clip.expand(stroke.width * 0.5 + painter.pixels_per_point().recip());
    let points = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    let mut distance = 0.0;
    let mut shapes = Vec::new();
    for pair in points.windows(2) {
        let [from, to] = [pair[0], pair[1]];
        let axis = usize::from(from.x == to.x);
        let other = 1 - axis;
        let length = (f64::from(to[axis]) - f64::from(from[axis])).abs();
        if from[other] >= clip.min[other] && from[other] <= clip.max[other] {
            let low = from[axis].min(to[axis]).max(clip.min[axis]);
            let high = from[axis].max(to[axis]).min(clip.max[axis]);
            if high > low {
                let forward = to[axis] > from[axis];
                let start = if forward { low } else { high };
                let direction = if forward { 1.0 } else { -1.0 };
                let visible_length = f64::from(high) - f64::from(low);
                let skipped = (f64::from(start) - f64::from(from[axis])).abs();
                let phase = (distance + skipped).rem_euclid(PERIOD);
                // Work relative to the visible fragment. Adding 8 to a huge
                // offscreen coordinate could otherwise stop advancing at all.
                let mut at = -phase;
                while at < visible_length {
                    let begin = at.max(0.0);
                    let end = (at + DASH).min(visible_length);
                    if end > begin {
                        let mut a = from;
                        let mut b = from;
                        a[axis] = start + direction * begin as f32;
                        b[axis] = start + direction * end as f32;
                        shapes.push(Shape::line_segment([a, b], stroke));
                    }
                    at += PERIOD;
                }
            }
        }
        distance += length;
    }
    painter.extend(shapes);
}

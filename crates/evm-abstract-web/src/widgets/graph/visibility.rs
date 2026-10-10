//! Immutable, linear-space viewport index for a completed layout. Long edge
//! bounds occupy one interval per axis rather than many spatial grid cells.

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use egui::Rect;

#[derive(Default)]
pub(super) struct Visibility {
    rectangles: Vec<(usize, Rect)>,
    horizontal: Intervals,
    vertical: Intervals,
}

impl Visibility {
    pub(super) fn new(rectangles: &BTreeMap<usize, Rect>) -> Self {
        let rectangles: Vec<_> = rectangles
            .iter()
            .filter(|(_, rect)| valid(**rect))
            .map(|(id, rect)| (*id, *rect))
            .collect();
        Self {
            horizontal: Intervals::new(&rectangles, 0),
            vertical: Intervals::new(&rectangles, 1),
            rectangles,
        }
    }

    /// Native/display IDs in stable ID order. Touching and zero-area bounds
    /// count as intersections, matching egui's clipping geometry.
    pub(super) fn query(&self, view: Rect) -> Vec<usize> {
        if !valid(view) {
            return Vec::new();
        }
        let horizontal = self.horizontal.candidates(view.min.x, view.max.x);
        let vertical = self.vertical.candidates(view.min.y, view.max.y);
        let candidates = if horizontal.len() <= vertical.len() {
            horizontal
        } else {
            vertical
        };
        let mut visible: Vec<_> = candidates
            .iter()
            .filter_map(|entry| {
                let (id, rect) = self.rectangles[entry.position];
                rect.intersects(view).then_some(id)
            })
            .collect();
        // Selecting the cheaper axis must not change overlapping paint order.
        visible.sort_unstable();
        visible
    }
}

struct Interval {
    min: f32,
    position: usize,
}

#[derive(Default)]
struct Intervals {
    entries: Vec<Interval>,
    prefix_max: Vec<f32>,
}

impl Intervals {
    fn new(rectangles: &[(usize, Rect)], axis: usize) -> Self {
        let mut entries: Vec<_> = rectangles
            .iter()
            .enumerate()
            .map(|(position, (_, rect))| Interval {
                min: rect.min[axis],
                position,
            })
            .collect();
        entries.sort_unstable_by(|left, right| {
            left.min
                .total_cmp(&right.min)
                .then_with(|| left.position.cmp(&right.position))
        });
        let mut max = f32::NEG_INFINITY;
        let prefix_max = entries
            .iter()
            .map(|entry| {
                max = max.max(rectangles[entry.position].1.max[axis]);
                max
            })
            .collect();
        Self {
            entries,
            prefix_max,
        }
    }

    fn candidates(&self, min: f32, max: f32) -> &[Interval] {
        let start = self.prefix_max.partition_point(|prefix| *prefix < min);
        let end = self.entries.partition_point(|entry| entry.min <= max);
        &self.entries[start..end]
    }
}

fn valid(rect: Rect) -> bool {
    rect.is_finite() && rect.min.x <= rect.max.x && rect.min.y <= rect.max.y
}

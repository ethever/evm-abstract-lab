//! Narrow-phase geometric culling after the cached route-bounds index. Visible
//! crossing routes remain continuous even when both endpoint cards are offscreen.
use super::EdgeRoute;
use egui::Rect;

pub(in crate::widgets::graph) fn has_visible_geometry(
    route: &EdgeRoute,
    viewport: Rect,
    label_visible: bool,
) -> bool {
    route
        .path
        .windows(2)
        .any(|points| Rect::from_two_pos(points[0], points[1]).intersects(viewport))
        || (label_visible && route.label.intersects(viewport))
}

#[cfg(test)]
mod tests {
    use super::{EdgeRoute, has_visible_geometry};
    use egui::{Pos2, Rect, Vec2};

    #[test]
    fn a_visible_crossing_is_retained_with_both_endpoints_offscreen() {
        let route = EdgeRoute {
            path: vec![Pos2::new(-200.0, 80.0), Pos2::new(400.0, 80.0)],
            label: Rect::NOTHING,
        };
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0));
        assert!(route.path.iter().all(|point| !view.contains(*point)));
        assert!(has_visible_geometry(&route, view, false));
    }

    #[test]
    fn fully_offscreen_segments_are_culled_and_visible_annotations_remain_available() {
        let mut route = EdgeRoute {
            path: vec![Pos2::new(-200.0, -100.0), Pos2::new(400.0, -100.0)],
            label: Rect::NOTHING,
        };
        let view = Rect::from_min_size(Pos2::ZERO, Vec2::new(300.0, 200.0));
        assert!(!has_visible_geometry(&route, view, false));
        route.label = Rect::from_min_size(Pos2::new(140.0, 80.0), Vec2::new(32.0, 16.0));
        assert!(has_visible_geometry(&route, view, true));
        assert!(!has_visible_geometry(&route, view, false));
    }
}

use std::collections::BTreeMap;

use egui::{Pos2, Rect};

use super::Visibility;

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> Rect {
    Rect::from_min_max(Pos2::new(left, top), Pos2::new(right, bottom))
}

#[test]
fn sparse_ids_negative_coordinates_long_edges_and_flat_bounds_remain_visible() {
    let bounds = BTreeMap::from([
        (7, rect(-50.0, -50.0, -20.0, -20.0)),
        (900, rect(-10000.0, 5.0, 10000.0, 5.0)),
        (20, rect(10.0, -500.0, 10.0, 500.0)),
        (usize::MAX, rect(5.0, 5.0, 5.0, 5.0)),
        (0, rect(0.0, 0.0, 10.0, 10.0)),
    ]);
    let index = Visibility::new(&bounds);
    assert_eq!(index.query(rect(4.0, 4.0, 6.0, 6.0)), [0, 900, usize::MAX]);
    assert_eq!(index.query(rect(10.0, 10.0, 20.0, 20.0)), [0, 20]);
    assert_eq!(index.query(rect(-45.0, -40.0, -40.0, -30.0)), [7]);
    assert_eq!(index.query(rect(5.0, 5.0, 5.0, 5.0)), [0, 900, usize::MAX]);
    assert_eq!(index.query(rect(10.0, -10.0, 10.0, 10.0)), [0, 20, 900]);
    assert!(
        index
            .query(rect(20000.0, 20000.0, 20001.0, 20001.0))
            .is_empty()
    );
}

#[test]
fn invalid_rectangles_and_views_are_filtered_without_indexing_panics() {
    let bounds = BTreeMap::from([
        (0, Rect::NOTHING),
        (1, Rect::EVERYTHING),
        (2, rect(f32::NAN, 0.0, 2.0, 2.0)),
        (3, rect(10.0, 0.0, -10.0, 20.0)),
        (4, rect(0.0, 0.0, 2.0, 2.0)),
    ]);
    let index = Visibility::new(&bounds);
    assert_eq!(index.query(rect(-10.0, -10.0, 10.0, 10.0)), [4]);
    for invalid in [
        Rect::NOTHING,
        Rect::EVERYTHING,
        rect(0.0, f32::NAN, 1.0, 1.0),
        rect(0.0, 2.0, 1.0, 1.0),
    ] {
        assert!(index.query(invalid).is_empty());
    }
    assert!(
        Visibility::default()
            .query(rect(0.0, 0.0, 1.0, 1.0))
            .is_empty()
    );
    assert!(
        Visibility::new(&BTreeMap::new())
            .query(rect(-1.0, -1.0, 1.0, 1.0))
            .is_empty()
    );
}

#[test]
fn both_axis_queries_match_brute_force_for_nested_disjoint_and_crossing_boxes() {
    let mut bounds = BTreeMap::new();
    for item in 0..300 {
        let left = ((item * 41) % 499) as f32 - 250.0;
        let top = ((item * 73) % 487) as f32 - 250.0;
        let width = (item % 13) as f32 * 17.0;
        let height = (item % 11) as f32 * 19.0;
        bounds.insert(item * 19 + 7, rect(left, top, left + width, top + height));
    }
    bounds.insert(90000, rect(-10000.0, -2.0, 10000.0, 2.0));
    bounds.insert(90001, rect(-2.0, -10000.0, 2.0, 10000.0));
    let index = Visibility::new(&bounds);
    for step in 0..160 {
        let left = ((step * 37) % 997) as f32 - 500.0;
        let top = ((step * 29) % 991) as f32 - 500.0;
        let view = rect(
            left,
            top,
            left + (step % 17) as f32 * 7.0,
            top + (step % 19) as f32 * 11.0,
        );
        let expected: Vec<_> = bounds
            .iter()
            .filter_map(|(id, rect)| rect.intersects(view).then_some(*id))
            .collect();
        assert_eq!(index.query(view), expected, "view {view:?}");
    }
}

#[test]
fn long_crossing_edges_use_the_selective_axis_without_expanding_storage() {
    let bounds: BTreeMap<_, _> = (0..1000)
        .map(|id| {
            (
                id * 71,
                rect(
                    -100000.0,
                    id as f32 * 10.0,
                    100000.0,
                    id as f32 * 10.0 + 1.0,
                ),
            )
        })
        .collect();
    let index = Visibility::new(&bounds);
    assert_eq!(index.rectangles.len(), 1000);
    assert_eq!(index.horizontal.entries.len(), 1000);
    assert_eq!(index.vertical.entries.len(), 1000);
    let view = rect(-1.0, 5000.0, 1.0, 5001.0);
    assert_eq!(
        index.horizontal.candidates(view.min.x, view.max.x).len(),
        1000
    );
    assert_eq!(index.vertical.candidates(view.min.y, view.max.y).len(), 1);
    assert_eq!(index.query(view), [500 * 71]);
}

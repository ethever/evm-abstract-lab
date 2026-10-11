use egui::epaint::PathShape;
use egui::{Color32, Context, LayerId, Painter, Pos2, RawInput, Rect, Shape, Stroke, Vec2};

use crate::palette;

struct Frame {
    arrows: Vec<PathShape>,
    visible: bool,
}

fn render(points: &[Pos2], time: f64, zoom: f32, clip: Rect) -> Frame {
    let ctx = Context::default();
    let mut visible = false;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0))),
            time: Some(time),
            ..RawInput::default()
        },
        |ui| {
            let painter = Painter::new(ui.ctx().clone(), LayerId::background(), clip);
            visible = super::paint(&painter, points, palette::BLUE, zoom);
        },
    );
    output.textures_delta.clear();
    let arrows = output
        .shapes
        .into_iter()
        .filter_map(|item| match item.shape {
            Shape::Path(path) if path.closed => Some(path),
            _ => None,
        })
        .collect();
    Frame { arrows, visible }
}

fn whole_frame(points: &[Pos2], time: f64) -> Frame {
    render(points, time, 1.0, Rect::EVERYTHING)
}

fn direction(arrow: &PathShape) -> Vec2 {
    let center = arrow.points[1].lerp(arrow.points[2], 0.5);
    (arrow.points[0] - center).normalized()
}

fn arrow_at(frame: &Frame, tip: Pos2, expected_direction: Vec2) {
    let arrow = frame
        .arrows
        .iter()
        .find(|arrow| arrow.points[0].distance(tip) < 0.001)
        .unwrap_or_else(|| panic!("missing flowing head at {tip:?}: {:?}", frame.arrows));
    assert!((direction(arrow) - expected_direction).length() < 0.001);
}

#[test]
fn moving_heads_follow_order_in_all_four_directions() {
    for expected_direction in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
        let start = Pos2::new(200.0, 200.0);
        let points = [start, start + expected_direction * 140.0];
        let before = whole_frame(&points, 0.1);
        let after = whole_frame(&points, 0.2);
        assert!(before.visible && after.visible);
        assert_eq!(before.arrows.len(), after.arrows.len());
        assert_eq!(before.arrows.len(), 3);
        for (before, after) in before.arrows.iter().zip(&after.arrows) {
            assert!((direction(after) - expected_direction).length() < 0.001);
            assert!(
                (after.points[0] - before.points[0] - expected_direction * 6.0).length() < 0.001
            );
            assert_eq!(after.fill, palette::BLUE.lerp_to_gamma(Color32::WHITE, 0.7));
        }
    }
}

#[test]
fn a_marker_crosses_an_elbow_instead_of_restarting_each_segment() {
    let points = [
        Pos2::new(10.0, 10.0),
        Pos2::new(40.0, 10.0),
        Pos2::new(40.0, 70.0),
        Pos2::new(90.0, 70.0),
    ];
    arrow_at(
        &whole_frame(&points, 26.0 / 60.0),
        Pos2::new(36.0, 10.0),
        Vec2::X,
    );
    arrow_at(
        &whole_frame(&points, 30.0 / 60.0),
        Pos2::new(40.0, 10.0),
        Vec2::Y,
    );
    arrow_at(
        &whole_frame(&points, 34.0 / 60.0),
        Pos2::new(40.0, 14.0),
        Vec2::Y,
    );
    // The second marker moves through the next elbow at the same phase.
    arrow_at(
        &whole_frame(&points, 30.0 / 60.0),
        Pos2::new(40.0, 66.0),
        Vec2::Y,
    );
    arrow_at(
        &whole_frame(&points, 34.0 / 60.0),
        Pos2::new(40.0, 70.0),
        Vec2::X,
    );
    arrow_at(
        &whole_frame(&points, 38.0 / 60.0),
        Pos2::new(44.0, 70.0),
        Vec2::X,
    );
}

#[test]
fn loop_and_reversed_back_edges_keep_their_actual_route_direction() {
    let loop_points = [
        Pos2::new(40.0, 40.0),
        Pos2::new(100.0, 40.0),
        Pos2::new(100.0, 100.0),
        Pos2::new(40.0, 100.0),
        Pos2::new(40.0, 40.0),
    ];
    let before = whole_frame(&loop_points, 14.0 / 60.0);
    let after = whole_frame(&loop_points, 20.0 / 60.0);
    arrow_at(&before, Pos2::new(54.0, 40.0), Vec2::X);
    arrow_at(&after, Pos2::new(60.0, 40.0), Vec2::X);
    arrow_at(&before, Pos2::new(94.0, 100.0), -Vec2::X);
    arrow_at(&after, Pos2::new(88.0, 100.0), -Vec2::X);
    arrow_at(&before, Pos2::new(40.0, 98.0), -Vec2::Y);
    arrow_at(&after, Pos2::new(40.0, 92.0), -Vec2::Y);
    let reversed: Vec<_> = loop_points.into_iter().rev().collect();
    arrow_at(&whole_frame(&reversed, 0.1), Pos2::new(40.0, 46.0), Vec2::Y);
}

#[test]
fn flow_stays_visible_and_finite_after_zoom_pixel_rounding_and_pan() {
    let route = [
        Pos2::new(10.0, 10.0),
        Pos2::new(10.1, 10.0),
        Pos2::new(10.1, 110.0),
        Pos2::new(210.0, 110.0),
    ];
    for density in [1.0, 2.0] {
        for zoom in [0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5] {
            let base = super::super::geometry::screen_points(
                &route,
                Pos2::new(100.25, 50.75),
                zoom,
                Stroke::new(1.6, palette::BLUE),
                density,
            );
            let pan = Vec2::new(30.0, -15.0);
            let panned: Vec<_> = base.iter().map(|point| *point + pan).collect();
            let before = render(&base, 0.11, zoom, Rect::EVERYTHING);
            let after = render(&panned, 0.11, zoom, Rect::EVERYTHING);
            assert!(!before.arrows.is_empty(), "DPR{density} zoom{zoom}");
            assert_eq!(before.arrows.len(), after.arrows.len());
            for (before, after) in before.arrows.iter().zip(after.arrows) {
                assert!((after.points[0] - before.points[0] - pan).length() < 0.001);
                assert!(after.points.iter().all(|point| point.is_finite()));
                let length = after.points[0].distance(after.points[1].lerp(after.points[2], 0.5));
                assert!((4.499..=10.001).contains(&length));
            }
        }
    }
}

#[test]
fn degenerate_routes_and_false_bounding_box_visibility_do_not_animate() {
    let clip = Rect::from_min_max(Pos2::new(40.0, 40.0), Pos2::new(60.0, 60.0));
    let around_clip = [
        Pos2::new(10.0, 10.0),
        Pos2::new(90.0, 10.0),
        Pos2::new(90.0, 90.0),
        Pos2::new(10.0, 90.0),
    ];
    let outside = render(&around_clip, 0.1, 1.0, clip);
    assert!(!outside.visible);
    assert!(outside.arrows.is_empty());
    let crossed = render(
        &[Pos2::new(10.0, 50.0), Pos2::new(90.0, 50.0)],
        0.1,
        1.0,
        clip,
    );
    assert!(crossed.visible);
    for points in [vec![], vec![Pos2::ZERO], vec![Pos2::ZERO, Pos2::ZERO]] {
        let frame = render(&points, 0.1, 1.0, clip);
        assert!(!frame.visible);
        assert!(frame.arrows.is_empty());
    }
}

#[test]
fn clipping_skips_long_offscreen_runs_without_resetting_the_phase() {
    let points = [Pos2::new(-1_000_000.0, 50.0), Pos2::new(1_000_000.0, 50.0)];
    let clip = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
    let frame = render(&points, 0.1, 1.0, clip);
    assert!(frame.visible);
    assert!((1..=3).contains(&frame.arrows.len()));
    let all = whole_frame(&[Pos2::new(-64.0, 50.0), Pos2::new(100.0, 50.0)], 0.1);
    for arrow in &frame.arrows {
        assert!(
            all.arrows
                .iter()
                .any(|other| other.points[0] == arrow.points[0])
        );
        assert!((direction(arrow) - Vec2::X).length() < 0.001);
    }
}

#[test]
fn billion_point_routes_keep_visible_heads_moving_between_frames() {
    let clip = Rect::from_min_max(Pos2::ZERO, Pos2::new(100.0, 100.0));
    for extent in [1_000_000_000.0, 2_000_000_000.0] {
        for expected_direction in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            let center = Pos2::new(50.0, 50.0);
            let points = [
                center - expected_direction * extent,
                center + expected_direction * extent,
            ];
            let mut previous: Option<Pos2> = None;
            for index in 0..7 {
                let frame = render(&points, 0.1 + f64::from(index) * 0.033, 1.0, clip);
                assert!(frame.visible);
                assert!(
                    (1..=3).contains(&frame.arrows.len()),
                    "clipping bounds paint cost"
                );
                let tips: Vec<_> = frame.arrows.iter().map(|arrow| arrow.points[0]).collect();
                assert!(
                    tips.iter().any(|tip| clip.contains(*tip)),
                    "flow must enter the clip"
                );
                if let Some(previous) = previous {
                    // Wrapping through the next repeated head retains the
                    // same 1.98-point forward phase at every visible frame.
                    let displacement = (tips[0] - previous)
                        .dot(expected_direction)
                        .rem_euclid(56.0);
                    assert!(
                        (displacement - 1.98).abs() < 0.001,
                        "extent{extent} frame{index}: {displacement}"
                    );
                }
                previous = Some(tips[0]);
            }
        }
    }
}

#[test]
fn one_and_two_point_routes_keep_a_forward_phase_at_thirty_fps() {
    for length in [1.0, 2.0] {
        for expected_direction in [Vec2::X, -Vec2::X, Vec2::Y, -Vec2::Y] {
            let start = Pos2::new(50.0, 50.0);
            let points = [start, start + expected_direction * length];
            let mut previous: Option<Pos2> = None;
            for index in 0..7 {
                let frame = render(
                    &points,
                    0.1 + f64::from(index) * 0.033,
                    0.01,
                    Rect::EVERYTHING,
                );
                assert_eq!(frame.arrows.len(), 1, "short visible edges retain a head");
                let tip = frame.arrows[0].points[0];
                if let Some(previous) = previous {
                    let displacement = (tip - previous).dot(expected_direction).rem_euclid(length);
                    assert!((displacement - length * 0.33).abs() < 0.001);
                    assert!(
                        displacement < length * 0.5,
                        "sampling cannot alias into reverse motion"
                    );
                }
                previous = Some(tip);
            }
        }
    }
}

#[test]
fn a_long_running_clock_retains_subframe_forward_motion() {
    let points = [Pos2::new(10.0, 50.0), Pos2::new(150.0, 50.0)];
    let before = whole_frame(&points, 1_000_000_000.1);
    let after = whole_frame(&points, 1_000_000_000.133);
    let displacement =
        (after.arrows[0].points[0].x - before.arrows[0].points[0].x).rem_euclid(56.0);
    assert!((displacement - 1.98).abs() < 0.001);
}

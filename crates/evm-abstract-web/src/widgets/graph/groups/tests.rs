use egui::{Context, Pos2, RawInput, Rect, Shape, Stroke, Vec2, ViewportId};
use evm_abstract_layout::{GroupId, GroupKind};

use super::{Group, frame, paint};
use crate::palette;

fn segments(output: &egui::FullOutput) -> Vec<[Pos2; 2]> {
    output
        .shapes
        .iter()
        .filter_map(|shape| match shape.shape {
            Shape::LineSegment { points, .. } => Some(points),
            _ => None,
        })
        .collect()
}

fn input(canvas: Rect, density: f32) -> RawInput {
    let mut input = RawInput {
        screen_rect: Some(canvas),
        ..RawInput::default()
    };
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(density);
    input
}

fn render_group(bounds: Rect, canvas: Rect, zoom: f32, density: f32) -> Vec<[Pos2; 2]> {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let group = Group {
        id: GroupId(3),
        parent: Some(GroupId(0)),
        kind: GroupKind::Chain,
        members: (0..1200).collect(),
        bounds,
        caption: Rect::from_min_size(bounds.min + Vec2::splat(6.0), Vec2::new(60.0, 16.0)),
        title: "Chain".into(),
    };
    let frozen = group.clone();
    let visible = Rect::from_min_size(Pos2::ZERO, canvas.size() / zoom);
    let mut output = ctx.run_ui(input(canvas, density), |ui| {
        paint(
            ui,
            &ui.painter().with_clip_rect(canvas),
            std::slice::from_ref(&group),
            canvas,
            Vec2::ZERO,
            zoom,
            visible,
        );
    });
    output.textures_delta.clear();
    assert_eq!(
        group, frozen,
        "clipping must leave group geometry and members intact"
    );
    segments(&output)
}

#[test]
fn a_giant_container_enclosing_the_viewport_allocates_no_invisible_dashes() {
    let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
    for density in [1.0, 2.0] {
        for zoom in [0.5, 1.0, 2.5] {
            let bounds =
                Rect::from_min_max(Pos2::new(-100.0, -100.0), Pos2::new(2000.0, 200_000.0));
            assert!(bounds.intersects(Rect::from_min_size(Pos2::ZERO, canvas.size() / zoom)));
            assert!(render_group(bounds, canvas, zoom, density).is_empty());
        }
    }
}

#[test]
fn a_long_chain_frame_allocates_only_the_visible_sides_at_every_zoom() {
    let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
    for density in [1.0, 2.0] {
        for zoom in [0.5, 1.0, 2.5] {
            for height in [200_000.0, 20_000_000.0] {
                let bounds = Rect::from_min_max(Pos2::new(20.0, -100.0), Pos2::new(320.0, height));
                let dashes = render_group(bounds, canvas, zoom, density);
                assert!(
                    !dashes.is_empty(),
                    "both real vertical sides remain visible"
                );
                assert!(
                    dashes.len() <= 156,
                    "offscreen height allocated {} dashes",
                    dashes.len()
                );
                for points in dashes {
                    assert!(points[0].x == 20.0 * zoom || points[0].x == 320.0 * zoom);
                    assert_eq!(
                        points[0].x, points[1].x,
                        "clipping cannot invent a viewport border"
                    );
                    assert!(
                        points
                            .iter()
                            .all(|point| point.y >= -1.5 && point.y <= 601.5)
                    );
                }
            }
        }
    }
}

#[test]
fn clipped_dashes_preserve_the_original_perimeter_phase_and_corner_continuity() {
    let world = Rect::from_min_max(Pos2::new(20.3, 30.7), Pos2::new(413.2, 271.9));
    for density in [1.0, 2.0] {
        for zoom in [0.2, 0.75, 1.0, 2.5] {
            for canvas in [
                Rect::from_min_max(Pos2::ZERO, Pos2::new(500.0, 320.0)),
                Rect::from_min_max(Pos2::new(0.0, 80.4), Pos2::new(280.2, 300.0)),
                Rect::from_min_max(Pos2::new(180.6, 0.0), Pos2::new(470.0, 160.8)),
                Rect::from_min_max(Pos2::new(120.0, 80.0), Pos2::new(300.0, 200.0)),
            ] {
                let ctx = Context::default();
                let rect = Rect::from_min_max(
                    Pos2::ZERO + world.min.to_vec2() * zoom + Vec2::new(-71.3, 26.7),
                    Pos2::ZERO + world.max.to_vec2() * zoom + Vec2::new(-71.3, 26.7),
                );
                let mut actual_clip = Rect::NOTHING;
                let mut output = ctx.run_ui(input(canvas, density), |ui| {
                    let painter = ui.painter().with_clip_rect(canvas);
                    actual_clip = painter
                        .clip_rect()
                        .intersect(canvas)
                        .expand(0.5 + density.recip());
                    frame(&painter, rect, canvas);
                });
                output.textures_delta.clear();
                let actual = segments(&output);
                let path = [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                    rect.left_top(),
                ];
                let expected: Vec<_> = Shape::dashed_line(
                    &path,
                    Stroke::new(1.0, palette::BORDER.gamma_multiply(0.65)),
                    4.0,
                    4.0,
                )
                .into_iter()
                .filter_map(|shape| match shape {
                    Shape::LineSegment { points, .. }
                        if Rect::from_two_pos(points[0], points[1]).intersects(actual_clip) =>
                    {
                        let clipped = [actual_clip.clamp(points[0]), actual_clip.clamp(points[1])];
                        (clipped[0] != clipped[1]).then_some(clipped)
                    }
                    _ => None,
                })
                .collect();
                assert_eq!(
                    actual.len(),
                    expected.len(),
                    "DPR{density} zoom{zoom} canvas{canvas:?}"
                );
                for (actual, expected) in actual.iter().zip(expected) {
                    for (actual, expected) in actual.iter().zip(expected) {
                        assert!(
                            (*actual - expected).length() < 0.001,
                            "phase changed: {actual:?} vs {expected:?}, DPR{density} zoom{zoom}"
                        );
                    }
                }
            }
        }
    }
}

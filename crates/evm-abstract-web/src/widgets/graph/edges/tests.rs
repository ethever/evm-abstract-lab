use egui::epaint::{PathShape, TessellationOptions, Tessellator};
use egui::{Context, Mesh, Pos2, RawInput, Rect, Shape, Vec2, ViewportId};
use evm_abstract_protocol::EdgeKind;

use super::{EdgeRoute, paint_edge};
use crate::palette;

const ZOOMS: [f32; 6] = [1.0, 0.75, 0.5, 0.36, 0.25, 0.1];

fn render(
    zoom: f32,
    density: f32,
    selected: bool,
    offset: f32,
) -> (PathShape, Option<Rect>, usize) {
    render_at_label(zoom, density, selected, offset, Pos2::new(45.0, 8.0))
}

fn render_at_label(
    zoom: f32,
    density: f32,
    selected: bool,
    offset: f32,
    label: Pos2,
) -> (PathShape, Option<Rect>, usize) {
    let ctx = Context::default();
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0))),
        ..RawInput::default()
    };
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(density);
    let route = EdgeRoute {
        points: vec![
            Pos2::new(20.0, 20.0),
            Pos2::new(120.0, 20.0),
            Pos2::new(120.0, 120.0),
            Pos2::new(220.0, 120.0),
        ],
        label,
    };
    let mut output = ctx.run_ui(input, |ui| {
        paint_edge(
            ui.painter(),
            &route,
            Pos2::new(20.19 + offset, 20.71 + offset),
            EdgeKind::Fallthrough,
            0,
            zoom,
            selected,
        );
    });
    output.textures_delta.clear();
    let path = output
        .shapes
        .iter()
        .find_map(|item| match &item.shape {
            Shape::Path(path) if !path.closed => Some(path.clone()),
            _ => None,
        })
        .unwrap();
    let arrow = output
        .shapes
        .iter()
        .find_map(|item| match &item.shape {
            Shape::Path(path) if path.closed => Some(path),
            _ => None,
        })
        .expect("every visible route keeps its arrowhead");
    assert_eq!(arrow.points.len(), 3);
    assert_eq!(arrow.points[0], *path.points.last().unwrap());
    assert!(arrow.points.iter().all(|point| point.is_finite()));
    let side_a = arrow.points[1] - arrow.points[0];
    let side_b = arrow.points[2] - arrow.points[0];
    assert!((side_a.x * side_b.y - side_a.y * side_b.x).abs() > 0.1);
    let mask = output.shapes.iter().find_map(|item| match &item.shape {
        Shape::Rect(rect) if rect.fill == palette::BACKGROUND => Some(rect.rect),
        _ => None,
    });
    let labels = output
        .shapes
        .iter()
        .filter(|item| matches!(&item.shape, Shape::Text(text) if text.galley.text() == "e0 next"))
        .count();
    (path, mask, labels)
}

fn alpha_at(mesh: &Mesh, point: Pos2) -> f32 {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .filter_map(|triangle| {
            let [a, b, c] = [
                mesh.vertices[triangle[0] as usize],
                mesh.vertices[triangle[1] as usize],
                mesh.vertices[triangle[2] as usize],
            ];
            let ab = b.pos - a.pos;
            let ac = c.pos - a.pos;
            let ap = point - a.pos;
            let determinant = ab.x * ac.y - ab.y * ac.x;
            if determinant.abs() < 0.000001 {
                return None;
            }
            let v = (ap.x * ac.y - ap.y * ac.x) / determinant;
            let w = (ab.x * ap.y - ab.y * ap.x) / determinant;
            let u = 1.0 - v - w;
            if u.min(v).min(w) < -0.0001 {
                return None;
            }
            Some(
                (u * f32::from(a.color.a())
                    + v * f32::from(b.color.a())
                    + w * f32::from(c.color.a()))
                    / 255.0,
            )
        })
        .fold(0.0, f32::max)
}

fn cross_section(mesh: &Mesh, segment: [Pos2; 2], density: f32) -> (f32, f32) {
    let horizontal = segment[0].y == segment[1].y;
    let center = segment[0].lerp(segment[1], 0.5);
    let axis = if horizontal { center.y } else { center.x };
    let base = (axis * density).floor() as i32;
    let mut peak = 0.0_f32;
    let mut total = 0.0;
    for pixel in base - 4..=base + 4 {
        let coordinate = (pixel as f32 + 0.5) / density;
        let point = if horizontal {
            Pos2::new(center.x, coordinate)
        } else {
            Pos2::new(coordinate, center.y)
        };
        let alpha = alpha_at(mesh, point);
        peak = peak.max(alpha);
        total += alpha;
    }
    (peak, total / density)
}

#[test]
fn tessellated_horizontal_and_vertical_edges_keep_equal_pixel_coverage() {
    for density in [1.0, 2.0] {
        for zoom in ZOOMS {
            for selected in [false, true] {
                for offset in [0.0, 0.2, 0.49, 0.75] {
                    let (path, _, _) = render(zoom, density, selected, offset);
                    let mut tessellator =
                        Tessellator::new(density, TessellationOptions::default(), [1, 1], vec![]);
                    let mut mesh = Mesh::default();
                    tessellator.tessellate_path(&path, &mut mesh);
                    let horizontal =
                        cross_section(&mesh, [path.points[0], path.points[1]], density);
                    let vertical = cross_section(&mesh, [path.points[1], path.points[2]], density);
                    assert!(
                        (horizontal.0 - vertical.0).abs() < 0.01,
                        "DPR{density} zoom{zoom} selected{selected} phase{offset}: peak alpha differs {horizontal:?} vs {vertical:?}"
                    );
                    let opacity = if selected {
                        1.0
                    } else {
                        f32::from(palette::BLUE.gamma_multiply(0.65).a()) / 255.0
                    };
                    let width = if selected { 1.6 } else { 1.0 };
                    assert!(
                        horizontal.0 > opacity * 0.75,
                        "stroke faded before its geometry became subpixel"
                    );
                    assert!((horizontal.1 - width * opacity).abs() < 0.025);
                    assert!((vertical.1 - width * opacity).abs() < 0.025);
                }
            }
        }
    }
}

#[test]
fn rendered_label_masks_do_not_erase_the_route_at_low_zoom() {
    for density in [1.0, 2.0] {
        for zoom in ZOOMS {
            for selected in [false, true] {
                for label in [Pos2::new(45.0, 8.0), Pos2::new(95.0, 58.0)] {
                    let (path, mask, labels) = render_at_label(zoom, density, selected, 0.0, label);
                    if zoom <= 0.35 {
                        assert_eq!(labels, 0);
                        continue;
                    }
                    assert_eq!(labels, 1, "readable labels must remain present");
                    let mask = mask.unwrap();
                    for segment in path.points.windows(2) {
                        let stroke = Rect::from_two_pos(segment[0], segment[1])
                            .expand(path.stroke.width / 2.0 + 0.5 / density);
                        let intersection = mask.intersect(stroke);
                        assert!(
                            !intersection.is_positive(),
                            "DPR{density} zoom{zoom} selected{selected}: label mask{mask:?} erases edge{stroke:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_subpixel_elbow_collapses_without_leaving_zero_length_normals() {
    let source = [Pos2::ZERO, Pos2::new(0.1, 0.0), Pos2::new(0.1, 50.0)];
    for density in [1.0, 2.0] {
        let points = super::geometry::screen_points(
            &source,
            Pos2::new(0.2, 0.2),
            0.1,
            egui::Stroke::new(1.0, palette::BLUE),
            density,
        );
        assert_eq!(points.len(), 2);
        assert!(
            points
                .windows(2)
                .all(|segment| (segment[1] - segment[0]).normalized().is_finite())
        );
    }
}

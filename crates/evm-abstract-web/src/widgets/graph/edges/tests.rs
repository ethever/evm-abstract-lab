use egui::epaint::{PathShape, TessellationOptions, Tessellator};
use egui::{Context, Mesh, Pos2, RawInput, Rect, Shape, Vec2, ViewportId};
use evm_abstract_protocol::EdgeKind;

use super::{EdgeRoute, paint_edge};
use crate::palette;

const ZOOMS: [f32; 8] = [2.5, 2.0, 1.0, 0.75, 0.5, 0.36, 0.25, 0.1];

struct PaintedEdge {
    paths: [PathShape; 2],
    label: Rect,
    text: Option<Rect>,
    bridge: Option<[Pos2; 2]>,
}

fn render(zoom: f32, density: f32, selected: bool, offset: f32) -> PaintedEdge {
    render_at_label(zoom, density, selected, offset, Pos2::new(130.0, 96.0))
}

fn render_at_label(
    zoom: f32,
    density: f32,
    selected: bool,
    offset: f32,
    label: Pos2,
) -> PaintedEdge {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut input = RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0))),
        ..RawInput::default()
    };
    input
        .viewports
        .get_mut(&ViewportId::ROOT)
        .unwrap()
        .native_pixels_per_point = Some(density);
    let label = Rect::from_min_size(label, Vec2::new(48.0, 16.0));
    let route = EdgeRoute {
        to_label: vec![
            Pos2::new(20.0, 20.0),
            Pos2::new(label.center().x, 20.0),
            label.center_top(),
        ],
        to_target: vec![
            label.center_bottom(),
            Pos2::new(label.center().x, 240.0),
            Pos2::new(260.0, 240.0),
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
    let paths: [PathShape; 2] = output
        .shapes
        .iter()
        .filter_map(|item| match &item.shape {
            Shape::Path(path) if !path.closed => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .try_into()
        .expect("a labeled edge has two separate paths");
    let arrow = output
        .shapes
        .iter()
        .find_map(|item| match &item.shape {
            Shape::Path(path) if path.closed => Some(path),
            _ => None,
        })
        .expect("every visible route keeps its arrowhead");
    assert_eq!(arrow.points.len(), 3);
    assert_eq!(arrow.points[0], *paths[1].points.last().unwrap());
    assert!(arrow.points.iter().all(|point| point.is_finite()));
    let side_a = arrow.points[1] - arrow.points[0];
    let side_b = arrow.points[2] - arrow.points[0];
    assert!((side_a.x * side_b.y - side_a.y * side_b.x).abs() > 0.1);
    let label = output
        .shapes
        .iter()
        .find_map(|item| match &item.shape {
            Shape::Rect(rect) if rect.fill == palette::BACKGROUND => Some(rect.rect),
            _ => None,
        })
        .expect("the label routing slot retains its background at every scale");
    let text = output.shapes.iter().find_map(|item| match &item.shape {
        Shape::Text(text) if text.galley.text() == "e₀ next" => {
            Some(Rect::from_min_size(text.pos, text.galley.size()))
        }
        _ => None,
    });
    let bridge = output.shapes.iter().find_map(|item| match &item.shape {
        Shape::LineSegment { points, .. } => Some(*points),
        _ => None,
    });
    PaintedEdge {
        paths,
        label,
        text,
        bridge,
    }
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
                    let edge = render(zoom, density, selected, offset);
                    let path = &edge.paths[0];
                    let mut tessellator =
                        Tessellator::new(density, TessellationOptions::default(), [1, 1], vec![]);
                    let mut mesh = Mesh::default();
                    tessellator.tessellate_path(path, &mut mesh);
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
fn label_vertices_keep_text_inside_and_routes_outside_at_readable_scales() {
    for density in [1.0, 2.0] {
        for zoom in ZOOMS {
            for selected in [false, true] {
                for label in [Pos2::new(130.0, 96.0), Pos2::new(95.0, 140.0)] {
                    let edge = render_at_label(zoom, density, selected, 0.0, label);
                    if zoom <= 0.35 {
                        assert!(edge.text.is_none());
                        assert_eq!(
                            edge.bridge,
                            Some([
                                *edge.paths[0].points.last().unwrap(),
                                edge.paths[1].points[0],
                            ]),
                            "hidden labels must not break edge connectivity"
                        );
                    } else {
                        assert!(edge.bridge.is_none());
                        let text = edge.text.expect("readable edge text is painted");
                        assert!(
                            edge.label.contains_rect(text),
                            "DPR{density} zoom{zoom}: text{text:?} escapes reserved label vertex{:?}",
                            edge.label
                        );
                    }
                    for path in &edge.paths {
                        for segment in path.points.windows(2) {
                            let stroke = Rect::from_two_pos(segment[0], segment[1])
                                .expand(path.stroke.width / 2.0 + 0.5 / density);
                            let interior = edge.label.shrink(path.stroke.width + 1.0 / density);
                            assert!(
                                !interior.intersect(stroke).is_positive(),
                                "DPR{density} zoom{zoom}: route crosses label interior"
                            );
                        }
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

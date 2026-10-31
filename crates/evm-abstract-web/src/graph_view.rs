//! Geometry shared by independent workspaces. Numeric IDs here are caller-owned
//! layout keys only; this module never assigns entity meaning or provenance.

use std::collections::BTreeMap;

use egui::{Painter, Pos2, Rect, Shape, Stroke, Vec2};
use evm_abstract_layout as geometry;

#[cfg(any(target_arch = "wasm32", test))]
mod canvas;
#[cfg(any(target_arch = "wasm32", test))]
pub(crate) use canvas::{Canvas, Card, Connection, Scene};

pub(crate) const FIT_MARGIN: f32 = 7.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct EdgeRoute {
    pub path: Vec<Pos2>,
    pub label: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Group {
    pub id: geometry::GroupId,
    pub parent: Option<geometry::GroupId>,
    pub kind: geometry::GroupKind,
    pub members: Vec<usize>,
    pub bounds: Rect,
    pub caption: Rect,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Placement {
    pub nodes: BTreeMap<usize, Rect>,
    pub edges: BTreeMap<usize, EdgeRoute>,
    pub groups: Vec<Group>,
    pub bounds: Rect,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            nodes: BTreeMap::new(),
            edges: BTreeMap::new(),
            groups: Vec::new(),
            bounds: Rect::NOTHING,
        }
    }
}

pub(crate) fn fit_scale(content: Vec2, viewport: Vec2) -> f32 {
    ((viewport.x - FIT_MARGIN * 2.0).max(1.0) / content.x.max(1.0))
        .min((viewport.y - FIT_MARGIN * 2.0).max(1.0) / content.y.max(1.0))
}

pub(crate) fn scene(input: &geometry::Input) -> Result<Placement, geometry::LayoutError> {
    let layout = geometry::layout(input)?;
    let mut placement = Placement::default();
    for (id, bounds) in layout.nodes {
        placement.nodes.insert(id, rectangle(bounds, "node", id)?);
    }
    for (id, edge) in layout.edges {
        placement.edges.insert(
            id,
            EdgeRoute {
                path: edge
                    .path
                    .into_iter()
                    .map(|p| point(p, "edge", id))
                    .collect::<Result<_, _>>()?,
                label: rectangle(edge.label, "label", id)?,
            },
        );
    }
    for group in layout.groups {
        placement.groups.push(Group {
            id: group.group.id,
            parent: group.group.parent,
            kind: group.group.kind,
            members: group.group.members,
            bounds: rectangle(group.bounds, "group", group.group.id.0)?,
            caption: rectangle(group.caption, "group caption", group.group.id.0)?,
            title: group.title,
        });
    }
    if let Some(bounds) = layout.bounds {
        placement.bounds = rectangle(bounds, "content", 0)?;
    }
    Ok(placement)
}

fn point(
    value: geometry::Point,
    element: &'static str,
    id: usize,
) -> Result<Pos2, geometry::LayoutError> {
    if !value.is_finite()
        || value.x.abs() > f64::from(f32::MAX)
        || value.y.abs() > f64::from(f32::MAX)
    {
        return Err(geometry::LayoutError::Geometry {
            element,
            id,
            reason: "outside painter coordinate range",
        });
    }
    Ok(Pos2::new(value.x as f32, value.y as f32))
}

fn rectangle(
    value: geometry::Rect,
    element: &'static str,
    id: usize,
) -> Result<Rect, geometry::LayoutError> {
    if !value.is_valid() {
        return Err(geometry::LayoutError::Geometry {
            element,
            id,
            reason: "nonfinite or empty rectangle",
        });
    }
    Ok(Rect::from_min_max(
        point(value.origin, element, id)?,
        point(
            geometry::Point::new(value.right(), value.bottom()),
            element,
            id,
        )?,
    ))
}

pub(crate) fn screen_points(
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

/// Paint one continuous route and its arrow with physical-pixel rounding.
/// Returning the same screen points lets callers add their own evidence overlays.
pub(crate) fn paint_path(
    painter: &Painter,
    route: &EdgeRoute,
    origin: Pos2,
    zoom: f32,
    stroke: Stroke,
) -> Vec<Pos2> {
    let points = screen_points(
        &route.path,
        origin,
        zoom,
        stroke,
        painter.pixels_per_point(),
    );
    if points.len() < 2 {
        return points;
    }
    painter.add(Shape::line(points.clone(), stroke));
    let end = points[points.len() - 1];
    let direction = (end - points[points.len() - 2]).normalized();
    let perpendicular = Vec2::new(-direction.y, direction.x);
    let size = (6.0 * zoom).max(2.0);
    painter.add(Shape::convex_polygon(
        vec![
            end,
            end - direction * size + perpendicular * size * 0.5,
            end - direction * size - perpendicular * size * 0.5,
        ],
        stroke.color,
        Stroke::NONE,
    ));
    points
}

pub(crate) fn screen_rect(canvas: Rect, world: Rect, pan: Vec2, zoom: f32) -> Rect {
    Rect::from_min_size(
        canvas.min + pan + world.min.to_vec2() * zoom,
        world.size() * zoom,
    )
}

/// Preserve the pointer's world position, including when Fit chose a tiny scale.
pub(crate) fn zoom_at(zoom: &mut f32, pan: &mut Vec2, pointer: Vec2, factor: f32) -> bool {
    if !factor.is_finite() || factor <= 0.0 || (factor - 1.0).abs() <= f32::EPSILON {
        return false;
    }
    let next = (*zoom * factor).min(2.5);
    let ratio = next / *zoom;
    if next <= 0.0 || !ratio.is_finite() || ratio <= 0.0 {
        return false;
    }
    let next_pan = pointer - (pointer - *pan) * ratio;
    if !next_pan.is_finite() {
        return false;
    }
    *zoom = next;
    *pan = next_pan;
    true
}

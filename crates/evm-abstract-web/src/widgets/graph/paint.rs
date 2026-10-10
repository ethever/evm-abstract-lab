//! Semantic zoom affects painted content only. Detailed previews still supply
//! all measured geometry, and a small hysteresis band avoids wheel jitter.

use egui::{Align2, FontId, Painter, Rect, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_protocol::BlockCoverage;

use super::{HEADER_HEIGHT, LINE_HEIGHT, PAD, content::NodeText};
use crate::palette;

#[cfg(test)]
mod tests;

const COMPACT_BELOW: f32 = 0.55;
const DETAILED_ABOVE: f32 = 0.65;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailLevel {
    Identity,
    Preview,
}

/// Keep the camera's presentation preference in egui's transient UI memory,
/// separate from report data and immutable layout geometry.
pub(super) fn detail_level(ui: &Ui, zoom: f32) -> DetailLevel {
    ui.ctx().data_mut(|data| {
        let level = data.get_temp_mut_or_insert_with(ui.id().with("cfg_semantic_zoom"), || {
            if zoom < DETAILED_ABOVE {
                DetailLevel::Identity
            } else {
                DetailLevel::Preview
            }
        });
        if zoom <= COMPACT_BELOW {
            *level = DetailLevel::Identity;
        } else if zoom >= DETAILED_ABOVE {
            *level = DetailLevel::Preview;
        }
        *level
    })
}

pub(super) fn node(
    painter: &Painter,
    rect: Rect,
    content: &NodeText,
    selected: bool,
    hovered: bool,
    zoom: f32,
    level: DetailLevel,
) {
    let border = if selected {
        palette::ACCENT
    } else if content.frontier || content.coverage != BlockCoverage::Current {
        palette::WARNING
    } else if hovered {
        palette::BLUE
    } else {
        palette::BORDER
    };
    painter.rect(
        rect,
        4.0,
        if selected {
            palette::SELECTED
        } else {
            palette::PANEL
        },
        Stroke::new(if selected { 1.5 } else { 1.0 }, border),
        StrokeKind::Inside,
    );
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if level == DetailLevel::Identity {
        identity(&painter, rect, content, zoom);
        return;
    }
    let label = |y: f32, text: &str, color, size: f32| {
        crate::notation::paint(
            &painter,
            rect.min + Vec2::new(PAD, y) * zoom,
            Align2::LEFT_TOP,
            text,
            FontId::monospace(size * zoom),
            color,
        );
    };
    label(4.0, &content.title, identity_color(content), 12.0);
    label(18.0, &content.detail, palette::MUTED, 10.0);
    painter.hline(
        rect.x_range(),
        rect.top() + (HEADER_HEIGHT - 3.0) * zoom,
        Stroke::new(1.0, palette::BORDER),
    );
    for (index, (line, color)) in content.lines.iter().enumerate() {
        label(
            HEADER_HEIGHT + index as f32 * LINE_HEIGHT,
            line,
            *color,
            11.0,
        );
    }
}

fn identity_color(content: &NodeText) -> egui::Color32 {
    if content.coverage == BlockCoverage::Current {
        palette::ACCENT
    } else {
        palette::WARNING
    }
}

fn identity(painter: &Painter, rect: Rect, content: &NodeText, zoom: f32) {
    let nominal = (12.0 * zoom).clamp(9.0, 12.0);
    let measured = content
        .compact
        .iter()
        .map(|text| {
            painter
                .layout_job(crate::notation::job(
                    text,
                    FontId::monospace(nominal),
                    identity_color(content),
                ))
                .size()
        })
        .fold(Vec2::ZERO, |size, row| {
            Vec2::new(size.x.max(row.x), size.y + row.y)
        });
    if measured.min_elem() <= 0.0 {
        return;
    }
    let available = (rect.size() - Vec2::splat(4.0 * zoom)).max(Vec2::splat(0.1));
    let fit = (available.x / measured.x)
        .min(available.y / measured.y)
        .min(1.0);
    let font = FontId::monospace(nominal * fit);
    let rows: Vec<_> = content
        .compact
        .iter()
        .map(|text| {
            painter.layout_job(crate::notation::job(
                text,
                font.clone(),
                identity_color(content),
            ))
        })
        .collect();
    let height: f32 = rows.iter().map(|row| row.size().y).sum();
    let mut top = rect.center().y - height * 0.5;
    for row in rows {
        let size = row.size();
        painter.galley(
            egui::Pos2::new(rect.center().x - size.x * 0.5, top),
            row,
            identity_color(content),
        );
        top += size.y;
    }
}

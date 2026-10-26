//! Semantic zoom affects painted content only. Detailed previews still supply
//! all measured geometry, and a small hysteresis band avoids wheel jitter.

use egui::{Align2, FontId, Painter, Rect, Stroke, StrokeKind, Ui, Vec2};
use evm_abstract_protocol::{BlockCoverage, OutcomeKind};

use super::{HEADER_HEIGHT, LINE_HEIGHT, PAD, content::NodeText};
use crate::palette;

#[cfg(test)]
mod tests;

const COMPACT_BELOW: f32 = 0.78;
const DETAILED_ABOVE: f32 = 0.88;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailLevel {
    Silhouette,
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
        if zoom < 0.45 {
            *level = DetailLevel::Silhouette;
        } else if zoom >= DETAILED_ABOVE {
            *level = DetailLevel::Preview;
        } else if zoom <= COMPACT_BELOW || *level == DetailLevel::Silhouette {
            *level = DetailLevel::Identity;
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
    } else if hovered {
        palette::BLUE
    } else {
        palette::BORDER
    };
    painter.rect(
        rect,
        4.0,
        match content.termination.single_kind() {
            Some(OutcomeKind::Return) => palette::TERMINAL_NORMAL,
            Some(OutcomeKind::Revert) => palette::TERMINAL_REVERT,
            Some(OutcomeKind::Failure) => palette::TERMINAL_EXCEPTION,
            None => palette::PANEL,
        },
        Stroke::new(if selected { 1.5 } else { 1.0 }, border),
        StrokeKind::Inside,
    );
    if rect.width().min(rect.height()) >= 12.0
        && (content.frontier
            || content.coverage != BlockCoverage::Current
            || content.termination.unresolved > 0
            || content.termination.non_current > 0)
    {
        // The inset dashed outline keeps incomplete coverage visible alongside
        // the independent outer selection/hover outline and semantic fill.
        let inset = 3.0_f32.min(rect.width().min(rect.height()) * 0.2);
        let warning = rect.shrink(inset);
        painter.extend(egui::Shape::dashed_line(
            &[
                warning.left_top(),
                warning.right_top(),
                warning.right_bottom(),
                warning.left_bottom(),
                warning.left_top(),
            ],
            Stroke::new(1.0, palette::WARNING),
            4.0,
            3.0,
        ));
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    if level == DetailLevel::Silhouette || rect.width() < 42.0 || rect.height() < 20.0 {
        return;
    }
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
    for (index, (line, color)) in content.badges.iter().chain(&content.lines).enumerate() {
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
    // Identity is a different presentation, not a font floor on a shrinking
    // preview. Omit rows that cannot fit; each glyph stays inside this card.
    let font = FontId::monospace(10.0);
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
    let available = rect.shrink((4.0 * zoom).max(2.0));
    let mut top = available.top();
    for row in rows {
        let size = row.size();
        if top + size.y > available.bottom() {
            break;
        }
        painter.galley(
            egui::Pos2::new(available.left(), top),
            row,
            identity_color(content),
        );
        top += size.y;
    }
}

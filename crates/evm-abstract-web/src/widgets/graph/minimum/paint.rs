//! Conservative cards use the same clipped semantic zoom as Native cards.
use super::control::{Card, CardKind};
use crate::{palette, widgets::graph::paint};
use egui::{Align2, FontId, Rect, Stroke, StrokeKind, Vec2};

pub(super) fn paint_card(
    painter: &egui::Painter,
    rect: Rect,
    card: &Card,
    selected: bool,
    hovered: bool,
    zoom: f32,
    level: paint::DetailLevel,
) {
    let current = !card.origins.is_empty()
        && card
            .origins
            .iter()
            .all(|origin| origin.current && origin.complete && !origin.excluded);
    let color = if selected {
        palette::ACCENT
    } else if hovered {
        palette::BLUE
    } else if card.target_set.is_some() || card.boundary {
        palette::WARNING
    } else if current {
        palette::BLUE
    } else {
        palette::MUTED
    };
    painter.rect(
        rect,
        4.0,
        if selected {
            palette::SELECTED
        } else {
            palette::RAISED
        },
        Stroke::new(if selected { 1.5 } else { 1.0 }, color),
        StrokeKind::Inside,
    );
    if level == paint::DetailLevel::Silhouette || rect.width() < 42.0 || rect.height() < 20.0 {
        return;
    }
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let inset = (6.0 * zoom).max(2.0);
    let at = rect.min + Vec2::splat(inset);
    if level == paint::DetailLevel::Identity {
        painter.text(
            at,
            Align2::LEFT_TOP,
            &card.title,
            FontId::monospace(10.0),
            palette::TEXT,
        );
        return;
    }
    painter.text(
        at,
        Align2::LEFT_TOP,
        &card.title,
        FontId::monospace(12.0 * zoom),
        palette::TEXT,
    );
    painter.text(
        at + egui::vec2(0.0, 24.0 * zoom),
        Align2::LEFT_TOP,
        &card.caption,
        FontId::proportional(11.0 * zoom),
        color,
    );
    painter.text(
        at + egui::vec2(0.0, 46.0 * zoom),
        Align2::LEFT_TOP,
        &card.qualifier,
        FontId::monospace(10.5 * zoom),
        palette::MUTED,
    );
    if matches!(card.kind, CardKind::Calls { .. } | CardKind::Folded) && rect.height() >= 76.0 {
        painter.text(
            rect.right_bottom() - Vec2::splat(inset),
            Align2::RIGHT_BOTTOM,
            "Open →",
            FontId::proportional(10.0 * zoom),
            palette::BLUE,
        );
    }
}

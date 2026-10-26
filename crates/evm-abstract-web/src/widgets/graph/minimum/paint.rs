//! Conservative cards use clipped code previews and independent receipt/target
//! hit regions. Painting never decides the semantic level of a control edge.
use super::{
    control::Card,
    text::{ACTION_HEIGHT, CardText},
};
use crate::{palette, widgets::graph::paint};
use egui::{Align2, FontId, Rect, Stroke, StrokeKind, Vec2};

pub(super) struct CardPaint<'a> {
    pub card: &'a Card,
    pub text: &'a CardText,
    pub selected: bool,
    pub hovered: bool,
    pub zoom: f32,
    pub level: paint::DetailLevel,
}

pub(super) fn paint_card(painter: &egui::Painter, rect: Rect, style: CardPaint<'_>) {
    let CardPaint {
        card,
        text,
        selected,
        hovered,
        zoom,
        level,
    } = style;
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
    let inset = ((if text.compact { 4.0 } else { 6.0 }) * zoom).max(2.0);
    let at = rect.min + Vec2::splat(inset);
    let title = if text.size.x < 220.0 {
        &text.compact_title
    } else {
        &text.title
    };
    if level == paint::DetailLevel::Identity {
        painter.text(
            at,
            Align2::LEFT_TOP,
            title,
            FontId::monospace(10.0),
            palette::TEXT,
        );
        return;
    }
    painter.text(
        at,
        Align2::LEFT_TOP,
        title,
        FontId::monospace(if text.compact {
            11.0 * zoom
        } else {
            12.0 * zoom
        }),
        palette::TEXT,
    );
    painter.text(
        at + egui::vec2(
            0.0,
            if text.compact {
                16.0 * zoom
            } else {
                20.0 * zoom
            },
        ),
        Align2::LEFT_TOP,
        &text.detail,
        FontId::monospace(11.0 * zoom),
        color,
    );
    for (row, (line, color)) in text.rows.iter().enumerate() {
        painter.text(
            at + egui::vec2(
                0.0,
                (if text.compact { 32.0 } else { 38.0 } + row as f32 * 14.0) * zoom,
            ),
            Align2::LEFT_TOP,
            line,
            FontId::monospace(11.0 * zoom),
            *color,
        );
    }
}

/// Footer actions remain separate at readable scale. A source jump hub can
/// select its own receipt without opening any of its possible destinations.
#[cfg(test)]
pub(super) fn actions(rect: Rect, card: &Card, zoom: f32, level: paint::DetailLevel) -> Actions {
    actions_with_targets(rect, card, card.target_set.is_some(), zoom, level)
}

pub(super) fn actions_with_targets(
    rect: Rect,
    card: &Card,
    has_targets: bool,
    zoom: f32,
    level: paint::DetailLevel,
) -> Actions {
    if level != paint::DetailLevel::Preview || (card.origins.is_empty() && !has_targets) {
        return Actions {
            body: rect,
            receipts: None,
            targets: None,
        };
    }
    let height = (ACTION_HEIGHT * zoom).max(20.0).min(rect.height());
    let footer = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - height),
        rect.right_bottom(),
    );
    let receipts = (!card.origins.is_empty()).then(|| {
        if has_targets {
            Rect::from_min_max(footer.min, footer.center_bottom())
        } else {
            footer
        }
    });
    let targets = has_targets.then(|| {
        if receipts.is_some() {
            Rect::from_min_max(footer.center_top(), footer.max)
        } else {
            footer
        }
    });
    Actions {
        body: Rect::from_min_max(rect.min, footer.right_top()),
        receipts,
        targets,
    }
}

pub(super) struct Actions {
    pub body: Rect,
    pub receipts: Option<Rect>,
    pub targets: Option<Rect>,
}

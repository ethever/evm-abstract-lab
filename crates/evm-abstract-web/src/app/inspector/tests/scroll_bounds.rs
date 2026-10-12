//! A short inspector clips its contents to the chosen panel rectangle. Scroll
//! limits must use that actual viewport so the last row remains reachable.

use super::{account, rendered_at, word};
use crate::app::inspector::{buffers, stores};
use egui::{Context, Event, Pos2, Rect, Vec2};
use evm_abstract_protocol as api;

#[derive(Clone, Copy, Debug)]
enum Surface {
    Storage,
    Transient,
    Stack,
    Bytes,
}

impl Surface {
    fn last_label(self) -> &'static str {
        match self {
            Self::Storage | Self::Transient => "0x5f",
            Self::Stack => "[   0]",
            Self::Bytes => "0x0000005f",
        }
    }

    fn short_height(self) -> f32 {
        match self {
            Self::Storage | Self::Transient => 48.0,
            Self::Stack => 60.0,
            Self::Bytes => 95.0,
        }
    }
}

fn draw(
    ctx: &Context,
    time: &mut f64,
    report: &api::AnalysisReport,
    surface: Surface,
    height: f32,
    events: Vec<Event>,
) -> Vec<(String, Rect)> {
    rendered_at(
        ctx,
        time,
        events,
        Vec2::new(520.0, height),
        |ui| match surface {
            Surface::Storage | Surface::Transient => stores::storage(
                ui,
                report,
                None,
                &report.accounts[0].address,
                matches!(surface, Surface::Transient),
            ),
            Surface::Stack => buffers::stack(ui, &report.byte_arrays[0].values),
            Surface::Bytes => buffers::show(ui, report, 0, "Memory"),
        },
    )
}

fn scroll_to_end(
    ctx: &Context,
    time: &mut f64,
    report: &api::AnalysisReport,
    surface: Surface,
    height: f32,
) -> Vec<(String, Rect)> {
    for _ in 0..3 {
        draw(ctx, time, report, surface, height, vec![]);
    }
    let events = vec![
        Event::PointerMoved(Pos2::new(260.0, height - 4.0)),
        Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: Vec2::new(0.0, -100_000.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        },
    ];
    let mut painted = draw(ctx, time, report, surface, height, events);
    for _ in 0..30 {
        painted = draw(ctx, time, report, surface, height, vec![]);
    }
    painted
}

#[test]
fn final_rows_remain_fully_reachable_when_the_content_viewport_is_below_64_points() {
    let mut report = crate::tests::report();
    let mut account = account();
    account.storage = (0..96)
        .map(|index| api::StorageEntry {
            slot: format!("0x{index:x}"),
            value: word("0xcafe"),
        })
        .collect();
    account.transient = account.storage.clone();
    report.accounts = vec![account];
    report.byte_arrays = vec![api::ByteArraySnapshot {
        length: word("0x60"),
        default: word("0x0"),
        cells: (0..96)
            .map(|index| api::ByteCell {
                offset: index,
                value: index,
            })
            .collect(),
        values: (0..96).map(|_| word("0x2a")).collect(),
        memory: true,
    }];
    for surface in [
        Surface::Storage,
        Surface::Transient,
        Surface::Stack,
        Surface::Bytes,
    ] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        let mut time = 0.0;
        let tall = scroll_to_end(&ctx, &mut time, &report, surface, 240.0);
        let full_height = tall
            .iter()
            .find(|(text, _)| text == surface.last_label())
            .expect("last row is visible in the tall baseline")
            .1
            .height();
        let short_height = surface.short_height();
        let short = scroll_to_end(&ctx, &mut time, &report, surface, short_height);
        let last = short
            .iter()
            .find(|(text, _)| text == surface.last_label())
            .unwrap_or_else(|| {
                panic!("{surface:?}: last row is unreachable in {short_height}pt: {short:?}")
            });
        assert!(
            last.1.height() >= full_height - 0.5,
            "{surface:?}: last row is clipped at the scroll limit: {:?}, expected height {full_height}",
            last.1,
        );
        assert!(last.1.bottom() <= short_height);
    }
}

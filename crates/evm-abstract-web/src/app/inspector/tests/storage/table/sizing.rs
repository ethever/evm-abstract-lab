//! Defaults cover the whole key domain; only user gestures establish a width
//! preference. Report/account changes never need a full-list measurement pass.

use super::super::{context, storage_report, word};
use super::{drag, glyphs, near, panel, separator};
use egui::{Event, FontId, Modifiers, Pos2, Rect, Vec2};

fn long_slot() -> String {
    format!("0x{}90abcdef", "ab".repeat(28))
}

#[test]
fn default_reserves_offscreen_full_keys_while_ten_thousand_rows_remain_virtualized() {
    let mut report = storage_report(10_000);
    let last = report.accounts[0].storage.last_mut().unwrap();
    last.slot = long_slot();
    last.value = word("0xac3");
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    for transient in [false, true] {
        panel.transient = transient;
        let top = panel.draw(&context, &mut time, vec![]);
        assert!(
            top.rect(&long_slot()).is_none(),
            "the longest slot starts offscreen"
        );
        assert!(top.widgets < 100);
        let width = separator(&context, transient).rect.center().x;
        let last = panel.scroll(&context, &mut time, Pos2::new(width, 140.0), -1_000_000.0);
        let entry = report.accounts[0].storage.last().unwrap();
        let (clip, text) = last.slot_paint(entry);
        assert_eq!(
            glyphs(text),
            entry.slot,
            "an offscreen key must have a complete default"
        );
        assert!(!text.galley.elided);
        assert!(clip.contains_rect(Rect::from_min_size(text.pos, text.galley.size())));
        assert!(
            text.galley
                .rows
                .iter()
                .all(|row| !row.visuals.mesh.vertices.is_empty())
        );
        assert!(last.widgets < 100, "{} widgets", last.widgets);
        near(separator(&context, transient).rect.center().x, width);
    }
}

#[test]
fn changed_reports_accounts_and_stores_keep_untouched_defaults_complete() {
    let mut initial = storage_report(1);
    initial.accounts[0].transient = initial.accounts[0].storage.clone();
    let mut changed = initial.clone();
    changed.metadata.fingerprint = format!("0x{}", "12".repeat(32));
    changed.accounts[0].storage[0].slot = long_slot();
    changed.accounts[0].storage[0].value = word("0xac3");
    changed.accounts[0].transient = changed.accounts[0].storage.clone();
    let mut other = changed.accounts[0].clone();
    other.address = format!("0x{}", "22".repeat(20));
    changed.accounts.push(other);
    let snapshot = evm_abstract_protocol::StoreSnapshot {
        storage_facts: vec![],
        transient_facts: vec![],
        accounts: changed.accounts.clone(),
        ..super::super::super::store()
    };
    let context = context();
    let mut time = 0.0;
    for transient in [false, true] {
        let mut panel = panel(&initial);
        panel.transient = transient;
        panel.draw(&context, &mut time, vec![]);
        let initial_x = separator(&context, transient).rect.center().x;
        panel.report = &changed;
        for address in [&changed.accounts[1].address, &changed.accounts[0].address] {
            panel.address = address;
            for store in [None, Some(&snapshot)] {
                panel.store = store;
                let drawn = panel.draw(&context, &mut time, vec![]);
                assert_eq!(
                    drawn.slot_glyphs(&changed.accounts[0].storage[0]),
                    long_slot()
                );
                near(separator(&context, transient).rect.center().x, initial_x);
            }
        }
    }
}

#[test]
fn font_changes_refresh_an_untouched_default_and_preserve_an_explicit_native_width() {
    let mut report = storage_report(1);
    report.accounts[0].storage[0].slot = long_slot();
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    let original = panel.draw(&context, &mut time, vec![]);
    let slot_x = original.rect("Slot").unwrap().left();
    let width = original.rect("Value").unwrap().left() - slot_x - 8.0;
    let start = Pos2::new(
        separator(&context, false).rect.center().x,
        original.rect("Slot").unwrap().center().y,
    );
    drag(
        |events| {
            panel.draw(&context, &mut time, events);
        },
        start,
        start + Vec2::new(128.0 - width, 0.0),
    );
    panel.transient = true;
    let untouched = panel.draw(&context, &mut time, vec![]);
    context.global_style_mut(|style| {
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, FontId::monospace(19.0));
    });
    let larger = panel.draw(&context, &mut time, vec![]);
    assert!(larger.rect("Value").unwrap().left() > untouched.rect("Value").unwrap().left() + 100.0);
    assert_eq!(
        larger.slot_glyphs(&report.accounts[0].storage[0]),
        long_slot()
    );
    panel.transient = false;
    let explicit = panel.draw(&context, &mut time, vec![]);
    near(explicit.rect("Value").unwrap().left(), slot_x + 128.0 + 8.0);
    assert!(
        explicit
            .slot_glyphs(&report.accounts[0].storage[0])
            .starts_with('…')
    );
    assert!(
        explicit
            .slot_glyphs(&report.accounts[0].storage[0])
            .ends_with("90abcdef")
    );
}

#[test]
fn a_narrow_window_scrolls_the_full_default_column_and_value_together() {
    let mut report = storage_report(1);
    report.accounts[0].storage[0].slot = long_slot();
    report.accounts[0].storage[0].value = word("0xac3");
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    let size = Vec2::new(180.0, 240.0);
    for transient in [false, true] {
        panel.transient = transient;
        let wide = panel.draw(&context, &mut time, vec![]);
        let width = wide.rect("Value").unwrap().left() - wide.rect("Slot").unwrap().left();
        let narrow = panel.draw_at(&context, &mut time, vec![], size);
        assert!(narrow.rect("0xac3").is_none());
        panel.draw_at(
            &context,
            &mut time,
            vec![
                Event::PointerMoved(Pos2::new(90.0, 180.0)),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(-10_000.0, 0.0),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
            size,
        );
        let mut scrolled = panel.draw_at(&context, &mut time, vec![], size);
        for _ in 0..30 {
            scrolled = panel.draw_at(&context, &mut time, vec![], size);
        }
        let state = egui::scroll_area::State::load(&context, scrolled.scroll_id).unwrap();
        assert!(state.offset.x > 300.0 && state.offset.y == 0.0);
        assert!(
            scrolled.rect("0xac3").is_some(),
            "Value uses the same horizontal scroll"
        );
        let (_, text) = scrolled.slot_paint(&report.accounts[0].storage[0]);
        assert_eq!(
            glyphs(text),
            long_slot(),
            "viewport changes never truncate the native column"
        );
        let (clip, text) = scrolled.slot_paint(&report.accounts[0].storage[0]);
        let last = text.galley.rows[0]
            .glyphs
            .last()
            .unwrap()
            .logical_rect()
            .translate(text.pos.to_vec2() + text.galley.rows[0].pos.to_vec2());
        assert!(
            clip.contains_rect(last),
            "the actual low-order glyph is visible"
        );
        for _ in 0..3 {
            panel.draw(&context, &mut time, vec![]);
        }
        let restored = panel.draw(&context, &mut time, vec![]);
        near(
            restored.rect("Value").unwrap().left() - restored.rect("Slot").unwrap().left(),
            width,
        );
    }
}

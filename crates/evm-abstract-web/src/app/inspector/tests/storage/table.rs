//! Native table geometry, gestures and presentation preferences.

use super::{
    Drawn, Panel, account, context, entries, frame, rendered_at, storage_report,
    viewport_widget_budget, word,
};
use crate::app::{View, inspector::Tab};
use egui::{Context, Event, FontId, Id, Modifiers, Pos2, Rect, Vec2};
use evm_abstract_protocol as api;

mod sizing;

fn separator(context: &Context, transient: bool) -> egui::Response {
    let id = Id::new(("inspector_storage_slot_table", transient))
        .with(egui::IdSalt::new("slots"))
        .with("resize_column")
        .with(0_usize);
    context.read_response(id).expect("native table separator")
}

fn drag(mut draw: impl FnMut(Vec<Event>), start: Pos2, end: Pos2) {
    draw(vec![Event::PointerMoved(start)]);
    draw(vec![Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: Modifiers::NONE,
    }]);
    draw(vec![Event::PointerMoved(end)]);
    draw(vec![]);
    draw(vec![Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    draw(vec![]);
}

fn panel(report: &api::AnalysisReport) -> Panel<'_> {
    Panel {
        report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 240.0,
    }
}

fn near(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 1.0, "{actual} != {expected}");
}

impl Drawn {
    fn slot_paint(&self, entry: &api::StorageEntry) -> &(Rect, egui::epaint::TextShape) {
        let value = self.rect(&entry.value.summary).expect("row value");
        self.paint
            .iter()
            .find(|(_, text)| {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                (rect.center().y - value.center().y).abs() < 1.0
                    && rect.right() < value.left()
                    && text
                        .galley
                        .job
                        .sections
                        .iter()
                        .all(|section| section.format.font_id.family == egui::FontFamily::Monospace)
            })
            .unwrap_or_else(|| panic!("missing slot paint for {}: {:?}", entry.slot, self.text))
    }

    fn slot_glyphs(&self, entry: &api::StorageEntry) -> String {
        glyphs(&self.slot_paint(entry).1)
    }

    fn assert_columns(&self, entries: &[api::StorageEntry]) {
        let slot_x = self.rect("Slot").unwrap().left();
        let value_x = self.rect("Value").unwrap().left();
        assert!(value_x > slot_x + 50.0);
        for entry in entries {
            let (clip, painted) = self.slot_paint(entry);
            let slot = Rect::from_min_size(painted.pos, painted.galley.size());
            let value = self.rect(&entry.value.summary).unwrap();
            // Eight points separate native cells; two keep glyph ink inside
            // the slot cell. Short keys remain unpadded at this same right edge.
            near(slot.right(), value_x - 10.0);
            near(value.left(), value_x);
            near(slot.center().y, value.center().y);
            assert!(slot.right() < value.left());
            assert!(
                clip.contains_rect(slot),
                "{clip:?} does not contain {slot:?}"
            );
            for row in &painted.galley.rows {
                assert!(
                    !row.visuals.mesh.vertices.is_empty(),
                    "slot must paint real glyphs"
                );
                let ink = row
                    .visuals
                    .mesh_bounds
                    .translate(painted.pos.to_vec2() + row.pos.to_vec2());
                assert!(
                    clip.contains_rect(ink),
                    "glyph ink {ink:?} escaped {clip:?}"
                );
            }
            assert!(
                !painted.galley.elided,
                "suffix fitting must not hide low bits"
            );
            let displayed = glyphs(painted);
            if let Some(suffix) = displayed.strip_prefix('…') {
                assert!(
                    !suffix.is_empty() && entry.slot.ends_with(suffix),
                    "{displayed}"
                );
                assert!(displayed.chars().count() < entry.slot.chars().count());
            } else {
                assert_eq!(displayed, entry.slot, "short keys must not be zero padded");
            }
        }
    }
}

fn glyphs(text: &egui::epaint::TextShape) -> String {
    text.galley
        .rows
        .iter()
        .flat_map(|row| &row.glyphs)
        .map(|glyph| glyph.chr)
        .collect()
}

#[test]
fn mixed_key_lengths_align_and_native_drags_shrink_and_grow_the_column() {
    let mut report = storage_report(3);
    for (entry, (slot, value)) in report.accounts[0].storage.iter_mut().zip([
        ("0x1".into(), "0xa1"),
        ("0x123456".into(), "0xa2"),
        (format!("0x{}90abcdef", "ab".repeat(28)), "0xa3"),
    ]) {
        entry.slot = slot;
        entry.value = word(value);
    }
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    for transient in [false, true] {
        panel.transient = transient;
        let initial = panel.draw(&context, &mut time, vec![]);
        initial.assert_columns(&report.accounts[0].storage);
        for entry in &report.accounts[0].storage {
            assert_eq!(initial.slot_glyphs(entry), entry.slot);
        }
        let initial_x = initial.rect("Value").unwrap().left();
        let slot_x = initial.rect("Slot").unwrap().left();
        let initial_width = initial_x - slot_x - 8.0;
        for width in [
            56.0,
            67.5,
            93.25,
            128.0,
            initial_width + 120.0,
            initial_width,
        ] {
            let response = separator(&context, transient);
            let start = Pos2::new(
                response.rect.center().x,
                initial.rect("Slot").unwrap().center().y,
            );
            let current = panel.draw(&context, &mut time, vec![]);
            let current_width = current.rect("Value").unwrap().left() - slot_x - 8.0;
            drag(
                |events| {
                    panel.draw(&context, &mut time, events);
                },
                start,
                start + Vec2::new(width - current_width, 0.0),
            );
            let resized = panel.draw(&context, &mut time, vec![]);
            resized.assert_columns(&report.accounts[0].storage);
            near(resized.rect("Value").unwrap().left(), slot_x + width + 8.0);
            let long = &report.accounts[0].storage[2];
            if width < initial_width {
                assert!(resized.slot_glyphs(long).starts_with('…'));
                assert!(resized.slot_glyphs(long).ends_with("cdef"));
            } else {
                assert_eq!(resized.slot_glyphs(long), long.slot);
            }
        }
    }
}

#[test]
fn narrowed_keys_keep_suffixes_and_values_keep_their_ellipses_and_full_hovers() {
    let mut report = storage_report(1);
    let entry = &mut report.accounts[0].storage[0];
    entry.slot = format!("0x{}90abcdef", "ab".repeat(28));
    entry.value = word(&format!("0x{}", "cd".repeat(32)));
    let entry = &report.accounts[0].storage[0];
    let panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    let initial = panel.draw(&context, &mut time, vec![]);
    let start = Pos2::new(
        separator(&context, false).rect.center().x,
        initial.rect("Slot").unwrap().center().y,
    );
    let initial_width =
        initial.rect("Value").unwrap().left() - initial.rect("Slot").unwrap().left() - 8.0;
    drag(
        |events| {
            panel.draw(&context, &mut time, events);
        },
        start,
        start + Vec2::new(128.0 - initial_width, 0.0),
    );
    let size = Vec2::new(520.0, panel.height);
    let drawn = panel.draw_at(&context, &mut time, vec![], size);
    let slot = drawn.slot_paint(entry);
    assert_eq!(glyphs(&slot.1), "…ababab90abcdef");
    assert!(!slot.1.galley.elided);
    let value = drawn
        .paint
        .iter()
        .find(|(_, text)| text.galley.text() == entry.value.summary)
        .unwrap();
    assert!(value.1.galley.elided && glyphs(&value.1).ends_with('…'));
    assert!(glyphs(&value.1).chars().count() < entry.value.summary.chars().count());
    for (clip, painted) in [slot, value] {
        assert!(
            painted
                .galley
                .job
                .sections
                .iter()
                .all(|section| section.format.font_id == FontId::monospace(13.0))
        );
        assert!(clip.contains_rect(Rect::from_min_size(painted.pos, painted.galley.size())));
    }
    let mut hovered = panel.draw_at(
        &context,
        &mut time,
        vec![Event::PointerMoved(
            Rect::from_min_size(slot.1.pos, slot.1.galley.size()).center(),
        )],
        size,
    );
    for _ in 0..15 {
        hovered = panel.draw_at(&context, &mut time, vec![], size);
    }
    assert!(
        hovered
            .paint
            .iter()
            .any(|(_, text)| glyphs(text) == entry.slot && !text.galley.elided),
        "full slot tooltip: {:?}",
        hovered.text
    );
    panel.draw_at(
        &context,
        &mut time,
        vec![Event::PointerMoved(
            drawn.rect(&entry.value.summary).unwrap().center(),
        )],
        size,
    );
    for _ in 0..15 {
        hovered = panel.draw_at(&context, &mut time, vec![], size);
    }
    for label in [
        "Constants",
        "Known zero bits",
        "Known one bits",
        "Unsigned range",
        "Signed range (bit patterns)",
        "Congruence",
        "Possible origins",
    ] {
        assert!(
            hovered.rect(label).is_some(),
            "missing {label}: {:?}",
            hovered.text
        );
    }
}

#[test]
fn widths_stay_fixed_while_scrolling_or_replacing_the_selected_snapshot() {
    let mut report = storage_report(256);
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut snapshot = super::store();
    snapshot.accounts[0].storage = vec![api::StorageEntry {
        slot: format!("0x{}", "ff".repeat(32)),
        value: word("0xd00d"),
    }];
    let mut panel = panel(&report);
    let context = context();
    let mut time = 0.0;
    let initial = panel.draw(&context, &mut time, vec![]);
    let initial_x = initial.rect("Value").unwrap().left();
    let start = Pos2::new(
        separator(&context, false).rect.center().x,
        initial.rect("Slot").unwrap().center().y,
    );
    drag(
        |events| {
            panel.draw(&context, &mut time, events);
        },
        start,
        start - Vec2::new(36.0, 0.0),
    );
    let expected = initial_x - 36.0;
    let scrolled = panel.scroll(&context, &mut time, Pos2::new(240.0, 140.0), -2000.0);
    assert!(scrolled.rect("0x100000").is_none());
    near(scrolled.rect("0xcafe").unwrap().left(), expected);
    panel.transient = true;
    let transient = panel.draw(&context, &mut time, vec![]);
    near(transient.rect("Value").unwrap().left(), initial_x);
    panel.transient = false;
    near(
        panel
            .draw(&context, &mut time, vec![])
            .rect("0xcafe")
            .unwrap()
            .left(),
        expected,
    );
    panel.store = Some(&snapshot);
    for _ in 0..3 {
        panel.draw(&context, &mut time, vec![]);
    }
    let replaced = panel.draw(&context, &mut time, vec![]);
    near(replaced.rect("Value").unwrap().left(), expected);
    near(replaced.rect("0xd00d").unwrap().left(), expected);
    panel.height = 600.0;
    let expanded = panel.expand(&context, &mut time);
    assert!(expanded.rect("Balance").is_some());
    near(expanded.rect("Value").unwrap().left(), expected);
}

#[test]
fn each_account_and_storage_kind_virtualizes_against_the_outer_viewport() {
    let mut report = storage_report(10_000);
    report.accounts[0].transient = entries(10_000);
    let mut second = report.accounts[0].clone();
    second.address = "0x2222222222222222222222222222222222222222".into();
    report.accounts.push(second);
    let context = context();
    let mut time = 0.0;
    for account in &report.accounts {
        for transient in [false, true] {
            let panel = Panel {
                address: &account.address,
                transient,
                ..panel(&report)
            };
            let widget_budget = viewport_widget_budget(&panel);
            let top = panel.expand(&context, &mut time);
            assert!(top.rect("Balance").is_some());
            assert_eq!(top.slots(), 0);
            assert!(top.widgets <= widget_budget, "{}", top.widgets);
            let border = Pos2::new(separator(&context, transient).rect.center().x, 140.0);
            let middle = panel.scroll(&context, &mut time, border, -50_000.0);
            assert!(middle.slots() > 0 && middle.slots() <= 12);
            assert!(middle.widgets <= widget_budget, "{}", middle.widgets);
            let offset = egui::scroll_area::State::load(&context, middle.scroll_id)
                .unwrap()
                .offset
                .y;
            let value = middle.rect("0xcafe").unwrap().center();
            let wheel = panel.scroll(&context, &mut time, value, -120.0);
            assert!(
                egui::scroll_area::State::load(&context, wheel.scroll_id)
                    .unwrap()
                    .offset
                    .y
                    > offset + 100.0,
                "wheel over cells must reach the outer scroll area"
            );
            // Keep the rich hover popup out of the row-creation count.
            let border = Pos2::new(separator(&context, transient).rect.center().x, 140.0);
            let last = panel.scroll(&context, &mut time, border, -1_000_000.0);
            assert!(last.rect("0x10270f").is_some());
            assert!(last.slots() > 0 && last.slots() <= 12);
            assert!(last.widgets <= widget_budget, "{}", last.widgets);
            assert!(
                egui::scroll_area::State::load(&context, last.scroll_id)
                    .unwrap()
                    .offset
                    .y
                    > 200_000.0
            );
            eprintln!(
                "account={}, transient={transient}, widgets(top/middle/last)={}/{}/{}, visible_rows(middle/last)={}/{}",
                account.address,
                top.widgets,
                middle.widgets,
                last.widgets,
                middle.slots(),
                last.slots(),
            );
        }
    }
}

#[test]
fn narrow_short_viewports_reach_the_final_row_without_an_inner_scroll_limit() {
    let mut report = storage_report(96);
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut panel = panel(&report);
    let context = context();
    crate::palette::configure(&context);
    let mut time = 0.0;
    for transient in [false, true] {
        panel.transient = transient;
        let tall = panel.scroll(&context, &mut time, Pos2::new(240.0, 140.0), -100_000.0);
        let full_height = tall.rect("0x10005f").unwrap().height();
        for size in [Vec2::new(180.0, 48.0), Vec2::new(320.0, 56.0)] {
            panel.draw_at(&context, &mut time, vec![], size);
            panel.draw_at(
                &context,
                &mut time,
                vec![
                    Event::PointerMoved(Pos2::new(40.0, 24.0)),
                    Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        // The default Slot column stays full width. Reach its
                        // right edge through the same outer horizontal scroll.
                        delta: Vec2::new(-100_000.0, -100_000.0),
                        modifiers: Modifiers::NONE,
                        phase: egui::TouchPhase::Move,
                    },
                ],
                size,
            );
            let mut last = panel.draw_at(&context, &mut time, vec![], size);
            for _ in 0..30 {
                last = panel.draw_at(&context, &mut time, vec![], size);
            }
            let rect = last
                .rect("0x10005f")
                .unwrap_or_else(|| panic!("{size:?}: {:?}", last.text));
            assert!(
                rect.height() >= full_height - 0.5 && rect.bottom() <= size.y,
                "last row must be fully visible: {rect:?}"
            );
            assert!(last.widgets < 100);
        }
    }
}

struct Workbench {
    context: Context,
    workspace: crate::Workspace,
    time: f64,
    size: Vec2,
}

impl Workbench {
    fn draw(&mut self, events: Vec<Event>) -> Vec<(String, Rect)> {
        rendered_at(&self.context, &mut self.time, events, self.size, |ui| {
            assert!(self.workspace.show(ui).is_none());
        })
    }

    fn rect(&mut self, label: &str) -> Rect {
        self.draw(vec![])
            .into_iter()
            .find(|(text, _)| text == label)
            .unwrap_or_else(|| panic!("missing {label}"))
            .1
    }

    // A metadata row can put the first value below the dock's viewport. Use
    // wheel input in the measured inspector, and require both column headers
    // and the value to paint before testing their geometry.
    fn storage_rect(&mut self, label: &str) -> Rect {
        let mut drawn = self.draw(vec![]);
        let bounds = egui::PanelState::load(&self.context, Id::new("world_inspector"))
            .unwrap()
            .outer_rect;
        let row = 21.0_f32.max(self.context.style().spacing.interact_size.y);
        for _ in 0..=(bounds.height() / row).ceil() as usize {
            let target = drawn
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect);
            if let Some(rect) = target
                && ["Slot", "Value"]
                    .iter()
                    .all(|label| drawn.iter().any(|(text, _)| text == label))
            {
                return rect;
            }
            let transient = self.workspace.inspector.tab == Tab::Transient;
            let position = Pos2::new(
                separator(&self.context, transient).rect.center().x,
                bounds.bottom() - row * 0.5,
            );
            self.draw(vec![
                Event::PointerMoved(position),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -row),
                    modifiers: Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ]);
            self.draw(vec![Event::PointerGone]);
            for _ in 0..30 {
                drawn = self.draw(vec![]);
            }
        }
        panic!(
            "{label} and table headers never became visible within one inspector viewport ({bounds:?}): {drawn:?}"
        );
    }

    fn graph_titles(&mut self) -> Vec<(String, Rect)> {
        self.draw(vec![])
            .into_iter()
            .filter(|(text, _)| text.starts_with('σ') && text.contains("  ·  B"))
            .collect()
    }

    fn click(&mut self, label: &str) {
        let position = self.rect(label).center();
        for pressed in [true, false] {
            self.draw(vec![
                Event::PointerMoved(position),
                Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ]);
        }
        self.draw(vec![]);
    }
}

#[test]
fn widths_survive_workbench_changes_and_column_drags_leave_panels_and_graph_still() {
    let mut report = storage_report(32);
    report.accounts[0].transient = report.accounts[0].storage.clone();
    let mut other = account();
    other.address = "0x2222222222222222222222222222222222222222".into();
    other.storage = report.accounts[0].storage.clone();
    report.accounts.push(other);
    report.stores = vec![api::StoreSnapshot {
        storage_facts: vec![],
        transient_facts: vec![],
        accounts: report.accounts.clone(),
        ..super::store()
    }];
    report.states = (0..3)
        .map(|state| api::StateDetails {
            state,
            entry: api::MachineSnapshot {
                path: evm_abstract_protocol::PathRef { node: 4 },
                frames: vec![frame(0, 0)],
                store: 0,
            },
            exit: None,
        })
        .collect();
    report.validate().unwrap();
    let mut workbench = Workbench {
        context: context(),
        workspace: crate::tests::ready(),
        time: 0.0,
        size: Vec2::new(1440.0, 1000.0),
    };
    crate::palette::configure(&workbench.context);
    workbench.workspace.receive(Ok(api::AnalyzeReply {
        result: Ok(report.clone()),
    }));
    assert!(
        workbench.workspace.report.is_some(),
        "valid references must enter the workspace"
    );
    workbench.workspace.account = Some(report.accounts[0].address.clone());
    workbench.workspace.selection.state = Some(0);
    workbench.workspace.inspector.tab = Tab::Storage;
    workbench.workspace.view = View::Graph;
    workbench.workspace.directory.open = true;
    workbench.draw(vec![]);
    let before = workbench.storage_rect("0xcafe");
    let initial_columns = workbench.rect("Value").left() - workbench.rect("Slot").left();
    let panels = ["world_inspector", "world_directory"].map(|name| {
        egui::PanelState::load(&workbench.context, Id::new(name))
            .unwrap()
            .outer_rect
    });
    let graph_summary = workbench.workspace.graph.accessible_summary();
    let zoom = workbench.workspace.graph.zoom;
    let graph_titles = workbench.graph_titles();
    assert!(!graph_titles.is_empty(), "compare actual visible CFG nodes");
    let selected = workbench.workspace.selection;
    let response = separator(&workbench.context, false);
    let start = Pos2::new(response.rect.center().x, workbench.rect("Slot").center().y);
    drag(
        |events| {
            workbench.draw(events);
        },
        start,
        start + Vec2::new(70.0, 0.0),
    );
    let expected = workbench.rect("0xcafe").left();
    near(expected, before.left() + 70.0);
    assert_eq!(workbench.workspace.selection, selected);
    assert_eq!(
        workbench.workspace.graph.accessible_summary(),
        graph_summary
    );
    assert_eq!(workbench.workspace.graph.zoom, zoom);
    assert_eq!(
        workbench.graph_titles(),
        graph_titles,
        "column drag moved the CFG"
    );
    for (name, before) in ["world_inspector", "world_directory"]
        .into_iter()
        .zip(panels)
    {
        assert_eq!(
            egui::PanelState::load(&workbench.context, Id::new(name))
                .unwrap()
                .outer_rect,
            before
        );
    }
    for view in [View::Disassembly, View::Ssa, View::Split, View::Graph] {
        workbench.workspace.view = view;
        near(workbench.storage_rect("0xcafe").left(), expected);
    }
    workbench.click("Maximize");
    near(
        workbench.rect("Value").left() - workbench.rect("Slot").left(),
        initial_columns + 70.0,
    );
    workbench.click("Restore");
    near(workbench.storage_rect("0xcafe").left(), expected);
    workbench.workspace.inspector.tab = Tab::Stack;
    workbench.draw(vec![]);
    workbench.workspace.inspector.tab = Tab::Transient;
    near(workbench.storage_rect("0xcafe").left(), before.left());
    workbench.workspace.inspector.tab = Tab::Storage;
    near(workbench.storage_rect("0xcafe").left(), expected);
    workbench.workspace.selection.state = Some(1);
    near(workbench.storage_rect("0xcafe").left(), expected);
    workbench.workspace.account = Some(report.accounts[1].address.clone());
    let second = workbench.draw(vec![]);
    assert!(
        second
            .iter()
            .any(|(text, _)| text == &report.accounts[1].address)
    );
    near(workbench.storage_rect("0xcafe").left(), expected);
    report.metadata.fingerprint = format!("0x{}", "12".repeat(32));
    report.stores[0].accounts[0].storage[0].slot = format!("0x{}", "ff".repeat(32));
    report.stores[0].accounts[0].storage[0].value = word("0xd00d");
    workbench
        .workspace
        .receive(Ok(api::AnalyzeReply { result: Ok(report) }));
    workbench.workspace.account = Some(
        workbench.workspace.report.as_ref().unwrap().accounts[0]
            .address
            .clone(),
    );
    near(workbench.storage_rect("0xd00d").left(), expected);
    workbench.size = Vec2::new(320.0, 600.0);
    workbench.draw(vec![]);
    workbench.size = Vec2::new(1440.0, 1000.0);
    near(workbench.storage_rect("0xd00d").left(), expected);
}

//! Storage's facts and virtual rows share one viewport; snapshot lookup is exact.

use super::super::{Inspector, Selection, Tab, stores};
use super::{account, frame, rendered_at, store, word};
use egui::{Context, Event, Id, Pos2, Rect};
use evm_abstract_protocol as api;

mod table;

struct Panel<'a> {
    report: &'a api::AnalysisReport,
    store: Option<&'a api::StoreSnapshot>,
    address: &'a str,
    transient: bool,
    height: f32,
}

struct Drawn {
    text: Vec<(String, Rect)>,
    paint: Vec<(Rect, egui::epaint::TextShape)>,
    scroll_id: Id,
    widgets: usize,
}

impl Drawn {
    fn rect(&self, label: &str) -> Option<Rect> {
        self.text
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| *rect)
    }

    fn slots(&self) -> usize {
        self.text
            .iter()
            .filter(|(text, _)| text.starts_with("0x10") && text.len() == 8)
            .count()
    }
}

impl Panel<'_> {
    fn draw(&self, context: &Context, time: &mut f64, events: Vec<Event>) -> Drawn {
        self.draw_at(context, time, events, egui::vec2(900.0, self.height))
    }

    fn draw_at(
        &self,
        context: &Context,
        time: &mut f64,
        events: Vec<Event>,
        size: egui::Vec2,
    ) -> Drawn {
        let mut scroll_id = Id::NULL;
        let mut widgets = 0;
        let mut paint = Vec::new();
        let text = rendered_at(context, time, events, size, |ui| {
            scroll_id = ui.make_persistent_id(egui::IdSalt::new((
                "storage_slots",
                self.transient,
                self.address,
            )));
            stores::storage(ui, self.report, self.store, self.address, self.transient);
            let mut layers = context.memory(|memory| memory.layer_ids().collect::<Vec<_>>());
            if !layers.contains(&ui.layer_id()) {
                layers.push(ui.layer_id());
            }
            paint = context.graphics(|graphics| {
                layers
                    .into_iter()
                    .filter_map(|layer| graphics.get(layer))
                    .flat_map(|layer| layer.all_entries())
                    .filter_map(|shape| match &shape.shape {
                        egui::Shape::Text(text) => Some((shape.clip_rect, text.clone())),
                        _ => None,
                    })
                    .collect()
            });
            // Count actual widget creation, including offscreen widgets. A
            // paint-only count could miss an accidental full-list loop.
            widgets = context.viewport(|viewport| {
                viewport
                    .this_pass
                    .widgets
                    .layers()
                    .map(|(_, widgets)| widgets.len())
                    .sum()
            });
        });
        Drawn {
            text,
            paint,
            scroll_id,
            widgets,
        }
    }

    fn click(&self, context: &Context, time: &mut f64, position: Pos2) -> Drawn {
        for pressed in [true, false] {
            self.draw(
                context,
                time,
                vec![
                    Event::PointerMoved(position),
                    Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        self.draw(context, time, vec![])
    }

    fn scroll(&self, context: &Context, time: &mut f64, position: Pos2, delta: f32) -> Drawn {
        let mut drawn = self.draw(
            context,
            time,
            vec![
                Event::PointerMoved(position),
                Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, delta),
                    modifiers: egui::Modifiers::NONE,
                    phase: egui::TouchPhase::Move,
                },
            ],
        );
        for _ in 0..30 {
            drawn = self.draw(context, time, vec![]);
        }
        drawn
    }

    fn expand(&self, context: &Context, time: &mut f64) -> Drawn {
        let drawn = self.draw(context, time, vec![]);
        let header = drawn.rect("Account facts in this store").unwrap();
        self.click(context, time, header.center())
    }
}

fn context() -> Context {
    let context = Context::default();
    crate::notation::initialize_fonts(&context);
    context.global_style_mut(|style| style.animation_time = 0.0);
    context
}

fn entries(count: usize) -> Vec<api::StorageEntry> {
    (0..count)
        .map(|index| api::StorageEntry {
            slot: format!("0x{:x}", 0x100000 + index),
            value: word("0xcafe"),
        })
        .collect()
}

fn storage_report(count: usize) -> api::AnalysisReport {
    let mut report = crate::tests::report();
    let mut initial = account();
    initial.code_hash = Some("0xfeed".into());
    initial.delegation_target = Some("0x3333333333333333333333333333333333333333".into());
    initial.program = Some(7);
    report.programs.push(crate::tests::source_program(
        7,
        initial.address.clone(),
        vec![],
    ));
    initial.storage = entries(count);
    report.accounts = vec![initial];
    report
}

// Calibrate the fixed metadata/table chrome independently of the large list.
// One vs two actual rows isolates widget cost, including invisible widgets.
fn viewport_widget_budget(panel: &Panel<'_>) -> usize {
    assert!(
        panel.store.is_none(),
        "calibration uses the initial-account fixture"
    );
    let mut reference = panel.report.clone();
    let [(two, pitch), (one, _)] = [2, 1].map(|rows| {
        let account = reference
            .accounts
            .iter_mut()
            .find(|account| account.address == panel.address)
            .unwrap();
        let entries = if panel.transient {
            &mut account.transient
        } else {
            &mut account.storage
        };
        assert!(entries.len() >= rows);
        entries.truncate(rows);
        let sample = Panel {
            report: &reference,
            store: None,
            address: panel.address,
            transient: panel.transient,
            height: panel.height,
        };
        let context = context();
        let mut time = 0.0;
        sample.expand(&context, &mut time);
        let drawn = sample.scroll(
            &context,
            &mut time,
            Pos2::new(240.0, panel.height * 0.5),
            -1_000_000.0,
        );
        assert_eq!(drawn.slots(), rows, "reference rows must actually paint");
        let style = context.style();
        let font = egui::TextStyle::Monospace.resolve(&style);
        let height = 21.0_f32
            .max(context.fonts_mut(|fonts| fonts.row_height(&font)))
            .max(style.spacing.interact_size.y);
        (drawn.widgets, height + style.spacing.item_spacing.y)
    });
    assert!(two > one, "actual row widgets must be counted: {one}/{two}");
    let per_row = two - one;
    let fixed = one
        .checked_sub(per_row)
        .expect("fixed metadata/table widgets");
    // TableBody::rows rounds the viewport endpoints and retains one end row:
    // ceil(viewport/pitch)+2 bounds even a partially clipped starting row.
    let rows = (panel.height / pitch).ceil() as usize + 2;
    let budget = fixed + rows * per_row;
    eprintln!(
        "fixed widgets={fixed}, widgets/row={per_row}, viewport rows={rows}, budget={budget}"
    );
    budget
}

#[test]
fn expanded_facts_use_natural_height_and_the_resized_panel() {
    let report = storage_report(100);
    let mut panel = Panel {
        report: &report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 180.0,
    };
    let context = context();
    let mut time = 0.0;
    let short = panel.expand(&context, &mut time);
    assert!(short.rect("Balance").is_some());
    assert!(short.rect("Pending destruction").is_none());

    panel.height = 600.0;
    let tall = panel.draw(&context, &mut time, vec![]);
    for label in [
        "Existence",
        "Balance",
        "Nonce",
        "Code",
        "Code hash",
        "Delegation implementation",
        "Captured program",
        "Created in transaction",
        "Pending destruction",
        "Base slot default",
    ] {
        assert!(
            tall.rect(label).is_some(),
            "missing {label}: {:?}",
            tall.text
        );
    }
    let facts_height =
        tall.rect("Pending destruction").unwrap().bottom() - tall.rect("Existence").unwrap().top();
    assert!(
        facts_height > 80.0,
        "facts retain their full natural height"
    );
    assert!(tall.slots() > short.slots() + 5);
    let collapsed = panel.click(
        &context,
        &mut time,
        tall.rect("Account facts in this store").unwrap().center(),
    );
    assert!(collapsed.rect("Balance").is_none());
    assert!(collapsed.slots() > tall.slots());
}

// The former test required facts and the first slot to fit together in 180pt.
// Expanded facts now keep their natural height, so retain short-panel coverage
// by scrolling from the facts to the default and first slot in the same area.
#[test]
fn expanded_facts_and_first_slot_are_reachable_in_one_short_panel_scroll() {
    let report = storage_report(32);
    let panel = Panel {
        report: &report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 180.0,
    };
    let context = context();
    let mut time = 0.0;
    let expanded = panel.expand(&context, &mut time);
    assert!(expanded.rect("Balance").is_some());
    assert!(expanded.rect("0x100000").is_none());
    let scrolled = panel.scroll(
        &context,
        &mut time,
        expanded.rect("Balance").unwrap().center(),
        -150.0,
    );
    for label in ["Pending destruction", "Base slot default", "0x100000"] {
        assert!(
            scrolled.rect(label).is_some(),
            "main scroll must reach {label}: {:?}",
            scrolled.text
        );
    }
    assert!(scrolled.rect(panel.address).is_none());
    let state = egui::scroll_area::State::load(&context, scrolled.scroll_id).unwrap();
    assert!(state.offset.y > 100.0);
}

#[test]
fn ten_thousand_slots_create_only_viewport_rows_and_reach_the_last_slot() {
    let report = storage_report(10_000);
    let panel = Panel {
        report: &report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 240.0,
    };
    let widget_budget = viewport_widget_budget(&panel);
    let context = context();
    let mut time = 0.0;
    let first = panel.expand(&context, &mut time);
    assert_eq!(first.slots(), 0, "facts above the list fill this viewport");
    let last = panel.scroll(&context, &mut time, Pos2::new(240.0, 140.0), -1_000_000.0);
    assert!(
        last.rect("0x10270f").is_some(),
        "last slot must be reachable: {:?}",
        last.text
    );
    assert!(last.slots() > 0 && last.slots() <= 12);
    assert!(
        last.widgets <= widget_budget,
        "actual creation {} exceeds fixed-metadata + viewport-row budget {widget_budget}",
        last.widgets
    );
    let state = egui::scroll_area::State::load(&context, last.scroll_id).unwrap();
    assert!(state.offset.y > 200_000.0);
    let returned = panel.scroll(&context, &mut time, Pos2::new(240.0, 140.0), 1_000_000.0);
    assert!(returned.rect("Balance").is_some());
    assert_eq!(returned.slots(), 0);
    assert!(returned.widgets <= widget_budget);
}

#[test]
fn account_and_transient_views_keep_independent_scroll_and_fact_state() {
    let mut report = storage_report(256);
    report.accounts[0].transient = vec![api::StorageEntry {
        slot: "0x300".into(),
        value: word("0xbeef"),
    }];
    let mut second = account();
    second.address = "0x2222222222222222222222222222222222222222".into();
    second.balance = word("0xbbbb");
    second.storage = vec![api::StorageEntry {
        slot: "0x200".into(),
        value: word("0x2222"),
    }];
    report.accounts.push(second);
    let mut panel = Panel {
        report: &report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 180.0,
    };
    let context = context();
    let mut time = 0.0;
    panel.expand(&context, &mut time);
    let last = panel.scroll(&context, &mut time, Pos2::new(240.0, 130.0), -100_000.0);
    assert!(last.rect("0x1000ff").is_some());

    panel.address = &report.accounts[1].address;
    let second = panel.draw(&context, &mut time, vec![]);
    assert_ne!(second.scroll_id, last.scroll_id);
    assert!(second.rect("0x200").is_some());
    assert!(second.rect("0x2222").is_some());
    assert!(second.rect("Balance").is_none());

    panel.address = &report.accounts[0].address;
    panel.transient = true;
    let transient = panel.draw(&context, &mut time, vec![]);
    assert_ne!(transient.scroll_id, last.scroll_id);
    assert!(transient.rect("0x300").is_some());
    assert!(transient.rect("0xbeef").is_some());
    assert!(transient.rect("Balance").is_none());
    assert!(transient.rect("0x1000ff").is_none());

    panel.transient = false;
    let restored = panel.draw(&context, &mut time, vec![]);
    assert_eq!(restored.scroll_id, last.scroll_id);
    assert!(restored.rect("0x1000ff").is_some());
    let top = panel.scroll(&context, &mut time, Pos2::new(240.0, 130.0), 100_000.0);
    assert!(
        top.rect("Balance").is_some(),
        "facts stay expanded for this account"
    );
}

#[test]
fn current_rollback_and_suspended_frame_stores_render_their_own_slots() {
    let mut report = storage_report(1);
    report.accounts[0].storage[0].value = word("0xeeee");
    report.stores = vec![store(), store(), store()];
    for (index, store) in report.stores.iter_mut().enumerate() {
        store.accounts[0].balance = word(&format!("0x{:x}", 0xb000 + index));
        store.accounts[0].storage = vec![api::StorageEntry {
            slot: "0x100000".into(),
            value: word(&format!("0x{:x}", 0xc000 + index)),
        }];
        store.accounts[0].transient = vec![api::StorageEntry {
            slot: "0x300".into(),
            value: word(&format!("0x{:x}", 0x7000 + index)),
        }];
    }
    report.states = vec![api::StateDetails {
        state: 0,
        entry: api::MachineSnapshot {
            path: evm_abstract_protocol::PathRef { node: 4 },
            frames: vec![frame(0, 1), frame(1, 2)],
            store: 0,
        },
        exit: None,
    }];
    let mut selection = Selection {
        state: Some(0),
        ..Selection::default()
    };
    let mut inspector = Inspector {
        tab: Tab::Storage,
        last_state: Some(0),
        ..Inspector::default()
    };
    let context = context();
    let mut time = 0.0;
    for (frame, rollback, transient, expected) in [
        (None, false, false, "0xc000"),
        (None, true, false, "0xc002"),
        (Some(0), true, false, "0xc001"),
        (Some(0), false, false, "0xc000"),
        (Some(0), true, true, "0x7001"),
        (None, true, true, "0x7002"),
    ] {
        inspector.frame = frame;
        inspector.rollback = rollback;
        inspector.tab = if transient {
            Tab::Transient
        } else {
            Tab::Storage
        };
        let visible = rendered_at(
            &context,
            &mut time,
            vec![],
            egui::vec2(1200.0, 600.0),
            |ui| inspector.show(ui, &report, &mut selection, None),
        );
        assert!(
            visible.iter().any(|(text, _)| text == expected),
            "wrong store for frame={frame:?}, rollback={rollback}, transient={transient}: {visible:?}"
        );
        assert!(!visible.iter().any(|(text, _)| text == "0xeeee"));
        for label in ["Current store", "Frame rollback store"] {
            assert!(visible.iter().any(|(text, _)| text == label));
        }
    }
    assert_eq!(report.states[0].entry.store, 0);
}

#[test]
fn missing_accounts_and_explicit_zero_keep_their_own_unknown_or_zero_defaults() {
    let mut report = storage_report(1);
    let unknown = |summary: &str| {
        let mut value = crate::tests::empty_report().metadata.environment.call_value;
        value.summary = summary.into();
        value
    };
    report.accounts[0].storage_default = unknown("Unknown persistent");
    report.accounts[0].transient_default = unknown("Unknown transient");
    report.accounts[0].storage[0].value = word("0x0");
    let mut snapshot = store();
    snapshot.accounts.clear();
    snapshot.storage_default = unknown("Store unknown persistent");
    snapshot.transient_default = word("0x0");
    let mut panel = Panel {
        report: &report,
        store: Some(&snapshot),
        address: &report.accounts[0].address,
        transient: false,
        height: 240.0,
    };
    let context = context();
    let mut time = 0.0;
    let missing = panel.draw(&context, &mut time, vec![]);
    assert!(missing.rect("Store unknown persistent").is_some());
    assert!(missing.rect("Account facts in this store").is_none());
    assert_eq!(missing.slots(), 0);
    assert!(missing.rect("0x0").is_none());

    panel.transient = true;
    let zero_default = panel.draw(&context, &mut time, vec![]);
    assert!(zero_default.rect("0x0").is_some());
    assert!(zero_default.rect("Unknown transient").is_none());

    panel.store = None;
    panel.transient = false;
    let initial = panel.draw(&context, &mut time, vec![]);
    assert!(initial.rect("Unknown persistent").is_some());
    assert!(initial.rect("0x100000").is_some());
    assert!(
        initial.rect("0x0").is_some(),
        "an explicit zero is still shown"
    );
}

#[test]
fn long_slot_values_keep_their_hover_details_in_the_shared_scroll_area() {
    let mut report = storage_report(1);
    report.accounts[0].storage[0].value = word(&format!("0x{}", "ab".repeat(32)));
    let panel = Panel {
        report: &report,
        store: None,
        address: &report.accounts[0].address,
        transient: false,
        height: 600.0,
    };
    let context = context();
    let mut time = 0.0;
    let drawn = panel.draw(&context, &mut time, vec![]);
    let position = drawn
        .rect(&report.accounts[0].storage[0].value.summary)
        .unwrap()
        .center();
    let mut hovered = panel.draw(&context, &mut time, vec![Event::PointerMoved(position)]);
    for _ in 0..15 {
        hovered = panel.draw(&context, &mut time, vec![]);
    }
    for label in [
        "Constants",
        "Known zero bits",
        "Known one bits",
        "Unsigned range",
    ] {
        assert!(
            hovered.rect(label).is_some(),
            "missing {label}: {:?}",
            hovered.text
        );
    }
}

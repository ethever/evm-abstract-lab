//! Real typed pools remain distinct from observed CFG results and source code.
use super::super::{Inspector, Tab, tests::rendered_at};
use super::*;
use crate::app::{
    Selection,
    terminal_coverage::tests::{full_width_report, report},
};
use egui::{Context, Event, Modifiers, PointerButton, Rect};
use evm_abstract_protocol as api;

fn context() -> Context {
    let context = Context::default();
    crate::palette::configure(&context);
    context.global_style_mut(|style| style.animation_time = 0.0);
    context
}

fn center(text: &[(String, Rect)], label: &str) -> egui::Pos2 {
    let expected = evm_abstract_notation::normalize_subscripts(label);
    text.iter()
        .find(|(text, _)| text == &expected)
        .unwrap_or_else(|| panic!("missing {label}: {text:?}"))
        .1
        .center()
}

fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<Event> {
    vec![
        Event::PointerMoved(pos),
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        },
    ]
}

#[test]
fn ui_only_clone_renders_all_three_alternatives_with_no_observed_outcomes() {
    let native_report = report();
    assert!(!native_report.outcomes.is_empty());
    assert!(
        native_report
            .outcomes
            .iter()
            .all(|outcome| outcome.kind == api::OutcomeKind::Failure)
    );
    // This derived clone exercises empty-list rendering only; it is not native
    // execution evidence. Keep the complete certificate, pools and SSA intact.
    let mut ui_only_report = native_report.clone();
    ui_only_report.outcomes.clear();
    ui_only_report.validate().unwrap();
    let coverage = ui_only_report.root_terminal_coverage.as_ref().unwrap();
    let context = context();
    let mut time = 0.0;
    let mut inspector = Inspector {
        tab: Tab::Outcomes,
        ..Inspector::default()
    };
    let original_selection = Selection {
        state: Some(ui_only_report.cfg[0].id),
        pc: Some(0),
        value: None,
    };
    let mut selection = original_selection;
    let mut draw = |events| {
        rendered_at(
            &context,
            &mut time,
            events,
            egui::vec2(1200.0, 1100.0),
            |ui| {
                inspector.outcomes(ui, &ui_only_report, &mut selection);
            },
        )
    };
    draw(vec![]);
    let text = draw(vec![]);
    for expected in [
        "Conservative root terminal coverage",
        "Original input binding",
        "Return alternative",
        "Revert alternative",
        "Failure alternative",
        "Observed terminal outcomes",
        "No terminal root outcome recorded. Check coverage frontiers.",
    ] {
        center(&text, expected);
    }
    for outcome in &coverage.outcomes {
        center(&text, &format!("Data snapshot #{} · length", outcome.data));
        center(
            &text,
            &format!("Finalized store {}", Symbol::Store(outcome.store)),
        );
    }
    let position = center(&text, "Return alternative");
    for pressed in [true, false] {
        draw(pointer(position, pressed));
    }
    let text = draw(vec![]);
    let position = center(&text, "Return alternative");
    for pressed in [true, false] {
        draw(pointer(position, pressed));
    }
    let text = draw(vec![]);
    center(&text, "Failure alternative");
    assert_eq!(selection, original_selection);
    assert!(ui_only_report.outcomes.is_empty());
    assert!(
        ui_only_report
            .frontiers
            .iter()
            .any(|frontier| frontier.kind == api::FrontierKind::Transfers)
    );
    assert!(!ui_only_report.ssa.complete);
    ui_only_report.outcomes.clone_from(&native_report.outcomes);
    assert_eq!(ui_only_report, native_report);
}

#[test]
fn original_binding_retains_full_identity_logical_address_and_captured_program() {
    let report = full_width_report();
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    let input = &coverage.input;
    assert_eq!(input.frame.address, format!("0x{}", "11".repeat(20)));
    assert!(matches!(
        input.frame.address_value,
        api::AddressValue::Symbolic(_)
    ));
    let context = context();
    let mut time = 0.0;
    let mut draw = |events| {
        rendered_at(
            &context,
            &mut time,
            events,
            egui::vec2(1600.0, 1800.0),
            |ui| {
                binding(ui, &report, coverage);
            },
        )
    };
    draw(vec![]);
    let text = draw(vec![]);
    for expected in [
        "All states matching the original input binding",
        "Offline snapshot",
        "web-bytecode",
        "Offline namespace",
        "Fact revision",
        "18446744073709551615",
        "18446744073709551614",
        "Original input scope",
        "Storage source expression",
        "Original storage source",
        "Physical storage owner",
        "symbolic To",
        "symbolic Caller",
        "Captured program",
        "Original calldata",
        "Original execution configuration",
        "Original domain policy",
    ] {
        // Source graph headers include the actual root index after the label.
        assert!(
            text.iter().any(|(label, _)| label.contains(expected)),
            "{expected}: {text:?}"
        );
    }
    center(&text, &input.frame.address);
    center(&text, &input.frame.code_hash);
    let position = center(&text, "Captured executing program");
    for pressed in [true, false] {
        draw(pointer(position, pressed));
    }
    let text = draw(vec![]);
    let program = report
        .programs
        .iter()
        .find(|program| program.id == input.program)
        .unwrap();
    center(&text, &program.bytecode);
    center(
        &text,
        &format!("Saved store {}", Symbol::Store(input.saved_store)),
    );
    center(
        &text,
        &format!("Initial store {}", Symbol::Store(input.initial_store)),
    );
    assert_eq!(
        input.call_value.identity.as_ref().unwrap().scope,
        api::DecimalU64(u64::MAX)
    );
    assert_eq!(input.frame.basic_block_index, 0);
    assert_eq!(input.frame.stack_height, 0);
    assert!(input.frame.jump_history.is_empty());
}

#[test]
fn terminal_bytes_come_from_each_alternatives_actual_pool_reference() {
    let report = report();
    let coverage = report.root_terminal_coverage.as_ref().unwrap();
    for outcome in &coverage.outcomes {
        let context = context();
        let mut time = 0.0;
        let mut draw = |events| {
            rendered_at(
                &context,
                &mut time,
                events,
                egui::vec2(1200.0, 900.0),
                |ui| {
                    alternative(ui, &report, outcome);
                },
            )
        };
        draw(vec![]);
        let text = draw(vec![]);
        let position = center(&text, "Terminal bytes");
        for pressed in [true, false] {
            draw(pointer(position, pressed));
        }
        let text = draw(vec![]);
        center(&text, "Length");
        center(&text, "Base byte default");
        let data = &report.byte_arrays[outcome.data];
        center(&text, &data.length.summary);
        center(&text, &data.default.summary);
        assert!(text.iter().any(|(label, _)| label
            == &format!(
                "Sources, writes and copy ranges · root #{}",
                data.symbolic.node
            )));
        if outcome.kind == api::OutcomeKind::Failure {
            assert_eq!(data.length.unsigned.lower, "0x0");
            assert_eq!(data.length.unsigned.upper, "0x0");
        }
    }
}

#[test]
fn ordinary_store_defaults_show_actual_bases_and_unknown_code_without_source_fallback() {
    let mut report = report();
    let coverage = report.root_terminal_coverage.take().unwrap();
    report.metadata.limits.root_terminal_coverage = false;
    let returned = &report.stores[coverage.outcomes[0].store];
    assert!(returned.code_default.is_none());
    assert!(returned.accounts.is_empty());
    assert!(
        report
            .accounts
            .iter()
            .any(|account| account.code_kind == api::CodeKind::Runtime)
    );
    let context = context();
    let mut time = 0.0;
    let mut draw = |events| {
        rendered_at(
            &context,
            &mut time,
            events,
            egui::vec2(1200.0, 1000.0),
            |ui| {
                stores::changes(ui, &report, returned);
            },
        )
    };
    draw(vec![]);
    let text = draw(vec![]);
    assert!(
        !text
            .iter()
            .any(|(label, _)| label == "Unlisted account nonce")
    );
    let position = center(&text, "Unlisted accounts and storage bases");
    for pressed in [true, false] {
        draw(pointer(position, pressed));
    }
    let text = draw(vec![]);
    for expected in [
        "Persistent slot default",
        "Transient slot default",
        "Unlisted account balance",
        "Unlisted account nonce",
        "Unlisted account existence",
        "Unlisted account code",
        "Unlisted account creation",
        "Unlisted account pending destruction",
        "Unknown",
        "Storage bases are scoped by account owner when read. A shared base does not imply equal values for different owners.",
    ] {
        center(&text, expected);
    }
    center(&text, &returned.balance_default.summary);
    center(&text, &returned.nonce_default.summary);
    center(
        &text,
        &format!(
            "Persistent base template · root #{}",
            returned.storage_root.node
        ),
    );
    center(
        &text,
        &format!(
            "Transient base template · root #{}",
            returned.transient_root.node
        ),
    );
    assert!(!text.iter().any(|(label, _)| label == "Runtime"));
    assert!(
        !text
            .iter()
            .any(|(label, _)| label == &report.programs[0].bytecode)
    );
    report.validate().unwrap();
}

#[test]
fn unlisted_code_defaults_preserve_runtime_empty_delegation_and_unknown_meanings() {
    let mut report = report();
    let coverage = report.root_terminal_coverage.take().unwrap();
    let mut store = report.stores[coverage.outcomes[0].store].clone();
    for (code, kind, detail) in [
        (None, "Unknown", None),
        (Some(api::AccountCode::Unknown), "Unknown", None),
        (Some(api::AccountCode::Empty), "Empty", None),
        (
            Some(api::AccountCode::Runtime("0x602a00".into())),
            "Runtime",
            Some("0x602a00"),
        ),
        (
            Some(api::AccountCode::Delegation(format!(
                "0x{}",
                "22".repeat(20)
            ))),
            "Delegation",
            Some("0x2222222222222222222222222222222222222222"),
        ),
    ] {
        store.code_default = code;
        let context = context();
        let mut time = 0.0;
        let mut draw = |events| {
            rendered_at(
                &context,
                &mut time,
                events,
                egui::vec2(1200.0, 1000.0),
                |ui| {
                    stores::changes(ui, &report, &store);
                },
            )
        };
        draw(vec![]);
        let text = draw(vec![]);
        let position = center(&text, "Unlisted accounts and storage bases");
        for pressed in [true, false] {
            draw(pointer(position, pressed));
        }
        let text = draw(vec![]);
        center(&text, "Unlisted account code");
        center(&text, kind);
        if let Some(detail) = detail {
            center(&text, detail);
        }
    }
}

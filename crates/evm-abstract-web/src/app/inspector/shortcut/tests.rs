use egui::{Context, Event, Id, ImeEvent, Key, Modifiers, RawInput, os::OperatingSystem};

use super::Shortcut;

// Exercise the browser's production delivery state on the host as well. DOM
// identity and event cancellation are covered by the browser acceptance checks.
#[path = "../../../framework/browser/shortcut/state.rs"]
mod browser_state;

fn key(pressed: bool, modifiers: Modifiers) -> Event {
    Event::Key {
        key: Key::J,
        physical_key: Some(Key::J),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn raw(events: Vec<Event>) -> RawInput {
    RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1440.0, 1000.0),
        )),
        events,
        ..RawInput::default()
    }
}

fn run(ctx: &Context, input: RawInput, contents: impl FnMut(&mut egui::Ui)) {
    let mut output = ctx.run_ui(input, contents);
    output.textures_delta.clear();
}

fn consume(ctx: &Context, shortcut: &mut Shortcut, input: RawInput) -> (usize, Vec<Event>) {
    let mut presses = 0;
    let mut remaining = vec![];
    run(ctx, input, |ui| {
        presses = shortcut.consume(ui.ctx(), true);
        remaining = ui.input(|input| input.events.clone());
    });
    (presses, remaining)
}

#[test]
fn inspect_chord_and_hover_use_the_actual_platform_and_exact_modifiers() {
    for os in [
        OperatingSystem::Windows,
        OperatingSystem::Nix,
        OperatingSystem::Mac,
    ] {
        let ctx = Context::default();
        ctx.set_os(os);
        assert_eq!(
            super::hint(&ctx),
            if os == OperatingSystem::Mac {
                "Show/hide Inspect (Cmd+J)"
            } else {
                "Show/hide Inspect (Ctrl+J)"
            }
        );
        for bits in 0..32 {
            let modifiers = Modifiers {
                ctrl: bits & 1 != 0,
                mac_cmd: bits & 2 != 0,
                alt: bits & 4 != 0,
                shift: bits & 8 != 0,
                command: bits & 16 != 0,
            };
            let expected = if os == OperatingSystem::Mac {
                [2, 18].contains(&bits)
            } else {
                [1, 17].contains(&bits)
            };
            let mut shortcut = Shortcut::default();
            let (presses, remaining) = consume(
                &ctx,
                &mut shortcut,
                raw(vec![key(true, modifiers), key(false, modifiers)]),
            );
            assert_eq!(presses, usize::from(expected), "{os:?}, {modifiers:?}");
            assert_eq!(remaining.len(), if expected { 1 } else { 2 });
        }
    }
}

#[test]
fn held_keys_do_not_repeat_but_two_distinct_presses_are_both_counted() {
    let ctx = Context::default();
    let mut shortcut = Shortcut::default();
    assert_eq!(
        consume(&ctx, &mut shortcut, raw(vec![key(true, Modifiers::CTRL)])).0,
        1
    );
    for _ in 0..3 {
        assert_eq!(
            consume(&ctx, &mut shortcut, raw(vec![key(true, Modifiers::CTRL)])).0,
            0
        );
    }
    assert_eq!(
        consume(
            &ctx,
            &mut shortcut,
            raw(vec![
                key(false, Modifiers::CTRL),
                key(true, Modifiers::CTRL),
                key(false, Modifiers::CTRL),
                key(true, Modifiers::CTRL),
                key(false, Modifiers::CTRL),
            ])
        )
        .0,
        2
    );
}

#[test]
fn composition_owns_keys_until_commit_and_unfocused_workspaces_leave_input_alone() {
    let ctx = Context::default();
    let mut shortcut = Shortcut::default();
    let (presses, remaining) = consume(
        &ctx,
        &mut shortcut,
        raw(vec![
            Event::Ime(ImeEvent::Preedit {
                text: "候选".into(),
                active_range_chars: None,
            }),
            key(true, Modifiers::CTRL),
            key(false, Modifiers::CTRL),
        ]),
    );
    assert_eq!(presses, 0);
    assert_eq!(remaining.len(), 3);
    assert_eq!(
        consume(
            &ctx,
            &mut shortcut,
            raw(vec![
                key(true, Modifiers::CTRL),
                key(false, Modifiers::CTRL)
            ])
        )
        .0,
        0
    );
    assert_eq!(
        consume(
            &ctx,
            &mut shortcut,
            raw(vec![
                Event::Ime(ImeEvent::Commit("候选".into())),
                key(true, Modifiers::CTRL),
                key(false, Modifiers::CTRL),
            ])
        )
        .0,
        1
    );
    let mut input = raw(vec![
        key(true, Modifiers::CTRL),
        key(false, Modifiers::CTRL),
    ]);
    input.focused = false;
    let (presses, remaining) = consume(&ctx, &mut shortcut, input);
    assert_eq!(presses, 0);
    assert_eq!(remaining.len(), 2);
}

#[test]
fn ordinary_text_edit_accepts_the_chord_and_preserves_draft_and_cursor_selection() {
    use egui::{
        text::{CCursor, CCursorRange},
        text_edit::TextEditState,
    };

    for os in [
        OperatingSystem::Windows,
        OperatingSystem::Nix,
        OperatingSystem::Mac,
    ] {
        let ctx = Context::default();
        ctx.set_os(os);
        let modifiers = if os.is_mac() {
            Modifiers::MAC_CMD | Modifiers::COMMAND
        } else {
            Modifiers::CTRL
        };
        let mut shortcut = Shortcut::default();
        let id = Id::new("shortcut_editor");
        let mut draft = "17".to_owned();
        run(&ctx, raw(vec![]), |ui| {
            crate::framework::line_editor(ui, "shortcut_editor", &mut draft, "Value ID")
                .request_focus();
        });
        assert!(ctx.text_edit_focused());
        let selection = CCursorRange::two(CCursor::new(1), CCursor::new(2));
        let mut state = TextEditState::load(&ctx, id).unwrap();
        state.cursor.set_char_range(Some(selection));
        state.store(&ctx, id);

        let release = key(false, modifiers);
        run(
            &ctx,
            raw(vec![key(true, modifiers), release.clone()]),
            |ui| {
                assert_eq!(shortcut.consume(ui.ctx(), true), 1, "{os:?}");
                // Consumption happens before the focused editor sees the frame.
                assert_eq!(
                    ui.input(|input| input.events.clone()),
                    vec![release.clone()]
                );
                crate::framework::line_editor(ui, "shortcut_editor", &mut draft, "Value ID");
            },
        );
        assert_eq!(draft, "17");
        assert_eq!(
            TextEditState::load(&ctx, id).unwrap().cursor.char_range(),
            Some(selection)
        );
        assert!(ctx.text_edit_focused());

        run(&ctx, raw(vec![Event::Text("3".into())]), |ui| {
            assert_eq!(shortcut.consume(ui.ctx(), true), 0);
            crate::framework::line_editor(ui, "shortcut_editor", &mut draft, "Value ID");
        });
        assert_eq!(
            draft, "13",
            "ordinary editing still replaces the selected digit"
        );
    }
}

#[test]
fn popup_modal_and_text_edit_composition_keep_their_keyboard_events() {
    let events = vec![key(true, Modifiers::CTRL), key(false, Modifiers::CTRL)];
    let mut shortcut = Shortcut::default();

    let ctx = Context::default();
    egui::Popup::open_id(&ctx, Id::new("shortcut_popup"));
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, events);

    let ctx = Context::default();
    let mut draft = "17".to_owned();
    run(&ctx, raw(vec![]), |ui| {
        egui::Modal::new(Id::new("shortcut_modal")).show(ui.ctx(), |ui| {
            crate::framework::line_editor(ui, "shortcut_modal_editor", &mut draft, "Value ID")
                .request_focus();
        });
    });
    assert!(ctx.text_edit_focused());
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, events);

    let ctx = Context::default();
    run(&ctx, raw(vec![]), |ui| {
        crate::framework::line_editor(ui, "shortcut_ime_editor", &mut draft, "Value ID")
            .request_focus();
    });
    assert!(ctx.text_edit_focused());
    let mut composing_events = vec![Event::Ime(ImeEvent::Preedit {
        text: "候选".into(),
        active_range_chars: None,
    })];
    composing_events.extend(events);
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(composing_events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, composing_events);
}

#[test]
fn browser_delivery_survives_canvas_text_agent_focus_transitions_and_suppresses_repeats() {
    use browser_state::{State, Target};

    let state = State::default();
    assert!(!state.keydown(Target::Canvas, true, false, false));
    state.set_available(true);
    assert!(state.keydown(Target::Canvas, true, false, false));
    // The click/repaint focus transition must not erase an accepted press.
    state.blur(Target::Canvas);
    assert!(state.keydown(Target::TextAgent, true, false, true));
    assert_eq!(state.take_presses(), 1);
    assert_eq!(state.take_presses(), 0);

    assert!(state.keydown(Target::TextAgent, true, false, false));
    state.blur(Target::TextAgent);
    assert!(state.keydown(Target::Canvas, true, false, true));
    assert_eq!(state.take_presses(), 1);

    // A stale egui text focus cannot authorize a different DOM target.
    for target in [Target::Canvas, Target::TextAgent] {
        assert!(!state.keydown(target, false, false, false));
        assert!(state.keydown(target, true, false, true));
    }
    assert_eq!(state.take_presses(), 0);
}

#[test]
fn browser_composition_and_availability_are_scoped_to_owned_targets() {
    use browser_state::{State, Target};

    let state = State::default();
    state.set_available(true);
    for target in [Target::Canvas, Target::TextAgent] {
        state.composition(target, true);
        assert!(!state.keydown(target, true, false, false));
        assert!(!state.keydown(target, true, false, true));
        state.composition(target, false);
        assert!(!state.keydown(target, true, true, false));
        assert!(state.keydown(target, true, false, false));
        state.blur(target);
    }
    assert_eq!(state.take_presses(), 2);

    state.composition(Target::Canvas, true);
    assert!(state.keydown(Target::TextAgent, true, false, false));
    assert!(!state.keydown(Target::Canvas, true, false, false));
    state.blur(Target::Canvas);
    assert!(state.keydown(Target::Canvas, true, false, false));
    state.set_available(false); // analysis form/modal
    assert!(!state.keydown(Target::Canvas, true, false, false));
    assert!(!state.keydown(Target::TextAgent, true, false, true));
    assert_eq!(state.take_presses(), 2);
}

#[test]
fn workbench_shortcut_toggles_once_and_analysis_form_keeps_priority() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = crate::tests::ready();
    let mut draft = "17".to_owned();
    run(&ctx, raw(vec![]), |ui| {
        crate::framework::line_editor(ui, "workbench_value_editor", &mut draft, "Value ID")
            .request_focus();
    });
    assert!(ctx.text_edit_focused());
    let selection = workspace.selection;
    run(&ctx, raw(vec![key(true, Modifiers::CTRL)]), |ui| {
        assert!(workspace.show(ui).is_none());
    });
    assert!(!workspace.inspector.expanded);
    assert_eq!(workspace.selection, selection);
    assert_eq!(draft, "17");
    run(&ctx, raw(vec![key(true, Modifiers::CTRL)]), |ui| {
        workspace.show(ui);
    });
    assert!(!workspace.inspector.expanded);
    run(
        &ctx,
        raw(vec![
            key(false, Modifiers::CTRL),
            key(true, Modifiers::CTRL),
            key(false, Modifiers::CTRL),
        ]),
        |ui| {
            workspace.show(ui);
        },
    );
    assert!(workspace.inspector.expanded);
    assert_eq!(workspace.selection, selection);
    workspace.form.open = true;
    run(
        &ctx,
        raw(vec![
            key(true, Modifiers::CTRL),
            key(false, Modifiers::CTRL),
        ]),
        |ui| {
            workspace.show(ui);
        },
    );
    assert!(workspace.inspector.expanded);
    assert!(workspace.form.open);
}

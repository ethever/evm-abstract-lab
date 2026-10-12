use egui::{Context, Event, Id, ImeEvent, Key, Modifiers, RawInput, os::OperatingSystem};

use super::Shortcut;

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
fn text_edit_popup_and_modal_keep_their_keyboard_events() {
    let ctx = Context::default();
    let mut shortcut = Shortcut::default();
    let mut text = String::new();
    run(&ctx, raw(vec![]), |ui| {
        crate::framework::line_editor(ui, "shortcut_editor", &mut text, "Editor").request_focus();
    });
    assert!(ctx.text_edit_focused());
    let events = vec![key(true, Modifiers::CTRL), key(false, Modifiers::CTRL)];
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, events);

    let ctx = Context::default();
    egui::Popup::open_id(&ctx, Id::new("shortcut_popup"));
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, events);

    let ctx = Context::default();
    run(&ctx, raw(vec![]), |ui| {
        egui::Modal::new(Id::new("shortcut_modal")).show(ui.ctx(), |ui| {
            ui.label("Modal keyboard owner");
        });
    });
    let (presses, remaining) = consume(&ctx, &mut shortcut, raw(events.clone()));
    assert_eq!(presses, 0);
    assert_eq!(remaining, events);
}

#[test]
fn workbench_shortcut_toggles_once_and_analysis_form_keeps_priority() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = crate::tests::ready();
    run(&ctx, raw(vec![key(true, Modifiers::CTRL)]), |ui| {
        assert!(workspace.show(ui).is_none());
    });
    assert!(!workspace.inspector.expanded);
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

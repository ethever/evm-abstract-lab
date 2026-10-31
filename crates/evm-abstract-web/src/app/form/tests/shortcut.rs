use super::*;

#[test]
fn ime_commit_frame_and_repeat_do_not_submit_or_close_the_form() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut form = AnalysisForm {
        open: true,
        ..Default::default()
    };
    frame(&ctx, &mut form, vec![]);
    let ime = Event::Ime(egui::ImeEvent::Preedit {
        text: "60".into(),
        active_range_chars: None,
    });
    assert!(frame(&ctx, &mut form, vec![ime, enter()]).is_none());
    assert!(frame(&ctx, &mut form, vec![enter()]).is_none());
    assert!(
        frame(
            &ctx,
            &mut form,
            vec![Event::Ime(egui::ImeEvent::Commit("60".into())), enter()]
        )
        .is_none()
    );
    assert!(form.open);
    assert!(frame(&ctx, &mut form, vec![enter()]).is_some());
}

#[test]
fn held_submit_chord_is_consumed_without_a_second_submission() {
    let ctx = Context::default();
    let mut keyboard = super::super::shortcut::Keyboard::default();
    let mut submissions = Vec::new();
    for _ in 0..2 {
        let mut output = ctx.run_ui(
            RawInput {
                events: vec![enter()],
                ..Default::default()
            },
            |ui| {
                submissions.push(keyboard.submit(ui.ctx()));
            },
        );
        output.textures_delta.clear();
    }
    assert_eq!(submissions, [true, false]);
}

#[test]
fn workspace_open_chord_does_not_also_submit_the_newly_opened_editor() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut workspace = crate::Workspace::default();
    let mut command = None;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events: vec![enter()],
            ..Default::default()
        },
        |ui| {
            command = workspace.show(ui);
        },
    );
    output.textures_delta.clear();
    assert!(workspace.form.open);
    assert!(command.is_none());
    assert_eq!(workspace.task.phase, crate::app::job::TaskPhase::Idle);
}

#[test]
fn closing_and_immediately_reopening_requests_the_source_editor_focus_again() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut form = AnalysisForm {
        open: true,
        ..Default::default()
    };
    frame(&ctx, &mut form, vec![]);
    frame(&ctx, &mut form, vec![]);
    frame(
        &ctx,
        &mut form,
        vec![Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert!(!form.open);
    // No intervening hidden-form frame: this is the browser's close/reopen race.
    form.open = true;
    frame(&ctx, &mut form, vec![]);
    assert_eq!(
        ctx.memory(|memory| memory.focused()),
        Some(crate::framework::BytecodeEditor::Analysis.id())
    );
}

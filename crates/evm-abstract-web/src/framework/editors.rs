use super::{BytecodeEditor, bytecode_editor};
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn frame(ctx: &Context, editor: BytecodeEditor, text: &mut String, time: f64, events: Vec<Event>) {
    ctx.memory_mut(|memory| memory.request_focus(editor.id()));
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0))),
            time: Some(time),
            events,
            ..RawInput::default()
        },
        |ui| bytecode_editor(ui, editor, text, 80.0),
    );
    output.textures_delta.clear();
}

#[test]
fn bytecode_editors_keep_real_undo_redo_cursor_and_composition_ownership_separate() {
    let ctx = Context::default();
    crate::palette::configure(&ctx);
    let mut analysis = "6001".to_owned();
    let mut decompiler = "6002".to_owned();
    let a = BytecodeEditor::Analysis;
    let b = BytecodeEditor::Decompiler;
    let command = Modifiers {
        ctrl: true,
        command: true,
        ..Modifiers::NONE
    };
    frame(&ctx, a, &mut analysis, 0.0, vec![]);
    frame(
        &ctx,
        a,
        &mut analysis,
        0.1,
        vec![key(Key::End, Modifiers::NONE), Event::Text("aa".into())],
    );
    frame(&ctx, a, &mut analysis, 1.0, vec![]);
    assert_eq!(analysis, "6001aa");
    frame(&ctx, b, &mut decompiler, 2.0, vec![]);
    frame(
        &ctx,
        b,
        &mut decompiler,
        2.1,
        vec![key(Key::End, Modifiers::NONE), Event::Text("bb".into())],
    );
    frame(&ctx, b, &mut decompiler, 3.0, vec![]);
    frame(&ctx, b, &mut decompiler, 3.1, vec![key(Key::Z, command)]);
    assert_eq!(decompiler, "6002");
    assert_eq!(analysis, "6001aa");
    frame(&ctx, b, &mut decompiler, 3.2, vec![key(Key::Y, command)]);
    assert_eq!(decompiler, "6002bb");
    // Reopening the first owner uses its own real TextEditState.
    frame(&ctx, a, &mut analysis, 4.0, vec![key(Key::Z, command)]);
    assert_eq!(analysis, "6001");
    frame(&ctx, a, &mut analysis, 4.1, vec![key(Key::Y, command)]);
    assert_eq!(analysis, "6001aa");
    frame(
        &ctx,
        a,
        &mut analysis,
        4.2,
        vec![Event::Ime(egui::ImeEvent::Preedit {
            text: "cc".into(),
            active_range_chars: None,
        })],
    );
    let analysis_before_switch = analysis.clone();
    frame(&ctx, b, &mut decompiler, 5.0, vec![]);
    assert_eq!(decompiler, "6002bb");
    assert_eq!(analysis, analysis_before_switch);
    assert_eq!(ctx.memory(|memory| memory.focused()), Some(b.id()));
    let saved_a = egui::text_edit::TextEditState::load(&ctx, a.id()).unwrap();
    let saved_b = egui::text_edit::TextEditState::load(&ctx, b.id()).unwrap();
    assert_ne!(saved_a.cursor.char_range(), saved_b.cursor.char_range());
    // Explicit captured-source replacement intentionally starts fresh.
    b.replace(&ctx);
    decompiler = "6003".into();
    frame(&ctx, b, &mut decompiler, 6.0, vec![key(Key::Z, command)]);
    assert_eq!(decompiler, "6003");
    assert_eq!(analysis, analysis_before_switch);
}

use super::{Application, Command, Message, Mode};
use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};

fn frame(application: &mut Application, context: &Context, events: Vec<Event>) -> Option<Command> {
    let mut command = None;
    let mut output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
            events,
            ..RawInput::default()
        },
        |ui| {
            command = application.show(ui);
        },
    );
    output.textures_delta.clear();
    command
}

#[test]
fn independent_workspace_keeps_analysis_and_submits_captured_bytes_without_a_new_analysis() {
    let context = Context::default();
    crate::palette::configure(&context);
    let mut app = Application::default();
    assert!(matches!(
        app.startup_command(),
        Some(Command::Analysis(crate::Command::RpcProviders { .. }))
    ));
    assert!(app.rpc_provider_command().is_none());
    app.receive_message(Message::Analysis(crate::Message::Status {
        generation: u64::MAX,
        operation: crate::JobOperation::Poll,
        result: Box::new(Err(crate::TransportError::Http { status: 500 })),
    }));
    app.receive_message(Message::Decompile(crate::decompile::Message::Status {
        generation: u64::MAX,
        operation: crate::JobOperation::Poll,
        result: Box::new(Err(crate::TransportError::Http { status: 500 })),
    }));
    app.analysis
        .receive(Ok(evm_abstract_protocol::AnalyzeReply {
            result: Ok(crate::tests::report().into()),
        }));
    frame(&mut app, &context, vec![]);
    let before = app.analysis.accessible_status();
    let (bytes, fork) = app.analysis.captured_decompile_input().unwrap();
    let expected = bytes.to_owned();
    app.decompile.load_input(expected.clone(), fork);
    app.mode = Mode::Decompiler;
    app.prepare_input(&mut RawInput::default());
    assert!(app.accessible_status().contains("Workspace: Decompiler"));
    assert!(!app.inspector_shortcut_available());
    assert!(app.accessible_input_status().is_none());
    assert!(!app.accessible_rpc_provider_status().is_empty());
    app.toggle_inspector();
    frame(&mut app, &context, vec![]);
    frame(&mut app, &context, vec![]);
    let command = frame(
        &mut app,
        &context,
        vec![Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL,
        }],
    );
    match command {
        Some(Command::Decompile(crate::decompile::Command::Submit {
            request,
            generation,
        })) => {
            assert_eq!(request.bytecode, expected);
            assert_eq!(request.fork, fork);
            assert_eq!(
                request.strategy,
                evm_abstract_protocol::DecompileStrategy::Dispatcher
            );
            assert!(generation > 0);
        }
        _ => panic!("the independent input must submit a decompiler command"),
    }
    app.mode = Mode::Analysis;
    assert!(app.background_command(&context).is_none());
    frame(&mut app, &context, vec![]);
    assert_eq!(app.analysis.accessible_status(), before);
    assert!(app.accessible_status().contains("Workspace: Analysis"));
}

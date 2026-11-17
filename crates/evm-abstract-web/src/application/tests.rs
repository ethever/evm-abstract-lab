use super::{Application, Command, Message};
use crate::routing::{Page, Route};
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
    assert!(
        app.decompile.is_none(),
        "foreign late callbacks do not construct a page"
    );
    app.analysis
        .as_mut()
        .unwrap()
        .receive(Ok(evm_abstract_protocol::AnalyzeReply {
            result: Ok(crate::tests::report().into()),
        }));
    frame(&mut app, &context, vec![]);
    let before = app.analysis.as_ref().unwrap().accessible_status();
    let (bytes, fork) = app
        .analysis
        .as_ref()
        .unwrap()
        .captured_decompile_input()
        .unwrap();
    let expected = bytes.to_owned();
    app.activate(Route::page(Page::Decompiler));
    app.decompile
        .as_mut()
        .unwrap()
        .load_input(expected.clone(), fork);
    app.prepare_input(&mut RawInput::default());
    assert!(app.accessible_status().contains("Workspace: Decompiler"));
    assert!(!app.inspector_shortcut_available());
    assert!(app.accessible_input_status().is_none());
    assert!(app.accessible_rpc_provider_status().is_empty());
    assert!(app.startup_command().is_none());
    assert!(app.rpc_provider_command().is_none());
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
    app.activate(Route::default());
    assert!(app.cleanup_command().is_none());
    frame(&mut app, &context, vec![]);
    assert_eq!(app.analysis.as_ref().unwrap().accessible_status(), before);
    assert!(app.accessible_status().contains("Workspace: Analysis"));
}

#[test]
fn direct_decompiler_and_invalid_links_never_construct_or_start_analysis() {
    for query in ["", "?sha256=bad", "?address=bad&sha256=bad"] {
        let context = Context::default();
        crate::palette::configure(&context);
        let mut app = Application::new(Route::parse("/decompiler", query));
        assert!(app.analysis.is_none());
        assert!(app.startup_command().is_none());
        assert!(app.rpc_provider_command().is_none());
        assert!(frame(&mut app, &context, vec![]).is_none());
        assert!(app.analysis.is_none());
        assert!(app.accessible_status().contains("Workspace: Decompiler"));
        if !query.is_empty() {
            assert!(app.accessible_status().contains("Invalid decompiler link"));
        }
    }
}

#[test]
fn explicit_navigation_pushes_a_real_route_while_history_activation_does_not() {
    let mut app = Application::default();
    app.navigate(Route::page(Page::Decompiler));
    let navigation = app.take_navigation().unwrap();
    assert_eq!(navigation.route.url().as_deref(), Some("/decompiler"));
    assert_eq!(navigation.history, super::History::Push);
    assert!(app.take_navigation().is_none());
    app.activate(Route::default());
    assert!(app.take_navigation().is_none());
    app.navigation_failed("history unavailable".into());
    assert!(
        app.accessible_status()
            .contains("Navigation error: history unavailable")
    );
}

#[test]
fn late_provider_reply_cannot_start_a_hidden_analysis() {
    let context = Context::default();
    crate::palette::configure(&context);
    let mut app = Application::default();
    let Some(Command::Analysis(crate::Command::RpcProviders { generation })) =
        app.startup_command()
    else {
        panic!("fresh analyzer loads its catalog")
    };
    app.activate(Route::page(Page::Decompiler));
    app.receive_message(Message::Analysis(crate::Message::RpcProviders {
        generation,
        result: Ok(evm_abstract_protocol::RpcProvidersReply {
            result: Ok(Vec::new()),
        }),
    }));
    assert!(frame(&mut app, &context, vec![]).is_none());
    assert!(app.startup_command().is_none());
    assert!(app.rpc_provider_command().is_none());
    assert!(app.cleanup_command().is_none());
    app.activate(Route::default());
    assert!(matches!(
        frame(&mut app, &context, vec![]),
        Some(Command::Analysis(crate::Command::Submit { .. }))
    ));
}

#[test]
fn an_accepted_source_link_is_not_lost_when_the_user_switches_pages_in_that_frame() {
    use evm_abstract_protocol::{self as api, DecompileSettings};
    let request = api::DecompileRequest::default();
    let source = api::DecompileSource {
        sha256: api::decompile_sha256(&[0x60, 2, 0x60, 3, 1, 0]),
        config_sha256: DecompileSettings::from_request(&request).sha256(),
        origin: None,
        cache_hit: false,
    };
    let context = Context::default();
    crate::palette::configure(&context);
    let mut app = Application::new(Route::parse(
        "/decompiler",
        &format!("?sha256={}", source.sha256),
    ));
    let Some(Command::Decompile(crate::decompile::Command::Open { generation, .. })) =
        frame(&mut app, &context, vec![])
    else {
        panic!("source link must open")
    };
    app.receive_message(Message::Decompile(crate::decompile::Message::Opened {
        generation,
        result: Box::new(Ok(api::DecompileOpenReply {
            result: Ok(api::DecompileOpened {
                request,
                source: source.clone(),
                snapshot: api::DecompileJobSnapshot {
                    id: api::JobId(12),
                    state: api::JobState::Queued,
                    progress: Default::default(),
                    queue_ns: api::DecimalU64(0),
                    processing_ns: api::DecimalU64(0),
                    publication: None,
                    cache_warning: None,
                },
            }),
        })),
    }));
    app.navigate(Route::default());
    let canonical = app.take_navigation().unwrap();
    assert_eq!(canonical.route, Route::canonical(&source));
    assert_eq!(canonical.history, super::History::Replace);
    let page = app.take_navigation().unwrap();
    assert_eq!(page.route, Route::default());
    assert_eq!(page.history, super::History::Push);
    assert!(app.take_navigation().is_none());
}

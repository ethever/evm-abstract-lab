use super::{Artifact, Command, Message, Strategy, Workspace, fixture};
use crate::{
    application::History,
    routing::{Page, Route, Source},
};
use evm_abstract_protocol::{self as api, DecompileOpened, DecompileSettings, DecompileSource};

fn opened(artifact: &Artifact, id: u64) -> DecompileOpened {
    let settings = DecompileSettings {
        fork: match artifact.input.fork {
            api::decompile_model::Fork::Cancun => api::Fork::Cancun,
            api::decompile_model::Fork::Prague => api::Fork::Prague,
            api::decompile_model::Fork::Osaka => api::Fork::Osaka,
        },
        config: artifact.config.clone(),
    };
    let request = settings.clone().into_request(
        artifact
            .input
            .runtime_bytecode
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    );
    DecompileOpened {
        request,
        source: DecompileSource {
            sha256: api::decompile_sha256(&artifact.input.runtime_bytecode),
            config_sha256: settings.sha256(),
            origin: None,
            cache_hit: true,
        },
        snapshot: fixture::snapshot(id, api::JobState::Completed, Some(fixture::version(1)))
            .result
            .unwrap(),
    }
}

fn launch(workspace: &mut Workspace, opened: &DecompileOpened) -> u64 {
    workspace.open_route(&Route::canonical(&opened.source));
    let Command::Open {
        generation,
        request,
    } = workspace.pending.take().unwrap()
    else {
        panic!("source link must open")
    };
    assert!(matches!(
        request.source,
        api::DecompileSourceInput::Sha256(_)
    ));
    assert!(
        request.settings.is_none(),
        "a saved config must not be overridden by defaults"
    );
    generation
}

fn accept(workspace: &mut Workspace, opened: DecompileOpened, artifact: Artifact, generation: u64) {
    let id = opened.snapshot.id;
    workspace.receive(Message::Opened {
        generation,
        result: Box::new(Ok(api::DecompileOpenReply { result: Ok(opened) })),
    });
    assert!(
        matches!(workspace.task.next(0.0), Some(Command::Result { id: found, .. }) if found==id)
    );
    workspace.receive(Message::Result {
        generation,
        id,
        result: Box::new(Ok(api::DecompileJobResultReply {
            result: Ok(api::DecompileJobResult {
                id,
                publication: fixture::version(1),
                artifact,
            }),
        })),
    });
}

#[test]
fn saved_settings_restore_and_late_open_cannot_replace_retained_result() {
    let mut artifact = fixture::artifact(Strategy::CodeDuplication);
    artifact.config.limits.max_work = 987654;
    let b = opened(&artifact, 5);
    let mut workspace = Workspace::default();
    let generation = launch(&mut workspace, &b);
    accept(&mut workspace, b.clone(), artifact.clone(), generation);
    assert_eq!(workspace.take_location().unwrap().1, History::Replace);
    assert_eq!(workspace.form.bytecode, b.request.bytecode);
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
    let selection = workspace.state.selected.clone();
    let publication = workspace.state.publication;
    let route = workspace.views.route_label();

    let a_artifact = fixture::artifact(Strategy::Dispatcher);
    let a = opened(&a_artifact, 6);
    let old_generation = launch(&mut workspace, &a);
    workspace.open_route(&Route::canonical(&b.source));
    assert!(
        workspace.pending.is_none(),
        "the still-installed B publication is reused locally"
    );
    assert_eq!(workspace.state.selected, selection);
    assert_eq!(workspace.state.publication, publication);
    assert_eq!(workspace.views.route_label(), route);
    workspace.receive(Message::Opened {
        generation: old_generation,
        result: Box::new(Ok(api::DecompileOpenReply { result: Ok(a) })),
    });
    assert!(workspace.take_location().is_none());
    assert!(workspace.task.next(100.0).is_none());
    assert_eq!(workspace.artifact.as_ref(), Some(&artifact));
    assert!(
        workspace
            .accessible_status()
            .contains("Result cache: retained")
    );
}

#[test]
fn wrong_runtime_or_config_never_installs_even_with_the_right_job_identity() {
    for mismatch in [false, true] {
        let artifact = fixture::artifact(Strategy::Dispatcher);
        let source = opened(&artifact, 7);
        let mut workspace = Workspace::default();
        let generation = launch(&mut workspace, &source);
        let mut wrong = artifact;
        if mismatch {
            wrong.config.limits.max_work -= 1;
        } else {
            wrong.input.runtime_bytecode[0] ^= 1;
        }
        accept(&mut workspace, source, wrong, generation);
        assert!(workspace.artifact.is_none());
        assert!(matches!(
            workspace.task.error,
            Some(super::task::Error::InputMismatch | super::task::Error::ConfigMismatch)
        ));
    }
}

#[test]
fn cancelled_opening_is_cleaned_up_after_a_different_link_becomes_active() {
    let artifact = fixture::artifact(Strategy::Dispatcher);
    let mut source = opened(&artifact, 8);
    source.snapshot.state = api::JobState::Queued;
    source.snapshot.publication = None;
    source.source.cache_hit = false;
    let mut workspace = Workspace::default();
    let generation = launch(&mut workspace, &source);
    workspace.task.request_cancel();
    workspace.open_route(&Route::page(Page::Decompiler));
    workspace.receive(Message::Opened {
        generation,
        result: Box::new(Ok(api::DecompileOpenReply { result: Ok(source) })),
    });
    assert!(matches!(
        workspace.take_cleanup(),
        Some(Command::Cancel {
            id: api::JobId(8),
            ..
        })
    ));
    assert!(workspace.take_cleanup().is_none());
    assert!(workspace.take_location().is_none());
    assert!(workspace.artifact.is_none());
}

#[test]
fn address_catalog_is_decompiler_owned_and_requested_only_for_address_editing() {
    let mut workspace = Workspace::default();
    assert!(workspace.provider_command().is_none());
    let source = api::DecompileAddressInput {
        address: format!("0x{}", "12".repeat(20)),
        provider_id: None,
        block: api::BlockSelector::Latest,
        chain_id: Some("1".into()),
    };
    workspace.form.load_address(&source);
    let Command::Providers { generation } = workspace.provider_command().unwrap() else {
        unreachable!()
    };
    assert!(workspace.provider_command().is_none());
    workspace.receive(Message::Providers {
        generation,
        result: Ok(api::RpcProvidersReply {
            result: Ok(vec![api::RpcProvider {
                id: "mainnet".into(),
                name: "Mainnet".into(),
                endpoint: "http://localhost/".into(),
            }]),
        }),
    });
    assert!(workspace.form.validation().is_none());
    workspace.form.load_address(&source);
    assert!(
        workspace.form.validation().is_none(),
        "reopening an implicit provider keeps the available default selection"
    );
    workspace.form.load("00".into(), api::Fork::Osaka);
    assert!(workspace.provider_command().is_none());
    let route = Route {
        page: Page::Decompiler,
        source: Some(Source::Address(source)),
        error: None,
    };
    workspace.open_route(&route);
    assert!(!workspace.form.open);
    assert!(
        workspace.provider_command().is_none(),
        "URL resolution uses its typed source request directly"
    );
}

#[test]
fn a_manual_submit_pushes_its_result_link_and_preserves_the_editor_spelling() {
    use egui::{Context, Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
    let context = Context::default();
    crate::palette::configure(&context);
    let mut workspace = Workspace::default();
    let draft = "0X60 01\n55 00";
    workspace.form.load(draft.into(), api::Fork::Osaka);
    let mut frame = |events| {
        let mut command = None;
        let mut output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 1000.0))),
                events,
                ..Default::default()
            },
            |ui| command = workspace.show(ui),
        );
        output.textures_delta.clear();
        command
    };
    assert!(frame(vec![]).is_none());
    assert!(frame(vec![]).is_none());
    let Some(Command::Submit {
        generation,
        request,
    }) = frame(vec![Event::Key {
        key: Key::Enter,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL,
    }])
    else {
        panic!("the real form must submit")
    };
    assert_eq!(request.bytecode, "0x60015500");
    assert!(
        workspace.take_location().is_none(),
        "a submitted but unaccepted request does not change the URL"
    );
    let artifact = fixture::artifact(Strategy::Dispatcher);
    let mut response = opened(&artifact, 10);
    response.request.bytecode = "0x60015500".into();
    workspace.receive(Message::Opened {
        generation,
        result: Box::new(Ok(api::DecompileOpenReply {
            result: Ok(response),
        })),
    });
    assert_eq!(workspace.take_location().unwrap().1, History::Push);
    assert_eq!(workspace.form.bytecode, draft);
}

#[test]
fn an_invalid_link_after_address_resolution_does_not_start_provider_traffic() {
    let mut workspace = Workspace::default();
    let address = Route::parse("/decompiler", &format!("?address=0x{}", "12".repeat(20)));
    workspace.open_route(&address);
    assert!(workspace.provider_command().is_none());
    workspace.open_route(&Route::parse("/decompiler", "?sha256=invalid"));
    assert!(workspace.pending.is_none());
    assert!(workspace.provider_command().is_none());
    assert!(
        workspace
            .accessible_status()
            .contains("Invalid decompiler link")
    );
}

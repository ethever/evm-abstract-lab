use super::*;
use egui::{Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol::RpcProvider;

fn frame(ctx: &Context, form: &mut AnalysisForm, events: Vec<Event>) -> Option<AnalyzeRequest> {
    paint(ctx, form, events).0
}

fn paint(
    ctx: &Context,
    form: &mut AnalysisForm,
    events: Vec<Event>,
) -> (Option<AnalyzeRequest>, FullOutput) {
    let mut request = None;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0))),
            events,
            ..RawInput::default()
        },
        |ui| request = form.show(ui.ctx(), false),
    );
    output.textures_delta.clear();
    (request, output)
}

fn text_center(output: &FullOutput, label: &str) -> Pos2 {
    fn find(shape: &egui::Shape, label: &str) -> Option<Pos2> {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(text.pos + text.galley.size() * 0.5)
            }
            egui::Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
            _ => None,
        }
    }
    output
        .shapes
        .iter()
        .find_map(|shape| find(&shape.shape, label))
        .unwrap_or_else(|| panic!("missing text: {label}"))
}

fn click(ctx: &Context, form: &mut AnalysisForm, pos: Pos2) -> Option<AnalyzeRequest> {
    let mut request = None;
    for pressed in [true, false] {
        request = frame(
            ctx,
            form,
            vec![
                Event::PointerMoved(pos),
                Event::PointerButton {
                    pos,
                    button: PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::NONE,
                },
            ],
        )
        .or(request);
    }
    request
}

fn providers() -> Vec<RpcProvider> {
    vec![
        RpcProvider {
            id: "primary-id".into(),
            name: "Primary network".into(),
            endpoint: "http://127.0.0.1:8545".into(),
        },
        RpcProvider {
            id: "archive-id".into(),
            name: "Archive network".into(),
            endpoint: "https://example.com/archive".into(),
        },
    ]
}

fn load_providers(form: &mut AnalysisForm) {
    let Some(Command::RpcProviders { generation }) = form.provider_command() else {
        panic!("expected catalog request")
    };
    form.receive_providers(
        generation,
        Ok(RpcProvidersReply {
            result: Ok(providers()),
        }),
    );
}
fn enter() -> Event {
    Event::Key {
        key: Key::Enter,
        physical_key: Some(Key::Enter),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::CTRL,
    }
}

#[test]
fn opening_focuses_the_real_bytecode_editor_and_submit_preserves_typed_request() {
    let ctx = Context::default();
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    form.bytecode.bytecode.clear();
    frame(&ctx, &mut form, vec![]);
    frame(&ctx, &mut form, vec![]);
    assert!(ctx.text_edit_focused());
    frame(&ctx, &mut form, vec![Event::Text("6001fF".into())]);
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    assert_eq!(
        request.input,
        AnalysisInput::Bytecode(BytecodeInput {
            bytecode: "6001fF".into(),
            address: None
        })
    );
    assert_eq!(request.environment, EnvironmentInput::default());
    assert_eq!(request.limits, AnalysisLimits::default());
    assert!(!form.open);
}

#[test]
fn rpc_form_keeps_snapshot_identity_and_execution_overrides_distinct() {
    let ctx = Context::default();
    let mut form = AnalysisForm {
        open: true,
        source: Source::Rpc,
        ..AnalysisForm::default()
    };
    load_providers(&mut form);
    form.rpc.block = BlockSelector::Hash(format!("0x{}", "12".repeat(32)));
    form.environment.chain_id = Some("0x2".into());
    form.environment.calldata = evm_abstract_protocol::CalldataInput::Exact(String::new());
    frame(&ctx, &mut form, vec![]);
    frame(&ctx, &mut form, vec![]);
    frame(
        &ctx,
        &mut form,
        vec![Event::Text(
            "0x1111111111111111111111111111111111111111".into(),
        )],
    );
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    let AnalysisInput::Rpc(rpc) = request.input else {
        panic!("expected typed RPC request")
    };
    assert_eq!(rpc.provider_id, "primary-id");
    assert_eq!(rpc.address, "0x1111111111111111111111111111111111111111");
    assert_eq!(rpc.block, form.rpc.block);
    assert_eq!(request.environment.chain_id, Some("0x2".into()));
    assert_eq!(
        request.environment.number, None,
        "unchecked header fields inherit the pinned snapshot"
    );
    assert_eq!(
        request.environment.calldata,
        evm_abstract_protocol::CalldataInput::Exact(String::new())
    );
}

#[test]
fn rpc_selector_displays_names_and_urls_and_submits_the_clicked_providers_id() {
    let ctx = Context::default();
    let mut form = AnalysisForm {
        open: true,
        source: Source::Rpc,
        ..AnalysisForm::default()
    };
    load_providers(&mut form);
    frame(&ctx, &mut form, vec![]);
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(
        &ctx,
        &mut form,
        text_center(&output, "Primary network (http://127.0.0.1:8545)"),
    );
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(
        &ctx,
        &mut form,
        text_center(&output, "Archive network (https://example.com/archive)"),
    );
    assert_eq!(
        form.provider_status(),
        "RPC provider: Archive network (https://example.com/archive)"
    );
    let (_, output) = paint(&ctx, &mut form, vec![]);
    let request = click(&ctx, &mut form, text_center(&output, "Analyze")).unwrap();
    let AnalysisInput::Rpc(rpc) = request.input else {
        panic!("expected RPC selection")
    };
    assert_eq!(rpc.provider_id, "archive-id");
}

#[test]
fn missing_catalog_or_invalid_selection_blocks_button_and_keyboard_but_not_bytecode() {
    for state in [
        "unrequested",
        "loading",
        "empty",
        "failed",
        "invalid-selection",
    ] {
        let ctx = Context::default();
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..AnalysisForm::default()
        };
        if state != "unrequested" {
            form.provider_command();
        }
        match state {
            "empty" => form.receive_providers(1, Ok(RpcProvidersReply { result: Ok(vec![]) })),
            "failed" => form.receive_providers(1, Err(TransportError::Timeout { seconds: 15 })),
            "invalid-selection" => {
                form.receive_providers(
                    1,
                    Ok(RpcProvidersReply {
                        result: Ok(providers()),
                    }),
                );
                form.rpc.provider_id = "not-in-catalog".into();
            }
            _ => {}
        }
        frame(&ctx, &mut form, vec![]);
        let (_, output) = paint(&ctx, &mut form, vec![]);
        assert!(
            click(&ctx, &mut form, text_center(&output, "Analyze")).is_none(),
            "{state}"
        );
        assert!(frame(&ctx, &mut form, vec![enter()]).is_none(), "{state}");
        assert!(form.open, "{state}");
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, "Bytecode"));
        let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
        assert!(
            matches!(request.input, AnalysisInput::Bytecode(_)),
            "{state}"
        );
    }
}

#[test]
fn retry_after_empty_or_failed_catalog_recovers_and_ignores_old_callbacks() {
    for failed in [false, true] {
        let ctx = Context::default();
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..AnalysisForm::default()
        };
        form.provider_command();
        form.receive_providers(
            1,
            if failed {
                Err(TransportError::Timeout { seconds: 15 })
            } else {
                Ok(RpcProvidersReply { result: Ok(vec![]) })
            },
        );
        frame(&ctx, &mut form, vec![]);
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, "Retry"));
        assert!(matches!(
            form.provider_command(),
            Some(Command::RpcProviders { generation: 2 })
        ));
        assert!(
            form.provider_command().is_none(),
            "only one fetch while loading"
        );
        form.receive_providers(
            1,
            Ok(RpcProvidersReply {
                result: Ok(providers()),
            }),
        );
        assert!(!form.can_submit(false), "old callback cannot end the retry");
        form.receive_providers(
            2,
            Ok(RpcProvidersReply {
                result: Ok(providers()),
            }),
        );
        assert_eq!(
            form.provider_status(),
            "RPC provider: Primary network (http://127.0.0.1:8545)"
        );
        assert!(frame(&ctx, &mut form, vec![enter()]).is_some());
    }
}

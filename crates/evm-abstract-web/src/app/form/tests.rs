use super::*;
use egui::{Context, Event, FullOutput, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
use evm_abstract_protocol as api;
use evm_abstract_protocol::RpcProvider;

fn frame(ctx: &Context, form: &mut AnalysisForm, events: Vec<Event>) -> Option<AnalyzeRequest> {
    paint(ctx, form, events).0
}

fn paint(
    ctx: &Context,
    form: &mut AnalysisForm,
    events: Vec<Event>,
) -> (Option<AnalyzeRequest>, FullOutput) {
    paint_sized(ctx, form, events, Vec2::new(900.0, 700.0))
}

fn paint_sized(
    ctx: &Context,
    form: &mut AnalysisForm,
    events: Vec<Event>,
    size: Vec2,
) -> (Option<AnalyzeRequest>, FullOutput) {
    let mut request = None;
    let mut output = ctx.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
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

#[test]
fn selected_fork_controls_the_submitted_execution_rules() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    let size = Vec2::new(1440.0, 1000.0);
    for _ in 0..3 {
        paint_sized(&ctx, &mut form, vec![], size);
    }
    for fork in [Fork::Cancun, Fork::Osaka] {
        let output = paint_sized(&ctx, &mut form, vec![], size).1;
        let selector = text_center(&output, &format!("{:?}", form.fork));
        click_sized(&ctx, &mut form, selector, size);
        let menu = paint_sized(&ctx, &mut form, vec![], size).1;
        let target = text_center(&menu, &format!("{fork:?}"));
        click_sized(&ctx, &mut form, target, size);
        assert_eq!(form.fork, fork);
        assert_eq!(form.request().fork, fork);
    }
}

#[test]
fn precision_selector_submits_each_supported_numeric_model() {
    use evm_abstract_protocol::NumericModel;

    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    let size = Vec2::new(1440.0, 1400.0);

    for _ in 0..3 {
        paint_sized(&ctx, &mut form, vec![], size);
    }
    let output = paint_sized(&ctx, &mut form, vec![], size).1;
    click_sized(&ctx, &mut form, text_center(&output, "Precision"), size);
    for (model, current, choice) in [
        (NumericModel::FiniteSet, "Product", "Finite set"),
        (NumericModel::Product, "Finite set", "Product"),
    ] {
        let output = paint_sized(&ctx, &mut form, vec![], size).1;
        text_center(&output, "Numeric model");
        click_sized(&ctx, &mut form, text_center(&output, current), size);
        let output = paint_sized(&ctx, &mut form, vec![], size).1;
        click_sized(&ctx, &mut form, text_center(&output, choice), size);
        assert_eq!(
            form.request().limits,
            AnalysisLimits {
                numeric_model: model,
                ..AnalysisLimits::default()
            }
        );
        let output = paint_sized(&ctx, &mut form, vec![], size).1;
        text_center(&output, choice);
    }
}

fn click(ctx: &Context, form: &mut AnalysisForm, pos: Pos2) -> Option<AnalyzeRequest> {
    click_sized(ctx, form, pos, Vec2::new(900.0, 700.0))
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

fn replace_text(ctx: &Context, form: &mut AnalysisForm, text: &str) {
    frame(
        ctx,
        form,
        vec![
            Event::Key {
                key: Key::A,
                physical_key: Some(Key::A),
                pressed: true,
                repeat: false,
                modifiers: Modifiers {
                    ctrl: true,
                    command: true,
                    ..Modifiers::NONE
                },
            },
            Event::Text(text.into()),
        ],
    );
}

#[test]
fn integer_budget_editor_preserves_values_above_f64_precision_and_old_caps() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    frame(&ctx, &mut form, vec![]);
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(&ctx, &mut form, text_center(&output, "Execution budget"));
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(
        &ctx,
        &mut form,
        text_center(&output, &AnalysisLimits::default().max_work.to_string()),
    );
    replace_text(&ctx, &mut form, "9007199254740993");
    let (_, output) = paint(&ctx, &mut form, vec![]);
    text_center(&output, "9007199254740993");
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    assert_eq!(request.limits.max_work, 9_007_199_254_740_993);
    assert_eq!(request.limits.max_states, 100_000);
    assert_eq!(request.limits.max_memory_bytes, 64 * 1024 * 1024);
    let json = serde_json::to_string(&request).unwrap();
    assert!(json.contains("\"max_work\":9007199254740993"));
}

#[test]
fn root_terminal_checkbox_submits_the_default_and_explicit_opt_out_for_both_sources() {
    for source in [Source::Bytecode, Source::Rpc] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            source,
            ..AnalysisForm::default()
        };
        assert_eq!(form.request().limits, AnalysisLimits::default());
        assert!(form.request().limits.root_terminal_coverage);
        assert_eq!(form.request().limits.max_work, 2_000_000_000);
        if source == Source::Rpc {
            load_providers(&mut form);
        } else {
            form.bytecode.bytecode = "6003565b602a5f5260205ff3".into();
            form.limits.max_transfers = 1;
        }
        let mut expected = form.request();
        let size = Vec2::new(1440.0, 1000.0);
        let output = settle_sized(&ctx, &mut form, size);
        click_sized(
            &ctx,
            &mut form,
            text_center(&output, "Execution budget"),
            size,
        );
        for enabled in [true, false, true] {
            let output = settle_sized(&ctx, &mut form, size);
            let position = visible_label(
                &output,
                "Prepare conservative root terminal coverage",
                Rect::from_min_size(Pos2::ZERO, size),
            );
            if enabled != expected.limits.root_terminal_coverage {
                assert!(click_sized(&ctx, &mut form, position, size).is_none());
            }
            expected.limits.root_terminal_coverage = enabled;
            assert_eq!(form.request(), expected);
            let (submitted, _) = paint_sized(&ctx, &mut form, vec![enter()], size);
            let submitted = submitted.unwrap();
            assert_eq!(submitted, expected);
            let wire = serde_json::to_string(&submitted).unwrap();
            assert!(wire.contains(&format!("\"root_terminal_coverage\":{enabled}")));
            assert_eq!(
                serde_json::from_str::<AnalyzeRequest>(&wire).unwrap(),
                expected
            );
            #[cfg(not(target_arch = "wasm32"))]
            if source == Source::Bytecode {
                let report = evm_abstract_server::analyze::analyze(submitted).unwrap();
                report.validate().unwrap();
                assert_eq!(report.metadata.limits, expected.limits);
                let hybrid = report.hybrid.as_ref().unwrap();
                assert!(!hybrid.coverage.baseline_config.root_terminal_coverage);
                let native = report.graph(api::AnalysisNamespace::Native).unwrap();
                if enabled {
                    let config = &native.root_terminal_coverage.as_ref().unwrap().input.config;
                    assert!(config.root_terminal_coverage);
                    assert_eq!(config.max_transfers, expected.limits.max_transfers);
                    assert_eq!(config.max_work, expected.limits.max_work);
                } else {
                    assert!(native.root_terminal_coverage.is_none());
                }
            }
            paint_sized(&ctx, &mut form, vec![], size);
            form.open = true;
        }
    }
}

#[test]
fn root_terminal_checkbox_preserves_invalid_numeric_drafts_and_request_values() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        ..AnalysisForm::default()
    };
    let output = settle_sized(&ctx, &mut form, Vec2::new(900.0, 700.0));
    click(&ctx, &mut form, text_center(&output, "Execution budget"));
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(
        &ctx,
        &mut form,
        text_center(&output, &AnalysisLimits::default().max_work.to_string()),
    );
    replace_text(&ctx, &mut form, "invalid");
    for enabled in [false, true] {
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(
            &ctx,
            &mut form,
            text_center(&output, "Prepare conservative root terminal coverage"),
        );
        assert_eq!(form.request().limits.root_terminal_coverage, enabled);
        assert_eq!(
            form.request().limits.max_work,
            AnalysisLimits::default().max_work
        );
        assert!(!form.can_submit(false));
        assert!(frame(&ctx, &mut form, vec![enter()]).is_none());
        let (_, output) = paint(&ctx, &mut form, vec![]);
        assert!(click(&ctx, &mut form, text_center(&output, "Analyze")).is_none());
    }
    let (_, output) = paint(&ctx, &mut form, vec![]);
    click(&ctx, &mut form, text_center(&output, "invalid"));
    replace_text(&ctx, &mut form, "18446744073709551615");
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    assert!(request.limits.root_terminal_coverage);
    assert_eq!(request.limits.max_work, u64::MAX);
}

#[test]
fn invalid_numeric_draft_blocks_button_and_keyboard_until_corrected() {
    for invalid in ["not a number", "-1", "18446744073709551616"] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            ..AnalysisForm::default()
        };
        frame(&ctx, &mut form, vec![]);
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, "Execution budget"));
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(
            &ctx,
            &mut form,
            text_center(&output, &AnalysisLimits::default().max_work.to_string()),
        );
        replace_text(&ctx, &mut form, invalid);
        assert!(frame(&ctx, &mut form, vec![enter()]).is_none(), "{invalid}");
        let (_, output) = paint(&ctx, &mut form, vec![]);
        assert!(
            click(&ctx, &mut form, text_center(&output, "Analyze")).is_none(),
            "{invalid}"
        );
        assert!(form.open);
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, invalid));
        replace_text(&ctx, &mut form, "18446744073709551615");
        let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
        assert_eq!(request.limits.max_work, u64::MAX);
    }
}

#[test]
fn precision_and_acquisition_inputs_accept_values_above_previous_admission_caps() {
    for (header, initial, value) in [
        ("Precision", "512", "10000"),
        ("RPC acquisition budget", "67108864", "9007199254740993"),
    ] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            ..AnalysisForm::default()
        };
        frame(&ctx, &mut form, vec![]);
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, header));
        let (_, output) = paint(&ctx, &mut form, vec![]);
        click(&ctx, &mut form, text_center(&output, initial));
        replace_text(&ctx, &mut form, value);
        let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
        if header == "Precision" {
            assert_eq!(request.limits.max_constants, 10_000);
        } else {
            assert_eq!(request.limits.rpc_max_response_bytes, 9_007_199_254_740_993);
        }
    }
}

#[test]
fn opening_focuses_the_real_bytecode_editor_and_submit_preserves_typed_request() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
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
    crate::notation::initialize_fonts(&ctx);
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
    replace_text(
        &ctx,
        &mut form,
        "0x1111111111111111111111111111111111111111",
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
    crate::notation::initialize_fonts(&ctx);
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
        crate::notation::initialize_fonts(&ctx);
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..AnalysisForm::default()
        };
        if state != "unrequested" {
            form.provider_command();
        }
        match state {
            "empty" => {
                form.receive_providers(1, Ok(RpcProvidersReply { result: Ok(vec![]) }));
            }
            "failed" => {
                form.receive_providers(1, Err(TransportError::Timeout { seconds: 15 }));
            }
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
        crate::notation::initialize_fonts(&ctx);
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

const USDC: &str = "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48";
const MAINNET_EXAMPLES: [(&str, &str); 8] = [
    ("USDC", USDC),
    ("WETH", "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
    ("USDT", "0xdAC17F958D2ee523a2206206994597C13D831ec7"),
    ("Multicall3", "0xcA11bde05977b3631167028862bE2a173976CA11"),
    (
        "Uniswap V2 Router02",
        "0x7a250d5630B4cF539739dF2C5dAcb4c659F2488D",
    ),
    (
        "Uniswap V2 Pair (USDC/WETH)",
        "0xB4e16d0168e52d35CaCD2c6185b44281Ec28C9Dc",
    ),
    (
        "Uniswap V2 Factory",
        "0x5C69bEe701ef814a2B6a3EDD4B1652CB9cc5aA6f",
    ),
    (
        "Uniswap V4 PoolManager",
        "0x000000000004444c5dc75cB358380D2e3dE08A90",
    ),
];

fn settle_sized(ctx: &Context, form: &mut AnalysisForm, size: Vec2) -> FullOutput {
    paint_sized(ctx, form, vec![], size);
    paint_sized(ctx, form, vec![], size);
    paint_sized(ctx, form, vec![], size).1
}

fn click_sized(
    ctx: &Context,
    form: &mut AnalysisForm,
    pos: Pos2,
    size: Vec2,
) -> Option<AnalyzeRequest> {
    let mut request = None;
    for pressed in [true, false] {
        request = paint_sized(
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
            size,
        )
        .0
        .or(request);
    }
    request
}

fn visible_label(output: &FullOutput, label: &str, viewport: Rect) -> Pos2 {
    let center = text_center(output, label);
    assert!(
        viewport.contains(center),
        "{label} outside viewport: {center:?}"
    );
    fn visible(shape: &egui::Shape, label: &str, clip: Rect) -> bool {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                let rect = Rect::from_min_size(text.pos, text.galley.size());
                let shown = rect.intersect(clip);
                !text.galley.elided
                    && shown.width() >= rect.width() - 1.0
                    && shown.height() >= rect.height() - 1.0
            }
            egui::Shape::Vec(shapes) => shapes.iter().any(|shape| visible(shape, label, clip)),
            _ => false,
        }
    }
    assert!(
        output.shapes.iter().any(|shape| visible(
            &shape.shape,
            label,
            shape.clip_rect.intersect(viewport)
        )),
        "{label} is allocated but clipped or truncated"
    );
    center
}

#[test]
fn ethereum_entry_examples_submit_the_chosen_address_and_preserve_every_other_input() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        source: Source::Rpc,
        ..AnalysisForm::default()
    };
    load_providers(&mut form);
    form.rpc.provider_id = "archive-id".into();
    form.rpc.block = BlockSelector::Number(19_000_000);
    form.rpc.accounts = vec![evm_abstract_protocol::AccountQuery {
        address: "0x1111111111111111111111111111111111111111".into(),
        slots: vec!["0x2a".into()],
    }];
    form.environment.call_value = evm_abstract_protocol::WordInput::Concrete("0x2a".into());
    form.environment.chain_id = Some("0x1".into());
    form.limits.max_work = 9_007_199_254_740_993;
    form.limits.max_states = 12_345;
    form.limits.max_constants = 42;
    form.limits.rpc_max_response_bytes = 8 * 1024 * 1024;
    form.fork = Fork::Prague;
    let mut expected = form.request();
    let AnalysisInput::Rpc(input) = &expected.input else {
        panic!("expected RPC form")
    };
    assert_eq!(
        input.address, USDC,
        "default entry must be the reviewed mainnet USDC address"
    );
    let size = Vec2::new(900.0, 700.0);
    let mut current = "USDC";
    for (next, address) in MAINNET_EXAMPLES
        .into_iter()
        .skip(1)
        .chain([MAINNET_EXAMPLES[0]])
    {
        form.open = true;
        let output = settle_sized(&ctx, &mut form, size);
        assert!(click(&ctx, &mut form, text_center(&output, current)).is_none());
        let output = settle_sized(&ctx, &mut form, size);
        assert!(
            click(&ctx, &mut form, text_center(&output, next)).is_none(),
            "choosing an entry must not submit analysis"
        );
        let AnalysisInput::Rpc(input) = &mut expected.input else {
            unreachable!()
        };
        input.address = address.into();
        assert_eq!(
            form.request(),
            expected,
            "entry selection changed unrelated request fields"
        );
        let output = settle_sized(&ctx, &mut form, size);
        text_center(&output, next);
        assert!(form.open, "analysis was submitted before clicking Analyze");
        let submitted = click(&ctx, &mut form, text_center(&output, "Analyze")).unwrap();
        assert_eq!(submitted, expected);
        current = next;
        frame(&ctx, &mut form, vec![]);
    }
}

#[test]
fn custom_entry_remains_editable_visible_and_persistent_in_narrow_windows() {
    for size in [Vec2::new(390.0, 844.0), Vec2::new(320.0, 480.0)] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..AnalysisForm::default()
        };
        load_providers(&mut form);
        let viewport = Rect::from_min_size(Pos2::ZERO, size);
        let mut current = "USDC";
        for (label, address) in MAINNET_EXAMPLES.into_iter().skip(1) {
            let output = settle_sized(&ctx, &mut form, size);
            let selector = visible_label(&output, current, viewport);
            assert!(click_sized(&ctx, &mut form, selector, size).is_none());
            let output = settle_sized(&ctx, &mut form, size);
            let choice = visible_label(&output, label, viewport);
            visible_label(&output, "Custom", viewport);
            assert!(click_sized(&ctx, &mut form, choice, size).is_none());
            let output = settle_sized(&ctx, &mut form, size);
            visible_label(&output, label, viewport);
            assert_eq!(form.rpc.address, address);
            current = label;
        }
        let output = settle_sized(&ctx, &mut form, size);
        let selector = visible_label(&output, current, viewport);
        assert!(click_sized(&ctx, &mut form, selector, size).is_none());
        let output = settle_sized(&ctx, &mut form, size);
        let custom = visible_label(&output, "Custom", viewport);
        assert!(click_sized(&ctx, &mut form, custom, size).is_none());
        assert!(form.rpc.address.is_empty());
        settle_sized(&ctx, &mut form, size);
        let editor = ctx.read_response(Id::new("rpc_address")).unwrap();
        assert!(editor.interact_rect.height() >= 16.0);
        assert!(viewport.contains_rect(editor.interact_rect));
        click_sized(&ctx, &mut form, editor.interact_rect.center(), size);
        let manual = "0x1234567890123456789012345678901234567890";
        paint_sized(&ctx, &mut form, vec![Event::Text(manual.into())], size);
        assert_eq!(form.rpc.address, manual);
        let output = settle_sized(&ctx, &mut form, size);
        visible_label(&output, "Custom", viewport);
        click_sized(&ctx, &mut form, text_center(&output, "Close"), size);
        assert!(!form.open);
        paint_sized(&ctx, &mut form, vec![], size);
        form.open = true;
        let output = settle_sized(&ctx, &mut form, size);
        visible_label(&output, "Custom", viewport);
        assert_eq!(
            form.rpc.address, manual,
            "opening the form overwrote a manual entry"
        );
        let request = paint_sized(&ctx, &mut form, vec![enter()], size).0.unwrap();
        let AnalysisInput::Rpc(input) = request.input else {
            panic!("expected custom RPC input")
        };
        assert_eq!(input.address, manual);
        assert_eq!(input.provider_id, "primary-id");
    }
}

#[test]
fn typing_a_known_address_changes_the_label_without_rewriting_the_draft() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut form = AnalysisForm {
        open: true,
        source: Source::Rpc,
        ..AnalysisForm::default()
    };
    load_providers(&mut form);
    let size = Vec2::new(900.0, 700.0);
    settle_sized(&ctx, &mut form, size);
    for (label, address) in MAINNET_EXAMPLES {
        for draft in [address.to_ascii_lowercase(), address.to_ascii_uppercase()] {
            let editor = ctx.read_response(Id::new("rpc_address")).unwrap();
            assert!(click(&ctx, &mut form, editor.interact_rect.center()).is_none());
            replace_text(&ctx, &mut form, &draft);
            let output = settle_sized(&ctx, &mut form, size);
            visible_label(&output, label, Rect::from_min_size(Pos2::ZERO, size));
            assert_eq!(
                form.rpc.address, draft,
                "recognizing {label} must not normalize user text in place"
            );
            assert!(form.open, "typing an example must not submit analysis");
        }
    }
    let draft = form.rpc.address.clone();
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    let AnalysisInput::Rpc(input) = request.input else {
        panic!("expected RPC input")
    };
    assert_eq!(input.address, draft);
}

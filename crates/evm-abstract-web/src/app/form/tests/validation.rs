use super::*;

#[test]
fn invalid_active_source_cannot_submit_but_inactive_drafts_are_retained() {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    let mut form = AnalysisForm {
        open: true,
        ..Default::default()
    };
    form.bytecode.bytecode = "0xz1".into();
    load_providers(&mut form);
    assert!(frame(&ctx, &mut form, vec![enter()]).is_none());
    assert!(form.open);
    form.source = Source::Rpc;
    assert!(
        form.can_submit(false),
        "invalid inactive bytecode cannot block an address"
    );
    let request = frame(&ctx, &mut form, vec![enter()]).unwrap();
    assert!(matches!(request.input, AnalysisInput::Rpc(_)));
    assert_eq!(form.bytecode.bytecode, "0xz1");
    form.source = Source::Bytecode;
    form.bytecode.bytecode = "0x60 01\n60 02 01 00".into();
    form.rpc.address = "incomplete address".into();
    assert!(form.can_submit(false));
    assert_eq!(form.request().environment, EnvironmentInput::default());
}

#[test]
fn unknown_zero_and_exact_empty_are_distinct_and_invalid_exact_words_stay_in_the_editor() {
    let mut form = AnalysisForm::default();
    assert!(form.can_submit(false));
    assert_eq!(form.environment.call_value, api::WordInput::Unknown);
    assert_eq!(form.environment.calldata, api::CalldataInput::Unknown);
    form.environment.call_value = api::WordInput::Concrete(String::new());
    assert!(
        !form.can_submit(false),
        "blank exact value must not silently become zero"
    );
    form.environment.call_value = api::WordInput::Concrete("0".into());
    form.environment.calldata = api::CalldataInput::Exact(String::new());
    assert!(form.can_submit(false));
    let report = evm_abstract_server::analyze::analyze(form.request()).unwrap();
    report.validate().unwrap();
    assert_eq!(
        form.request().environment.call_value,
        api::WordInput::Concrete("0".into())
    );
    assert_eq!(
        form.request().environment.calldata,
        api::CalldataInput::Exact(String::new())
    );
    for value in ["-1", "1.5", "0xgg", &format!("0x1{}", "0".repeat(64))] {
        form.environment.call_value = api::WordInput::Concrete(value.into());
        assert!(!form.can_submit(false), "{value}");
    }
    for value in [
        "42",
        "0X2a",
        "0b101010",
        "0o52",
        "4_2",
        &format!("0x{}", "f".repeat(64)),
    ] {
        form.environment.call_value = api::WordInput::Concrete(value.into());
        assert!(form.can_submit(false), "{value}");
        evm_abstract_server::analyze::analyze(form.request()).unwrap();
    }
}

#[test]
fn block_number_editor_and_request_keep_every_u64_digit() {
    let size = Vec2::new(1440.0, 1000.0);
    for value in [
        "9007199254740993",
        "18446744073709551615",
        "18446744073709551616",
        "1.5",
        "0",
    ] {
        let ctx = Context::default();
        crate::notation::initialize_fonts(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..Default::default()
        };
        load_providers(&mut form);
        form.rpc.block = BlockSelector::Number(19_000_000);
        let output = settle_sized(&ctx, &mut form, size);
        click_sized(&ctx, &mut form, text_center(&output, "19000000"), size);
        replace_text(&ctx, &mut form, value);
        if let Ok(parsed) = value.parse::<u64>() {
            assert_eq!(form.rpc.block, BlockSelector::Number(parsed));
            assert!(form.can_submit(false));
            let request = form.request();
            let wire = serde_json::to_string(&request).unwrap();
            assert!(wire.contains(&format!("\"Number\":{parsed}")));
            assert_eq!(
                serde_json::from_str::<AnalyzeRequest>(&wire).unwrap(),
                request
            );
        } else {
            assert!(!form.can_submit(false));
        }
    }
}

#[test]
fn malformed_pinned_hash_addresses_and_duplicate_hash_observations_cannot_submit() {
    let mut form = AnalysisForm {
        source: Source::Rpc,
        ..Default::default()
    };
    load_providers(&mut form);
    form.rpc.block = BlockSelector::Hash("0x1234".into());
    assert!(!form.can_submit(false));
    form.rpc.block = BlockSelector::Hash(format!("0x{}", "12".repeat(32)));
    assert!(form.can_submit(false));
    form.rpc.accounts.push(api::AccountQuery {
        address: "0x1234".into(),
        slots: vec![],
    });
    assert!(!form.can_submit(false));
    form.rpc.accounts.clear();
    form.environment.block_hashes = vec![
        api::IndexedHash {
            index: "42".into(),
            hash: format!("0x{}", "ab".repeat(32)),
        },
        api::IndexedHash {
            index: "0x2a".into(),
            hash: format!("0x{}", "ab".repeat(32)),
        },
    ];
    assert!(
        !form.can_submit(false),
        "different syntax still names one observed index"
    );
}

#[test]
fn address_modal_with_product_style_and_long_provider_stays_inside_narrow_viewports() {
    for size in [Vec2::new(320.0, 480.0), Vec2::new(390.0, 844.0)] {
        let ctx = Context::default();
        crate::palette::configure(&ctx);
        ctx.global_style_mut(|style| style.animation_time = 0.0);
        let mut form = AnalysisForm {
            open: true,
            source: Source::Rpc,
            ..Default::default()
        };
        let Some(Command::RpcProviders { generation }) = form.provider_command() else {
            unreachable!()
        };
        form.receive_providers(
            generation,
            Ok(RpcProvidersReply {
                result: Ok(vec![RpcProvider {
                    id: "primary".into(),
                    name: "Pending acquisition".into(),
                    endpoint:
                        "http://127.0.0.1:37647/rpc?token=rpc-value-shown-in-provider-selector"
                            .into(),
                }]),
            }),
        );
        let output = settle_sized(&ctx, &mut form, size);
        let viewport = Rect::from_min_size(Pos2::ZERO, size);
        let modal = ctx
            .memory(|memory| memory.area_rect(Id::new("analysis_form")))
            .unwrap();
        assert!(viewport.contains_rect(modal), "{size:?}: {modal:?}");
        for label in [
            "New analysis",
            "Address",
            "Bytecode",
            "Close",
            "USDC",
            "Analyze",
        ] {
            visible_label(&output, label, viewport);
        }
        let editor = ctx.read_response(Id::new("rpc_address")).unwrap();
        assert!(viewport.contains_rect(editor.interact_rect));
        assert!(form.can_submit(false));
    }
}

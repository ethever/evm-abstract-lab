use super::*;

fn edit_sources(
    source: Source,
    text: &str,
) -> (AnalysisForm, AnalyzeRequest, Option<AnalyzeRequest>) {
    let ctx = Context::default();
    crate::notation::initialize_fonts(&ctx);
    ctx.global_style_mut(|style| style.animation_time = 0.0);
    let mut form = AnalysisForm {
        open: true,
        source,
        ..Default::default()
    };
    form.bytecode.bytecode = "600160020100".into();
    if source == Source::Rpc {
        load_providers(&mut form);
    }
    let original = form.request();
    assert_eq!(original.limits.max_detail_regions, 1);
    let size = Vec2::new(1440.0, 1000.0);
    let output = settle_sized(&ctx, &mut form, size);
    click_sized(&ctx, &mut form, text_center(&output, "Precision"), size);
    let output = settle_sized(&ctx, &mut form, size);
    text_center(&output, "Detailed code sources");
    click_sized(&ctx, &mut form, text_center(&output, "1"), size);
    let mut events = vec![Event::Key {
        key: Key::A,
        physical_key: Some(Key::A),
        pressed: true,
        repeat: false,
        modifiers: Modifiers {
            ctrl: true,
            command: true,
            ..Modifiers::NONE
        },
    }];
    if text.is_empty() {
        events.push(Event::Key {
            key: Key::Backspace,
            physical_key: Some(Key::Backspace),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
    } else {
        events.push(Event::Text(text.into()));
    }
    paint_sized(&ctx, &mut form, events, size);
    let submitted = paint_sized(&ctx, &mut form, vec![enter()], size).0;
    (form, original, submitted)
}

#[test]
fn detail_source_editor_preserves_zero_default_and_large_integer_requests_for_both_sources() {
    for source in [Source::Bytecode, Source::Rpc] {
        for value in [0, 1, 2, 3, 9_007_199_254_740_993, u64::MAX] {
            let (form, mut expected, submitted) = edit_sources(source, &value.to_string());
            expected.limits.max_detail_regions = value;
            let submitted = submitted.expect("a supported integer can be submitted");
            assert_eq!(submitted, expected);
            assert!(!form.open);
            let wire = serde_json::to_string(&submitted).unwrap();
            assert!(wire.contains(&format!("\"max_detail_regions\":{value}")));
            assert_eq!(
                serde_json::from_str::<AnalyzeRequest>(&wire).unwrap(),
                submitted
            );
        }
    }
}

#[test]
fn invalid_detail_source_drafts_cannot_replace_or_submit_the_previous_value() {
    for text in ["-1", "1.5", "18446744073709551616", ""] {
        let (form, original, submitted) = edit_sources(Source::Bytecode, text);
        assert!(submitted.is_none(), "draft {text:?}");
        assert!(!form.can_submit(false));
        assert!(form.open);
        assert_eq!(form.request(), original);
    }
}

#[test]
fn actual_form_requests_apply_detail_policy_without_replacing_the_shared_budget() {
    for regions in [0, 1, 2, 3] {
        let (_, _, submitted) = edit_sources(Source::Bytecode, &regions.to_string());
        let request = submitted.unwrap();
        let report = evm_abstract_server::analyze::analyze(request.clone()).unwrap();
        report.validate().unwrap();
        let coverage = &report.hybrid.as_ref().unwrap().coverage;
        assert_eq!(coverage.requested_detail_regions, regions);
        assert!(coverage.detail_regions.len() as u64 <= regions);
        assert!(coverage.baseline_current);
        assert!(coverage.work.total <= request.limits.max_work);
        let minimum = report.minimum.as_ref().unwrap();
        assert_eq!(minimum.status, api::AnalysisStatus::Converged);
        assert!(minimum.current);
        if regions == 0 {
            assert!(report.native.is_none());
            assert_eq!(coverage.detail, api::HybridDetailState::Disabled);
            assert!(
                crate::app::hybrid::summary(&report)
                    .unwrap()
                    .contains("detailed paths disabled")
            );
            let mut workspace = crate::app::Workspace::default();
            workspace.initial_command();
            workspace.receive(Ok(api::AnalyzeReply {
                result: Ok(report.clone()),
            }));
            workspace.activate_view(crate::app::model::Activation::Namespace(
                api::AnalysisNamespace::Native,
            ));
            assert!(workspace.report_view.identity().is_minimum());
            assert_eq!(workspace.selection, crate::app::Selection::default());
            assert!(
                workspace
                    .accessible_status()
                    .contains("detailed paths disabled")
            );
            let mut incomplete = request.clone();
            incomplete.input = AnalysisInput::Bytecode(BytecodeInput {
                bytecode: "6003565b00".into(),
                address: None,
            });
            incomplete.limits.max_transfers = 1;
            let incomplete = evm_abstract_server::analyze::analyze(incomplete).unwrap();
            incomplete.validate().unwrap();
            assert!(
                !incomplete
                    .hybrid
                    .as_ref()
                    .unwrap()
                    .coverage
                    .baseline_current
            );
            assert!(incomplete.native.is_none());
            let status = crate::app::hybrid::summary(&incomplete).unwrap();
            assert!(
                status.contains("Global coverage incomplete")
                    && status.contains("detailed paths disabled"),
                "{status}"
            );
        } else {
            assert_eq!(
                coverage.detail_regions.len(),
                1,
                "one captured source in this unchanged small program"
            );
            let native = report.native.as_ref().unwrap();
            assert_eq!(native.metadata.limits, request.limits);
            assert!(
                native
                    .cfg
                    .iter()
                    .any(|block| !block.executed_pcs.is_empty())
            );
        }
        assert!(
            crate::app::hybrid::source_policy(&report)
                .unwrap()
                .contains(&format!("limit {regions}"))
        );
    }
}
